use std::collections::BTreeMap;

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use thg_core::{
    Direction, EdgeRecord, GraphMutation, GraphMutationBatch, GraphStoreError, NeighborQuery,
    NodeQuery, NodeRecord,
};

use crate::cypher::ast::{
    AggOp, CypherPattern, EdgePattern, NodePattern, ParsedCypher, PropertyFilter, ReturnItem,
    WithItem,
};
use crate::cypher::parse::parse_cypher_pest;
use crate::cypher::planner;
use crate::state::TenantGraphStore;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;

#[derive(Clone, Debug, Deserialize)]
pub struct PublicCypherBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    pub query: String,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    #[serde(default)]
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct QuerySurfaceError {
    status: StatusCode,
    code: String,
    message: String,
}

impl QuerySurfaceError {
    fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }

    pub(crate) fn unsupported(feature: impl AsRef<str>, message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported_cypher_feature",
            format!("{}: {}", feature.as_ref(), message.into()),
        )
    }

    pub(crate) fn missing_param(name: &str) -> Self {
        Self::invalid(
            "missing_cypher_param",
            format!("missing required cypher parameter ${name}"),
        )
    }

    pub(crate) fn invalid(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn payload(&self) -> Value {
        json!({
            "ok": false,
            "error": self.code,
            "message": self.message,
        })
    }
}

impl From<GraphStoreError> for QuerySurfaceError {
    fn from(error: GraphStoreError) -> Self {
        let status = match error.code.as_str() {
            "redis_graph_store_error" | "redcore_io_error" | "store_internal_error" => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            _ => StatusCode::BAD_REQUEST,
        };
        Self::new(status, error.code, error.message)
    }
}

#[derive(Clone, Debug)]
struct ParsedQuery {
    operation: QueryOperation,
    requested_limit: usize,
}

#[derive(Clone, Debug)]
enum QueryOperation {
    NodeMatch(NodeQuery),
    Neighbors(NeighborQuery),
}

// ParsedCypher, CypherPattern, NodePattern, EdgePattern, PropertyFilter,
// ReturnItem, and ReturnItem::key are imported from crate::cypher::ast.
// The hand-rolled parser body in this file delegates to
// crate::cypher::parse::parse_cypher_pest.

pub fn resolve_tenant_id(
    explicit_tenant: Option<&str>,
    default_tenant: &str,
) -> Result<String, QuerySurfaceError> {
    let tenant = explicit_tenant
        .map(str::trim)
        .filter(|tenant| !tenant.is_empty())
        .unwrap_or(default_tenant.trim());
    if tenant.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "missing_tenant_id",
            "tenant_id is required when no default tenant is configured",
        ));
    }
    Ok(tenant.to_string())
}

pub fn execute_public_query(
    store: &TenantGraphStore,
    tenant: &str,
    body: &Value,
) -> Result<Value, QuerySurfaceError> {
    let parsed = parse_public_query(body)?;
    let explain = explain_public_query(tenant, body)?;
    match parsed.operation {
        QueryOperation::NodeMatch(mut query) => {
            query.limit = Some(parsed.requested_limit.saturating_add(1));
            let mut nodes = store.query_nodes(query)?;
            let truncated = nodes.len() > parsed.requested_limit;
            if truncated {
                nodes.truncate(parsed.requested_limit);
            }
            Ok(json!({
                "ok": true,
                "tenant": tenant,
                "operation": "node_match",
                "nodes": nodes,
                "stats": {
                    "returned": nodes.len(),
                    "truncated": truncated
                },
                "explain": explain,
            }))
        }
        QueryOperation::Neighbors(query) => {
            let mut neighbors = store.neighbors(query)?;
            let truncated = neighbors.len() > parsed.requested_limit;
            if truncated {
                neighbors.truncate(parsed.requested_limit);
            }
            Ok(json!({
                "ok": true,
                "tenant": tenant,
                "operation": "neighbors",
                "neighbors": neighbors,
                "stats": {
                    "returned": neighbors.len(),
                    "truncated": truncated
                },
                "explain": explain,
            }))
        }
    }
}

pub fn explain_public_query(tenant: &str, body: &Value) -> Result<Value, QuerySurfaceError> {
    let parsed = parse_public_query(body)?;
    let (operation, plan) = match parsed.operation {
        QueryOperation::NodeMatch(query) => {
            let seek = if query.label.is_some() || !query.properties.is_empty() {
                "node_index_seek"
            } else {
                "node_scan"
            };
            (
                "node_match",
                json!([
                    {
                        "op": seek,
                        "bounded": true,
                        "limit": parsed.requested_limit,
                        "uses": {
                            "label": query.label,
                            "properties": query.properties
                        }
                    },
                    {
                        "op": "limit",
                        "count": parsed.requested_limit
                    }
                ]),
            )
        }
        QueryOperation::Neighbors(query) => (
            "neighbors",
            json!([
                {
                    "op": "adjacency_seek",
                    "bounded": true,
                    "limit": parsed.requested_limit,
                    "direction": match query.direction {
                        Direction::Out => "out",
                        Direction::In => "in",
                    },
                    "edge_type": query.edge_type
                },
                {
                    "op": "limit",
                    "count": parsed.requested_limit
                }
            ]),
        ),
    };
    Ok(json!({
        "tenant": tenant,
        "operation": operation,
        "plan": plan,
        "compatibility": {
            "supported_operations": ["node_match", "neighbors"],
            "pending": ["multi_hop_expand", "path_projection", "sorting", "aggregation"]
        }
    }))
}

pub fn execute_cypher_query(
    store: &mut TenantGraphStore,
    tenant: &str,
    body: &PublicCypherBody,
) -> Result<Value, QuerySurfaceError> {
    let parsed = parse_cypher(&body.query, &body.params)?;
    if !parsed.writes.is_empty() {
        return execute_write_cypher(store, tenant, &parsed);
    }
    let explain = explain_cypher_query(tenant, body)?;
    match &parsed.pattern {
        CypherPattern::Node(node_pattern) => {
            // For pure-COUNT queries we ignore LIMIT for the scan, count all
            // matching nodes, and emit one aggregate row.
            let pure_count = returns_are_pure_count(&parsed.returns);
            let pipeline = needs_pipeline(&parsed);
            let scan_limit = if pure_count || pipeline {
                // Pipeline operators (SUM/AVG/MIN/MAX, ORDER BY, SKIP) need to
                // see every matching row before deciding what to keep.
                usize::MAX
            } else {
                parsed.limit.saturating_add(1)
            };
            let query = node_query_for_pattern(
                node_pattern,
                parsed
                    .where_filter
                    .as_ref()
                    .filter(|filter| filter.binding == node_pattern.binding),
                scan_limit,
            )?;
            let mut nodes = store.query_nodes(query.clone())?;
            if let Some(filter) = parsed.where_filter.as_ref() {
                nodes.retain(|node| {
                    filter.binding == node_pattern.binding && node_matches_filter(node, filter)
                });
            }

            if pure_count {
                let count = nodes.len();
                let mut row = serde_json::Map::new();
                for item in &parsed.returns {
                    if let ReturnItem::Count { expression, .. } = item {
                        row.insert(expression.clone(), json!(count));
                    }
                }
                return Ok(json!({
                    "ok": true,
                    "tenant": tenant,
                    "query": parsed.normalized,
                    "subset": "opencypher_v0_1_read_only",
                    "rows": [Value::Object(row)],
                    "row_count": 1,
                    "stats": {
                        "returned": 1,
                        "truncated": false,
                        "plan_operation": "aggregate_count",
                    },
                    "explain": explain,
                }));
            }

            if pipeline {
                let total_matched = nodes.len();
                let raw_rows: Vec<serde_json::Map<String, Value>> = nodes
                    .iter()
                    .map(|node| node_to_raw_row(node, node_pattern))
                    .collect();
                let processed = run_pipeline(raw_rows, &parsed);
                let row_count = processed.len();
                let truncated = parsed.with_clause.is_none()
                    && parsed.limit > 0
                    && total_matched > row_count + parsed.skip.unwrap_or(0);
                let plan_op = if parsed.with_clause.is_some() {
                    "node_with_aggregate"
                } else if returns_have_aggregate(&parsed.returns) {
                    "node_implicit_aggregate"
                } else {
                    "node_pipeline"
                };
                let rows: Vec<Value> = processed.into_iter().map(Value::Object).collect();
                return Ok(json!({
                    "ok": true,
                    "tenant": tenant,
                    "query": parsed.normalized,
                    "subset": "opencypher_v0_1_read_only",
                    "rows": rows,
                    "row_count": row_count,
                    "stats": {
                        "returned": row_count,
                        "truncated": truncated,
                        "plan_operation": plan_op,
                    },
                    "explain": explain,
                }));
            }

            let truncated = nodes.len() > parsed.limit;
            if truncated {
                nodes.truncate(parsed.limit);
            }
            let rows = nodes
                .iter()
                .map(|node| project_node_row(&parsed.returns, node_pattern, node))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({
                "ok": true,
                "tenant": tenant,
                "query": parsed.normalized,
                "subset": "opencypher_v0_1_read_only",
                "rows": rows,
                "row_count": rows.len(),
                "stats": {
                    "returned": rows.len(),
                    "truncated": truncated,
                    "plan_operation": if query.label.is_some() || !query.properties.is_empty() {
                        "node_index_seek"
                    } else {
                        "node_scan"
                    }
                },
                "explain": explain,
            }))
        }
        CypherPattern::Edge(edge_pattern) => {
            let left_filter = parsed
                .where_filter
                .as_ref()
                .filter(|filter| filter.binding == edge_pattern.left.binding);
            let right_filter = parsed
                .where_filter
                .as_ref()
                .filter(|filter| filter.binding == edge_pattern.right.binding);
            let seed_query = node_query_for_pattern(
                &edge_pattern.left,
                left_filter,
                parsed.limit.saturating_add(1),
            )?;
            if seed_query.label.is_none() && seed_query.properties.is_empty() {
                return Err(QuerySurfaceError::unsupported(
                    "relationship_scan",
                    "relationship MATCH requires at least a left label or left property filter in this slice",
                ));
            }
            let seeds = store.query_nodes(seed_query.clone())?;
            let mut rows = Vec::new();
            let mut edges_touched = 0usize;
            for seed in &seeds {
                if let Some(filter) = left_filter {
                    if !node_matches_filter(seed, filter) {
                        continue;
                    }
                }
                let neighbors = store.neighbors(NeighborQuery {
                    node_id: seed.id.clone(),
                    direction: Direction::Out,
                    edge_type: Some(edge_pattern.edge_type.clone()),
                })?;
                edges_touched += neighbors.len();
                for hit in neighbors {
                    let Some(target) = store.get_node(&hit.node_id)? else {
                        continue;
                    };
                    if !node_matches_pattern(&target, &edge_pattern.right) {
                        continue;
                    }
                    if let Some(filter) = right_filter {
                        if !node_matches_filter(&target, filter) {
                            continue;
                        }
                    }
                    rows.push(project_edge_row(
                        &parsed.returns,
                        edge_pattern,
                        seed,
                        &target,
                    )?);
                    if rows.len() > parsed.limit {
                        break;
                    }
                }
                if rows.len() > parsed.limit {
                    break;
                }
            }
            let truncated = rows.len() > parsed.limit;
            if truncated {
                rows.truncate(parsed.limit);
            }
            Ok(json!({
                "ok": true,
                "tenant": tenant,
                "query": parsed.normalized,
                "subset": "opencypher_v0_1_read_only",
                "rows": rows,
                "row_count": rows.len(),
                "stats": {
                    "returned": rows.len(),
                    "truncated": truncated,
                    "seed_nodes": seeds.len(),
                    "edges_touched": edges_touched,
                    "plan_operation": if seed_query.properties.is_empty() {
                        "node_by_label_expand_out"
                    } else {
                        "node_index_seek_expand_out"
                    }
                },
                "explain": explain,
            }))
        }
        CypherPattern::EdgeChain(chain) => {
            // Multi-hop chain: walk one hop at a time, filtering each step's
            // target by label and inline properties. Emit one row per leaf walk.
            let seed_filter = parsed
                .where_filter
                .as_ref()
                .filter(|f| f.binding == chain.start.binding);
            let seed_query =
                node_query_for_pattern(&chain.start, seed_filter, parsed.limit.saturating_add(1))?;
            if seed_query.label.is_none() && seed_query.properties.is_empty() {
                return Err(QuerySurfaceError::unsupported(
                    "chain_scan",
                    "multi-hop chain requires a label or property filter on the starting node",
                ));
            }
            let seeds = store.query_nodes(seed_query.clone())?;
            let mut rows: Vec<Value> = Vec::new();
            let mut hops_touched: usize = 0;

            // Recursive walk via an explicit stack: each frame holds the
            // current step index and the bindings accumulated so far.
            for seed in &seeds {
                if let Some(filter) = seed_filter {
                    if !node_matches_filter(seed, filter) {
                        continue;
                    }
                }
                walk_chain(
                    store,
                    chain,
                    &parsed,
                    seed,
                    0,
                    &mut Vec::from([(chain.start.binding.clone(), seed.clone())]),
                    &mut rows,
                    &mut hops_touched,
                )?;
                if rows.len() > parsed.limit {
                    break;
                }
            }
            let truncated = rows.len() > parsed.limit;
            if truncated {
                rows.truncate(parsed.limit);
            }
            Ok(json!({
                "ok": true,
                "tenant": tenant,
                "query": parsed.normalized,
                "subset": "opencypher_v0_1_read_only",
                "rows": rows,
                "row_count": rows.len(),
                "stats": {
                    "returned": rows.len(),
                    "truncated": truncated,
                    "seed_nodes": seeds.len(),
                    "hops_touched": hops_touched,
                    "plan_operation": "multi_hop_chain_walk",
                },
                "explain": explain,
            }))
        }
        CypherPattern::EdgeVarLength(var) => {
            let from_filter = parsed
                .where_filter
                .as_ref()
                .filter(|f| f.binding == var.from.binding);
            let to_filter = parsed
                .where_filter
                .as_ref()
                .filter(|f| f.binding == var.to.binding);
            let from_query =
                node_query_for_pattern(&var.from, from_filter, parsed.limit.saturating_add(1))?;
            if from_query.label.is_none() && from_query.properties.is_empty() {
                return Err(QuerySurfaceError::unsupported(
                    "var_length_scan",
                    "variable-length expand requires a label or property filter on the source",
                ));
            }
            let froms = store.query_nodes(from_query.clone())?;
            let max_hops = var.max.unwrap_or(8);
            if max_hops == 0 || var.min > max_hops {
                return Err(QuerySurfaceError::invalid(
                    "invalid_var_length_range",
                    format!(
                        "variable-length range {}..{:?} is empty or invalid",
                        var.min, var.max
                    ),
                ));
            }
            let mut rows: Vec<Value> = Vec::new();
            let mut endpoints_touched: usize = 0;

            for from_node in &froms {
                if let Some(filter) = from_filter {
                    if !node_matches_filter(from_node, filter) {
                        continue;
                    }
                }
                let mut visited: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut frontier: Vec<(String, Vec<String>)> =
                    vec![(from_node.id.clone(), vec![from_node.id.clone()])];
                visited.insert(from_node.id.clone());
                let mut depth: usize = 0;
                while depth < max_hops && !frontier.is_empty() {
                    depth += 1;
                    let mut next: Vec<(String, Vec<String>)> = Vec::new();
                    for (current_id, path) in frontier.drain(..) {
                        let neighbors = store.neighbors(NeighborQuery {
                            node_id: current_id,
                            direction: Direction::Out,
                            edge_type: Some(var.edge_type.clone()),
                        })?;
                        for hit in neighbors {
                            if visited.contains(&hit.node_id) {
                                continue;
                            }
                            let mut next_path = path.clone();
                            next_path.push(hit.node_id.clone());
                            if depth >= var.min {
                                if let Some(target_node) = store.get_node(&hit.node_id)? {
                                    endpoints_touched += 1;
                                    if !node_matches_pattern(&target_node, &var.to) {
                                        continue;
                                    }
                                    if let Some(filter) = to_filter {
                                        if !node_matches_filter(&target_node, filter) {
                                            continue;
                                        }
                                    }
                                    rows.push(project_var_length_row(
                                        &parsed.returns,
                                        var,
                                        from_node,
                                        &target_node,
                                        &next_path,
                                    )?);
                                    if rows.len() > parsed.limit {
                                        break;
                                    }
                                }
                            }
                            visited.insert(hit.node_id.clone());
                            next.push((hit.node_id, next_path));
                        }
                        if rows.len() > parsed.limit {
                            break;
                        }
                    }
                    frontier = next;
                    if rows.len() > parsed.limit {
                        break;
                    }
                }
                if rows.len() > parsed.limit {
                    break;
                }
            }
            let truncated = rows.len() > parsed.limit;
            if truncated {
                rows.truncate(parsed.limit);
            }
            Ok(json!({
                "ok": true,
                "tenant": tenant,
                "query": parsed.normalized,
                "subset": "opencypher_v0_1_read_only",
                "rows": rows,
                "row_count": rows.len(),
                "stats": {
                    "returned": rows.len(),
                    "truncated": truncated,
                    "seed_nodes": froms.len(),
                    "endpoints_touched": endpoints_touched,
                    "max_hops": max_hops,
                    "plan_operation": "variable_length_expand",
                },
                "explain": explain,
            }))
        }
    }
}

fn walk_chain(
    store: &TenantGraphStore,
    chain: &crate::cypher::ast::EdgeChain,
    parsed: &ParsedCypher,
    _current_seed: &NodeRecord,
    step_index: usize,
    path_bindings: &mut Vec<(String, NodeRecord)>,
    rows: &mut Vec<Value>,
    hops_touched: &mut usize,
) -> Result<(), QuerySurfaceError> {
    if step_index >= chain.steps.len() {
        rows.push(project_chain_row(&parsed.returns, chain, path_bindings)?);
        return Ok(());
    }
    if rows.len() > parsed.limit {
        return Ok(());
    }
    let step = &chain.steps[step_index];
    let current_id = path_bindings
        .last()
        .map(|(_, node)| node.id.clone())
        .expect("path_bindings non-empty");
    let neighbors = store.neighbors(NeighborQuery {
        node_id: current_id,
        direction: Direction::Out,
        edge_type: Some(step.edge_type.clone()),
    })?;
    *hops_touched += neighbors.len();
    let step_filter = parsed
        .where_filter
        .as_ref()
        .filter(|f| f.binding == step.target.binding);
    for hit in neighbors {
        if rows.len() > parsed.limit {
            return Ok(());
        }
        let Some(target_node) = store.get_node(&hit.node_id)? else {
            continue;
        };
        if !node_matches_pattern(&target_node, &step.target) {
            continue;
        }
        if let Some(filter) = step_filter {
            if !node_matches_filter(&target_node, filter) {
                continue;
            }
        }
        path_bindings.push((step.target.binding.clone(), target_node.clone()));
        walk_chain(
            store,
            chain,
            parsed,
            &target_node,
            step_index + 1,
            path_bindings,
            rows,
            hops_touched,
        )?;
        path_bindings.pop();
    }
    Ok(())
}

fn project_chain_row(
    items: &[ReturnItem],
    chain: &crate::cypher::ast::EdgeChain,
    bindings: &[(String, NodeRecord)],
) -> Result<Value, QuerySurfaceError> {
    let mut row = serde_json::Map::new();
    let find_binding = |name: &str| -> Option<&NodeRecord> {
        bindings.iter().find_map(|(b, n)| (b == name).then_some(n))
    };
    for item in items {
        match item {
            ReturnItem::Variable(binding) => match find_binding(binding) {
                Some(node) => {
                    row.insert(binding.clone(), json!(node));
                }
                None => {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN binding {binding} does not exist in the chain"),
                    ));
                }
            },
            ReturnItem::Property {
                binding,
                key,
                expression,
            } => match find_binding(binding) {
                Some(node) => {
                    row.insert(expression.clone(), property_value(node, key));
                }
                None => {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN binding {binding} does not exist in the chain"),
                    ));
                }
            },
            ReturnItem::Count { .. } => {
                unreachable!("count items handled at the executor level");
            }
            ReturnItem::Aggregate { expression, .. } => {
                return Err(QuerySurfaceError::unsupported(
                    "aggregate_projection_pending",
                    format!("SUM/AVG/MIN/MAX aggregations land in §P2-C pc.2.2: {expression}"),
                ));
            }
            ReturnItem::Path {
                binding,
                expression,
            } => {
                if chain.path_binding.as_deref() != Some(binding.as_str()) {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN path {binding} is not bound by this MATCH"),
                    ));
                }
                let ids: Vec<String> = bindings.iter().map(|(_, n)| n.id.clone()).collect();
                row.insert(expression.clone(), json!(ids));
            }
        }
    }
    Ok(Value::Object(row))
}

fn project_var_length_row(
    items: &[ReturnItem],
    var: &crate::cypher::ast::EdgeVarLength,
    from: &NodeRecord,
    to: &NodeRecord,
    path_ids: &[String],
) -> Result<Value, QuerySurfaceError> {
    let mut row = serde_json::Map::new();
    for item in items {
        match item {
            ReturnItem::Variable(binding) if binding == &var.from.binding => {
                row.insert(binding.clone(), json!(from));
            }
            ReturnItem::Variable(binding) if binding == &var.to.binding => {
                row.insert(binding.clone(), json!(to));
            }
            ReturnItem::Property {
                binding,
                key,
                expression,
            } if binding == &var.from.binding => {
                row.insert(expression.clone(), property_value(from, key));
            }
            ReturnItem::Property {
                binding,
                key,
                expression,
            } if binding == &var.to.binding => {
                row.insert(expression.clone(), property_value(to, key));
            }
            ReturnItem::Variable(binding) | ReturnItem::Property { binding, .. } => {
                return Err(QuerySurfaceError::invalid(
                    "invalid_return_binding",
                    format!("RETURN binding {binding} does not exist in the var-length pattern"),
                ));
            }
            ReturnItem::Count { .. } => {
                unreachable!("count items handled at the executor level");
            }
            ReturnItem::Aggregate { expression, .. } => {
                return Err(QuerySurfaceError::unsupported(
                    "aggregate_projection_pending",
                    format!("SUM/AVG/MIN/MAX aggregations land in §P2-C pc.2.2: {expression}"),
                ));
            }
            ReturnItem::Path {
                binding,
                expression,
            } => {
                if var.path_binding.as_deref() != Some(binding.as_str()) {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN path {binding} is not bound by this MATCH"),
                    ));
                }
                row.insert(expression.clone(), json!(path_ids));
            }
        }
    }
    Ok(Value::Object(row))
}

fn execute_write_cypher(
    store: &mut TenantGraphStore,
    tenant: &str,
    parsed: &ParsedCypher,
) -> Result<Value, QuerySurfaceError> {
    use crate::cypher::ast::WriteClause;

    let mut batch = crate::cypher::compile::compile_writes(&parsed.writes)?;

    for write in &parsed.writes {
        match write {
            WriteClause::CreateNode { .. } | WriteClause::CreateEdge { .. } => {
                // Already compiled by compile_writes.
            }
            WriteClause::Merge {
                node,
                on_create,
                on_match,
            } => {
                let id = node
                    .properties
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        QuerySurfaceError::invalid(
                            "missing_merge_id",
                            "MERGE node requires `id` property",
                        )
                    })?;
                let existing = store.get_node(id)?;
                let mut props: serde_json::Map<String, Value> = match &existing {
                    Some(existing_node) => match existing_node.properties.clone() {
                        Value::Object(map) => map,
                        _ => serde_json::Map::new(),
                    },
                    None => {
                        let mut map = serde_json::Map::new();
                        for (k, v) in node.properties.iter() {
                            map.insert(k.clone(), v.clone());
                        }
                        map
                    }
                };
                let branch = if existing.is_some() {
                    on_match.as_ref()
                } else {
                    on_create.as_ref()
                };
                if let Some(branch) = branch {
                    for (set_binding, key, expr) in &branch.sets {
                        if set_binding != &node.binding {
                            return Err(QuerySurfaceError::invalid(
                                "set_binding_mismatch",
                                format!(
                                    "SET binding {set_binding} does not match MERGE binding {}",
                                    node.binding
                                ),
                            ));
                        }
                        let resolved = resolve_set_expr(expr, &props)?;
                        props.insert(key.clone(), resolved);
                    }
                }
                let labels = node
                    .label
                    .as_ref()
                    .map(|l| vec![l.clone()])
                    .unwrap_or_default();
                let record =
                    thg_core::NodeRecord::new(id.to_string(), labels, Value::Object(props));
                batch
                    .mutations
                    .push(thg_core::GraphMutation::NodeUpsert(record));
            }
            WriteClause::Set {
                binding,
                key,
                value,
            } => {
                let pattern_node = match &parsed.pattern {
                    CypherPattern::Node(n) if n.binding == *binding => n,
                    other => {
                        return Err(QuerySurfaceError::invalid(
                            "set_binding_unresolved",
                            format!("SET binding {binding} not bound by MATCH pattern: {other:?}"),
                        ));
                    }
                };
                let where_filter = parsed
                    .where_filter
                    .as_ref()
                    .filter(|f| f.binding == *binding);
                let query = node_query_for_pattern(pattern_node, where_filter, MAX_LIMIT)?;
                let nodes = store.query_nodes(query)?;
                for mut node in nodes {
                    let mut props: serde_json::Map<String, Value> = match node.properties.clone() {
                        Value::Object(map) => map,
                        _ => serde_json::Map::new(),
                    };
                    let resolved = resolve_set_expr(value, &props)?;
                    props.insert(key.clone(), resolved);
                    node.properties = Value::Object(props);
                    batch
                        .mutations
                        .push(thg_core::GraphMutation::NodeUpsert(node));
                }
            }
            WriteClause::Delete { binding, detach } => {
                let pattern_node = match &parsed.pattern {
                    CypherPattern::Node(n) if n.binding == *binding => n,
                    other => {
                        return Err(QuerySurfaceError::invalid(
                            "delete_binding_unresolved",
                            format!("DELETE binding {binding} not bound by MATCH: {other:?}"),
                        ));
                    }
                };
                let where_filter = parsed
                    .where_filter
                    .as_ref()
                    .filter(|f| f.binding == *binding);
                let query = node_query_for_pattern(pattern_node, where_filter, MAX_LIMIT)?;
                let nodes = store.query_nodes(query)?;
                for node in nodes {
                    if *detach {
                        for direction in [Direction::Out, Direction::In] {
                            let hits = store.neighbors(NeighborQuery {
                                node_id: node.id.clone(),
                                direction,
                                edge_type: None,
                            })?;
                            for hit in hits {
                                if let Some(edge) = store.get_edge(&hit.edge_id)? {
                                    let mut tombstoned = edge.clone();
                                    tombstoned.tombstone = true;
                                    batch
                                        .mutations
                                        .push(thg_core::GraphMutation::EdgeUpsert(tombstoned));
                                }
                            }
                        }
                    }
                    let mut tombstoned = node.clone();
                    tombstoned.tombstone = true;
                    batch
                        .mutations
                        .push(thg_core::GraphMutation::NodeUpsert(tombstoned));
                }
            }
        }
    }

    let transaction = store.commit_batch(batch)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant,
        "query": parsed.normalized,
        "subset": "opencypher_v0_1_read_write",
        "transaction": {
            "graph_version": transaction.graph_version,
            "writes": transaction.writes,
        },
    }))
}

fn resolve_set_expr(
    expr: &crate::cypher::ast::SetExpr,
    current_props: &serde_json::Map<String, Value>,
) -> Result<Value, QuerySurfaceError> {
    use crate::cypher::ast::SetExpr;
    match expr {
        SetExpr::Literal(value) => Ok(value.clone()),
        SetExpr::Increment {
            base_binding: _,
            base_key,
            delta,
        } => {
            let current = current_props.get(base_key).cloned().unwrap_or(Value::Null);
            let current_int = current.as_i64().unwrap_or(0);
            let delta_int = delta.as_i64().unwrap_or(0);
            Ok(json!(current_int + delta_int))
        }
    }
}

pub fn parse_tx_cypher_mutations(
    query: &str,
    params: &BTreeMap<String, Value>,
) -> Result<GraphMutationBatch, QuerySurfaceError> {
    let normalized = normalize_query(query);
    if normalized.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "empty_cypher_query",
            "query is required",
        ));
    }
    let upper = normalized.to_ascii_uppercase();
    if upper.starts_with("EXPLAIN ") || upper.starts_with("PROFILE ") {
        return Err(QuerySurfaceError::unsupported(
            "query_prefix",
            "use POST /v1/cypher/explain instead of EXPLAIN or PROFILE query prefixes",
        ));
    }
    for keyword in ["DELETE ", "DETACH DELETE ", "SET ", "REMOVE "] {
        if upper.starts_with(keyword) || upper.contains(&format!(" {keyword}")) {
            return Err(QuerySurfaceError::unsupported(
                keyword.trim(),
                "write transaction mode currently supports CREATE and MERGE only",
            ));
        }
    }
    if upper.starts_with("CREATE ") {
        parse_tx_create_mutation(&normalized["CREATE ".len()..].trim(), params)
    } else if upper.starts_with("MERGE ") {
        parse_tx_create_mutation(&normalized["MERGE ".len()..].trim(), params)
    } else {
        Err(QuerySurfaceError::unsupported(
            "tx_mode",
            "transactional /v1/cypher requires CREATE or MERGE statements",
        ))
    }
}

pub fn explain_cypher_query(
    tenant: &str,
    body: &PublicCypherBody,
) -> Result<Value, QuerySurfaceError> {
    let parsed = parse_cypher(&body.query, &body.params)?;
    let (pattern, plan) = match &parsed.pattern {
        CypherPattern::Node(node_pattern) => {
            let seek = if node_pattern.label.is_some() || !node_pattern.properties.is_empty() {
                "node_index_seek"
            } else {
                "node_scan"
            };
            (
                "node_match",
                json!([
                    {
                        "op": seek,
                        "binding": node_pattern.binding,
                        "label": node_pattern.label,
                        "properties": node_pattern.properties,
                        "bounded": true
                    },
                    {
                        "op": "project",
                        "returns": parsed.returns.iter().map(ReturnItem::key).collect::<Vec<_>>()
                    },
                    {
                        "op": "limit",
                        "count": parsed.limit
                    }
                ]),
            )
        }
        CypherPattern::Edge(edge_pattern) => {
            let left_seek = if edge_pattern.left.properties.is_empty() {
                "node_by_label"
            } else {
                "node_index_seek"
            };
            (
                "relationship_expand",
                json!([
                    {
                        "op": left_seek,
                        "binding": edge_pattern.left.binding,
                        "label": edge_pattern.left.label,
                        "properties": edge_pattern.left.properties,
                        "bounded": true
                    },
                    {
                        "op": "expand_out",
                        "edge_type": edge_pattern.edge_type,
                        "target_binding": edge_pattern.right.binding,
                        "target_label": edge_pattern.right.label,
                        "target_properties": edge_pattern.right.properties
                    },
                    {
                        "op": "project",
                        "returns": parsed.returns.iter().map(ReturnItem::key).collect::<Vec<_>>()
                    },
                    {
                        "op": "limit",
                        "count": parsed.limit
                    }
                ]),
            )
        }
        CypherPattern::EdgeChain(chain) => (
            "multi_hop_chain",
            json!([
                {
                    "op": "node_by_label",
                    "binding": chain.start.binding,
                    "label": chain.start.label,
                    "properties": chain.start.properties,
                },
                {
                    "op": "expand_chain",
                    "hops": chain.steps.iter().map(|step| json!({
                        "edge_type": step.edge_type,
                        "target_binding": step.target.binding,
                        "target_label": step.target.label,
                    })).collect::<Vec<_>>(),
                },
                {
                    "op": "project",
                    "returns": parsed.returns.iter().map(ReturnItem::key).collect::<Vec<_>>()
                },
                {
                    "op": "limit",
                    "count": parsed.limit
                }
            ]),
        ),
        CypherPattern::EdgeVarLength(var) => (
            "variable_length_expand",
            json!([
                {
                    "op": "node_by_label",
                    "binding": var.from.binding,
                    "label": var.from.label,
                    "properties": var.from.properties,
                },
                {
                    "op": "expand_var_length",
                    "edge_type": var.edge_type,
                    "min_hops": var.min,
                    "max_hops": var.max,
                    "target_binding": var.to.binding,
                    "target_label": var.to.label,
                },
                {
                    "op": "project",
                    "returns": parsed.returns.iter().map(ReturnItem::key).collect::<Vec<_>>()
                },
                {
                    "op": "limit",
                    "count": parsed.limit
                }
            ]),
        ),
    };
    Ok(json!({
        "ok": true,
        "tenant": tenant,
        "query": parsed.normalized,
        "subset": "opencypher_v0_1_read_only",
        "pattern": pattern,
        "plan": plan,
        "compatibility": cypher_compatibility_matrix(),
    }))
}

pub fn cypher_compatibility_matrix() -> Value {
    json!({
        "version": "opencypher_v0_1_read_only",
        "supported": [
            "MATCH (n) RETURN n LIMIT <n>",
            "MATCH (n:Label {prop: $value}) RETURN n LIMIT <n>",
            "MATCH (n:Label) WHERE n.prop = $value RETURN n LIMIT <n>",
            "MATCH (a:Label)-[:TYPE]->(b:Label) RETURN a, b LIMIT <n>",
            "MATCH (a:Label)-[:TYPE]->(b:Label) RETURN a.prop, b.prop LIMIT <n>",
            "MATCH (a)-[:TYPE]->(b)-[:TYPE]->(c) RETURN a, b, c LIMIT <n>",
            "MATCH p = (a)-[:TYPE*1..3]->(b) RETURN p LIMIT <n>"
        ],
        "rejected": [
            "REMOVE",
            "OPTIONAL MATCH, WITH, UNION",
            "ORDER BY, SKIP, DISTINCT, aggregation",
            "incoming and undirected relationship patterns",
            "procedures, PROFILE, and EXPLAIN query prefixes"
        ],
        "pending": [
            "sorting and aggregation",
            "procedures over search, cache, and GraphRAG"
        ]
    })
}

fn parse_public_query(body: &Value) -> Result<ParsedQuery, QuerySurfaceError> {
    let operation = body
        .get("operation")
        .or_else(|| body.get("op"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if body.get("node_id").is_some() || body.get("direction").is_some() {
                "neighbors"
            } else {
                "node_match"
            }
        });
    let requested_limit = parse_limit(body.get("limit"))?;
    match operation {
        "node_match" | "node_index_seek" => {
            let label = body
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_string);
            let properties = body
                .get("properties")
                .or_else(|| body.get("props"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let properties = serde_json::from_value::<BTreeMap<String, Value>>(properties)
                .map_err(|error| {
                    QuerySurfaceError::invalid(
                        "invalid_query_properties",
                        format!("properties must be an object: {error}"),
                    )
                })?;
            Ok(ParsedQuery {
                operation: QueryOperation::NodeMatch(NodeQuery {
                    label,
                    properties,
                    limit: Some(requested_limit),
                }),
                requested_limit,
            })
        }
        "neighbors" => {
            let node_id = body
                .get("node_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|node_id| !node_id.is_empty())
                .ok_or_else(|| {
                    QuerySurfaceError::invalid(
                        "missing_node_id",
                        "neighbors query requires node_id",
                    )
                })?;
            let direction = match body
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("out")
                .to_ascii_lowercase()
                .as_str()
            {
                "out" => Direction::Out,
                "in" => Direction::In,
                other => {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_direction",
                        format!("direction must be \"out\" or \"in\", got {other}"),
                    ))
                }
            };
            let edge_type = body
                .get("edge_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|edge_type| !edge_type.is_empty())
                .map(str::to_string);
            Ok(ParsedQuery {
                operation: QueryOperation::Neighbors(NeighborQuery {
                    node_id: node_id.to_string(),
                    direction,
                    edge_type,
                }),
                requested_limit,
            })
        }
        other => Err(QuerySurfaceError::invalid(
            "unsupported_query_operation",
            format!("supported /v1/query operations are node_match and neighbors, got {other}"),
        )),
    }
}

fn parse_cypher(
    query: &str,
    params: &BTreeMap<String, Value>,
) -> Result<ParsedCypher, QuerySurfaceError> {
    reject_unsupported_subset(query)?;
    let parsed = parse_cypher_pest(query, params)?;
    // RETURN binding validation only fires when there is no WITH clause:
    // a WITH clause introduces fresh aliases that the RETURN may reference,
    // and those aliases are not bound by the MATCH pattern.
    if parsed.with_clause.is_none() {
        validate_return_items(&parsed.pattern, &parsed.returns)?;
    }
    validate_where_filter(&parsed.pattern, parsed.where_filter.as_ref())?;
    Ok(parsed)
}

/// Read-only-subset pre-check. Surfaces `unsupported_cypher_feature` for the
/// clauses the pest grammar will accept in later stages (CREATE, MERGE, DELETE,
/// SET, REMOVE, OPTIONAL MATCH, WITH, UNION, CALL, ORDER BY, SKIP, DISTINCT,
/// SUM/AVG aggregations) so callers see a
/// more actionable error than the bare pest `invalid_cypher_query`.
fn reject_unsupported_subset(query: &str) -> Result<(), QuerySurfaceError> {
    let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Ok(());
    }
    let upper = normalized.to_ascii_uppercase();
    // CREATE / MERGE / SET / DELETE / DETACH DELETE are now supported by the
    // pest grammar (§P3-A pa3.1.2 + pa3.1.3) and the auto-tx executor
    // (§P3-A pa3.3.1). REMOVE remains unsupported.
    if upper.starts_with("REMOVE ") || upper.contains(" REMOVE ") {
        return Err(QuerySurfaceError::unsupported(
            "REMOVE",
            "REMOVE clauses are not implemented yet",
        ));
    }
    if upper.starts_with("EXPLAIN ") || upper.starts_with("PROFILE ") {
        return Err(QuerySurfaceError::unsupported(
            "query_prefix",
            "use POST /v1/cypher/explain instead of EXPLAIN or PROFILE query prefixes",
        ));
    }
    // WITH, ORDER BY, SKIP are now supported by §P2-C (pc.2.1..pc.3.1) along
    // with SUM/AVG/MIN/MAX aggregations. OPTIONAL MATCH, UNION, CALL, and
    // DISTINCT remain unsupported.
    for keyword in [" OPTIONAL MATCH ", " UNION ", " CALL ", " DISTINCT "] {
        if upper.contains(keyword) {
            return Err(QuerySurfaceError::unsupported(
                keyword.trim(),
                "that clause is not implemented in the first /v1/cypher subset",
            ));
        }
    }
    // Variable-length expand and path aliases (`MATCH p = ...`) are now
    // supported by the pest grammar (§P2-B), so do not reject them here.
    // Anything the grammar cannot parse will surface as `invalid_cypher_query`
    // from `parse_cypher_pest`, which is the correct shape.
    Ok(())
}

fn validate_where_filter(
    pattern: &CypherPattern,
    filter: Option<&PropertyFilter>,
) -> Result<(), QuerySurfaceError> {
    let Some(filter) = filter else {
        return Ok(());
    };
    let valid = match pattern {
        CypherPattern::Node(node) => node.binding == filter.binding,
        CypherPattern::Edge(edge) => {
            edge.left.binding == filter.binding || edge.right.binding == filter.binding
        }
        CypherPattern::EdgeChain(chain) => {
            chain.start.binding == filter.binding
                || chain
                    .steps
                    .iter()
                    .any(|step| step.target.binding == filter.binding)
        }
        CypherPattern::EdgeVarLength(var) => {
            var.from.binding == filter.binding || var.to.binding == filter.binding
        }
    };
    if valid {
        Ok(())
    } else {
        Err(QuerySurfaceError::invalid(
            "invalid_where_binding",
            format!(
                "WHERE binding {} does not exist in the MATCH pattern",
                filter.binding
            ),
        ))
    }
}

fn validate_return_items(
    pattern: &CypherPattern,
    returns: &[ReturnItem],
) -> Result<(), QuerySurfaceError> {
    let bindings: Vec<&str> = match pattern {
        CypherPattern::Node(node) => vec![node.binding.as_str()],
        CypherPattern::Edge(edge) => vec![edge.left.binding.as_str(), edge.right.binding.as_str()],
        CypherPattern::EdgeChain(chain) => {
            let mut out = vec![chain.start.binding.as_str()];
            for step in &chain.steps {
                out.push(step.target.binding.as_str());
            }
            if let Some(path) = chain.path_binding.as_deref() {
                out.push(path);
            }
            out
        }
        CypherPattern::EdgeVarLength(var) => {
            let mut out = vec![var.from.binding.as_str(), var.to.binding.as_str()];
            if let Some(path) = var.path_binding.as_deref() {
                out.push(path);
            }
            out
        }
    };
    for item in returns {
        match item {
            ReturnItem::Variable(binding) => {
                if !bindings.contains(&binding.as_str()) {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN binding {binding} does not exist in the MATCH pattern"),
                    ));
                }
            }
            ReturnItem::Property { binding, .. } => {
                if !bindings.contains(&binding.as_str()) {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN binding {binding} does not exist in the MATCH pattern"),
                    ));
                }
            }
            ReturnItem::Count { binding, .. } => {
                if let Some(b) = binding {
                    if !bindings.contains(&b.as_str()) {
                        return Err(QuerySurfaceError::invalid(
                            "invalid_return_binding",
                            format!("COUNT({b}) binding does not exist in the MATCH pattern"),
                        ));
                    }
                }
            }
            ReturnItem::Aggregate { binding, .. } => {
                if let Some(b) = binding {
                    if !bindings.contains(&b.as_str()) {
                        return Err(QuerySurfaceError::invalid(
                            "invalid_return_binding",
                            format!("aggregation binding {b} does not exist in the MATCH pattern"),
                        ));
                    }
                }
            }
            ReturnItem::Path { binding, .. } => {
                if !bindings.contains(&binding.as_str()) {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_binding",
                        format!("RETURN path {binding} does not exist in the MATCH pattern"),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn returns_are_pure_count(returns: &[ReturnItem]) -> bool {
    !returns.is_empty()
        && returns
            .iter()
            .all(|r| matches!(r, ReturnItem::Count { .. }))
}

/// True when RETURN contains any SUM/AVG/MIN/MAX item that needs implicit
/// aggregation. COUNT is excluded because the executor still has a dedicated
/// pure-count fast path.
fn returns_have_aggregate(returns: &[ReturnItem]) -> bool {
    returns
        .iter()
        .any(|r| matches!(r, ReturnItem::Aggregate { .. }))
}

/// True when the parsed query needs the WITH/sort/skip/aggregate pipeline.
fn needs_pipeline(parsed: &ParsedCypher) -> bool {
    parsed.with_clause.is_some()
        || returns_have_aggregate(&parsed.returns)
        || !parsed.order_by.is_empty()
        || parsed.skip.is_some()
}

/// Expand a matched node into a flat map carrying the binding, the binding-as-
/// JSON value (so RETURN n returns the whole node), and every "binding.key"
/// property path so the pipeline can read source columns by name.
fn node_to_raw_row(node: &NodeRecord, pattern: &NodePattern) -> serde_json::Map<String, Value> {
    let mut row = serde_json::Map::new();
    row.insert(pattern.binding.clone(), json!(node));
    if let Value::Object(props) = json!(node.properties.clone()) {
        for (k, v) in props {
            row.insert(format!("{}.{}", pattern.binding, k), v);
        }
    } else if let Some(map) = node.properties.as_object() {
        for (k, v) in map {
            row.insert(format!("{}.{}", pattern.binding, k), v.clone());
        }
    }
    row
}

/// Run rows through WITH (or implicit-aggregate) -> ORDER BY -> SKIP -> LIMIT.
fn run_pipeline(
    rows: Vec<serde_json::Map<String, Value>>,
    parsed: &ParsedCypher,
) -> Vec<serde_json::Map<String, Value>> {
    let mut current = rows;

    if let Some(with) = &parsed.with_clause {
        let mut group_keys: Vec<String> = Vec::new();
        let mut aggs: Vec<planner::AggregateOutput> = Vec::new();
        for item in &with.items {
            match item {
                WithItem::Field { alias, .. } => group_keys.push(alias.clone()),
                WithItem::Aggregate {
                    op,
                    key,
                    alias,
                    binding,
                } => {
                    let source = key.as_ref().map(|k| match binding {
                        Some(b) => format!("{b}.{k}"),
                        None => k.clone(),
                    });
                    aggs.push(planner::AggregateOutput {
                        alias: alias.clone(),
                        op: *op,
                        source_key: source,
                    });
                }
            }
        }
        // Pre-project: emit Field aliases (renamed columns) and preserve
        // source columns the aggregator needs.
        let projected: Vec<serde_json::Map<String, Value>> = current
            .iter()
            .map(|row| {
                let mut out = serde_json::Map::new();
                for item in &with.items {
                    match item {
                        WithItem::Field {
                            binding,
                            key,
                            alias,
                        } => {
                            let src_key = match key {
                                Some(k) => format!("{binding}.{k}"),
                                None => binding.clone(),
                            };
                            out.insert(
                                alias.clone(),
                                row.get(&src_key).cloned().unwrap_or(Value::Null),
                            );
                        }
                        WithItem::Aggregate { binding, key, .. } => {
                            if let (Some(b), Some(k)) = (binding, key) {
                                let src_key = format!("{b}.{k}");
                                out.insert(
                                    src_key.clone(),
                                    row.get(&src_key).cloned().unwrap_or(Value::Null),
                                );
                            }
                        }
                    }
                }
                out
            })
            .collect();
        current = planner::aggregate(&projected, &planner::AggregateSpec { group_keys, aggs });

        // After aggregation the rows expose WITH aliases (group keys + agg aliases).
        // Final RETURN projection keeps only the columns the user asked for.
        let mut final_rows: Vec<serde_json::Map<String, Value>> = Vec::with_capacity(current.len());
        for row in current {
            let mut out = serde_json::Map::new();
            for item in &parsed.returns {
                match item {
                    ReturnItem::Variable(name) => {
                        if let Some(v) = row.get(name) {
                            out.insert(name.clone(), v.clone());
                        }
                    }
                    ReturnItem::Property { expression, .. } => {
                        if let Some(v) = row.get(expression) {
                            out.insert(expression.clone(), v.clone());
                        }
                    }
                    ReturnItem::Count { expression, .. }
                    | ReturnItem::Aggregate { expression, .. }
                    | ReturnItem::Path { expression, .. } => {
                        if let Some(v) = row.get(expression) {
                            out.insert(expression.clone(), v.clone());
                        }
                    }
                }
            }
            final_rows.push(out);
        }
        current = final_rows;
    } else if returns_have_aggregate(&parsed.returns) {
        // Implicit aggregation: no WITH, but RETURN has SUM/AVG/MIN/MAX.
        // The whole match set is one group; emit one row keyed by expression.
        let mut aggs: Vec<planner::AggregateOutput> = Vec::new();
        for item in &parsed.returns {
            if let ReturnItem::Aggregate {
                op,
                binding,
                key,
                expression,
            } = item
            {
                let source = key.as_ref().map(|k| match binding {
                    Some(b) => format!("{b}.{k}"),
                    None => k.clone(),
                });
                aggs.push(planner::AggregateOutput {
                    alias: expression.clone(),
                    op: *op,
                    source_key: source,
                });
            }
        }
        current = planner::aggregate(
            &current,
            &planner::AggregateSpec {
                group_keys: Vec::new(),
                aggs,
            },
        );
    }

    planner::sort_rows(&mut current, &parsed.order_by);
    if let Some(skip) = parsed.skip {
        current = planner::skip_rows(current, skip);
    }
    if parsed.limit > 0 && current.len() > parsed.limit {
        current.truncate(parsed.limit);
    }
    // Aggregate-emitted Sum can be float-with-fract-0 when divided by f64;
    // serde-json keeps the format we emit, so no normalization needed here.
    let _ = AggOp::Count; // ensure import resolves under #[allow(dead_code)] flow
    current
}

fn parse_tx_create_mutation(
    source: &str,
    params: &BTreeMap<String, Value>,
) -> Result<GraphMutationBatch, QuerySurfaceError> {
    let source = source.trim();
    if source.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "invalid_cypher_query",
            "transactional CREATE/MERGE requires a full node or relationship pattern",
        ));
    }
    if !source.ends_with(')') {
        return Err(QuerySurfaceError::invalid(
            "invalid_cypher_query",
            "transactional CREATE/MERGE requires a closing parenthesis",
        ));
    }

    if let Some((left, right)) = source.split_once(")-[:") {
        return parse_tx_create_relationship(left, right, params);
    }

    if source.contains(" CREATE ") || source.contains(" MATCH ") || source.contains(" RETURN ") {
        return Err(QuerySurfaceError::unsupported(
            "unsupported_transaction_query",
            "transactional CREATE/MERGE supports write statements only",
        ));
    }

    parse_tx_create_node(source, params)
}

fn parse_tx_create_node(
    source: &str,
    params: &BTreeMap<String, Value>,
) -> Result<GraphMutationBatch, QuerySurfaceError> {
    let mut pattern = parse_node_pattern(source, params)?;
    let id = extract_required_id(
        "node",
        pattern.label.as_deref().unwrap_or("<unknown>"),
        &mut pattern.properties,
    )?;
    let labels = pattern.label.into_iter().collect::<Vec<_>>();
    let properties = Value::Object(
        pattern
            .properties
            .into_iter()
            .map(|(key, value)| (key, value))
            .collect(),
    );
    Ok(GraphMutationBatch::new([GraphMutation::NodeUpsert(
        NodeRecord::new(id, labels, properties),
    )]))
}

fn parse_tx_create_relationship(
    left: &str,
    right: &str,
    params: &BTreeMap<String, Value>,
) -> Result<GraphMutationBatch, QuerySurfaceError> {
    let mut left_pattern = parse_node_pattern(&format!("{left})"), params)?;
    let (relation, right) = right.split_once("]->(").ok_or_else(|| {
        QuerySurfaceError::unsupported(
            "relationship_shape",
            "transactional CREATE/MERGE relationship shape requires [:TYPE ...]->(...)",
        )
    })?;

    let (edge_type, mut edge_properties) = parse_tx_relationship_properties(relation, params)?;
    if edge_type.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "invalid_relationship_type",
            "relationship type is required for transactional edge writes",
        ));
    }
    let edge_id = extract_required_id("relationship", &edge_type, &mut edge_properties)?;
    if !right.ends_with(')') {
        return Err(QuerySurfaceError::unsupported(
            "transactional_statement",
            "transactional CREATE/MERGE relationship statements must have closed nodes",
        ));
    }
    let mut right_pattern = parse_node_pattern(&format!("({right}"), params)?;
    let from_id = extract_required_id(
        "left node",
        &left_pattern.label.as_deref().unwrap_or("<unknown>"),
        &mut left_pattern.properties,
    )?;
    let to_id = extract_required_id(
        "right node",
        &right_pattern.label.as_deref().unwrap_or("<unknown>"),
        &mut right_pattern.properties,
    )?;

    let edge_properties = Value::Object(
        edge_properties
            .into_iter()
            .map(|(key, value)| (key, value))
            .collect(),
    );
    Ok(GraphMutationBatch::new([GraphMutation::EdgeUpsert(
        EdgeRecord::new(edge_id, from_id, edge_type, to_id, edge_properties),
    )]))
}

fn parse_tx_relationship_properties(
    source: &str,
    params: &BTreeMap<String, Value>,
) -> Result<(String, BTreeMap<String, Value>), QuerySurfaceError> {
    let source = source.trim().trim_start_matches(":").trim();
    if source.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "invalid_relationship_type",
            "relationship type is required",
        ));
    }
    if !source.contains('{') {
        return Ok((source.to_string(), BTreeMap::new()));
    }
    let (raw_type, tail) = source.split_once('{').ok_or_else(|| {
        QuerySurfaceError::unsupported(
            "relationship_shape",
            "relationship property block is malformed",
        )
    })?;
    let edge_type = raw_type.trim();
    let block = format!("{{{tail}");
    let (property_block, remainder) = take_braced_block(&block)?;
    if !remainder.trim().is_empty() {
        return Err(QuerySurfaceError::invalid(
            "invalid_relationship_properties",
            "relationship property block must only contain simple key:value pairs",
        ));
    }
    Ok((
        edge_type.to_string(),
        parse_property_block(property_block, params)?,
    ))
}

fn extract_required_id(
    location: &str,
    label: &str,
    properties: &mut BTreeMap<String, Value>,
) -> Result<String, QuerySurfaceError> {
    let raw = properties.remove("id").ok_or_else(|| {
        QuerySurfaceError::invalid(
            "missing_graph_record_id",
            format!("{location} {label} mutation requires an id field"),
        )
    })?;
    match raw {
        Value::String(id) => Ok(id),
        Value::Number(number) => Ok(number.to_string()),
        _ => Err(QuerySurfaceError::invalid(
            "invalid_graph_record_id",
            format!("{location} {label} id must be a string or number"),
        )),
    }
}

fn parse_node_pattern(
    source: &str,
    params: &BTreeMap<String, Value>,
) -> Result<NodePattern, QuerySurfaceError> {
    let source = source.trim();
    if !source.starts_with('(') || !source.ends_with(')') {
        return Err(QuerySurfaceError::invalid(
            "invalid_node_pattern",
            format!("invalid node pattern {source}"),
        ));
    }
    let inner = source[1..source.len() - 1].trim();
    let (binding, mut rest) = take_identifier(inner);
    if binding.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "invalid_node_binding",
            format!("node binding is required in {source}"),
        ));
    }
    rest = rest.trim_start();
    let mut label = None;
    if rest.starts_with(':') {
        let (label_name, next) = take_identifier(rest.trim_start_matches(':'));
        if label_name.is_empty() {
            return Err(QuerySurfaceError::invalid(
                "invalid_node_label",
                format!("node label is required in {source}"),
            ));
        }
        label = Some(label_name.to_string());
        rest = next.trim_start();
    }
    let mut properties = BTreeMap::new();
    if rest.starts_with('{') {
        let (block, next) = take_braced_block(rest)?;
        properties = parse_property_block(block, params)?;
        rest = next.trim_start();
    }
    if !rest.is_empty() {
        return Err(QuerySurfaceError::unsupported(
            "node_pattern_tail",
            format!("unsupported node pattern tail {rest}"),
        ));
    }
    Ok(NodePattern {
        binding: binding.to_string(),
        label,
        properties,
    })
}

fn node_query_for_pattern(
    pattern: &NodePattern,
    filter: Option<&PropertyFilter>,
    limit: usize,
) -> Result<NodeQuery, QuerySurfaceError> {
    let mut properties = pattern.properties.clone();
    if let Some(filter) = filter {
        match properties.get(filter.key.as_str()) {
            Some(existing) if existing != &filter.value => {
                return Err(QuerySurfaceError::invalid(
                    "contradictory_filter",
                    format!(
                        "WHERE {}.{} conflicts with inline property filter",
                        filter.binding, filter.key
                    ),
                ))
            }
            Some(_) => {}
            None => {
                properties.insert(filter.key.clone(), filter.value.clone());
            }
        }
    }
    Ok(NodeQuery {
        label: pattern.label.clone(),
        properties,
        limit: Some(limit),
    })
}

fn project_node_row(
    items: &[ReturnItem],
    pattern: &NodePattern,
    node: &NodeRecord,
) -> Result<Value, QuerySurfaceError> {
    let mut row = serde_json::Map::new();
    for item in items {
        match item {
            ReturnItem::Variable(binding) if binding == &pattern.binding => {
                row.insert(binding.clone(), json!(node));
            }
            ReturnItem::Property {
                binding,
                key,
                expression,
            } if binding == &pattern.binding => {
                row.insert(expression.clone(), property_value(node, key));
            }
            ReturnItem::Variable(binding) | ReturnItem::Property { binding, .. } => {
                return Err(QuerySurfaceError::invalid(
                    "invalid_return_binding",
                    format!("RETURN binding {binding} does not exist in the MATCH pattern"),
                ))
            }
            ReturnItem::Count { .. } => {
                // COUNT handled by the executor after rows are materialized.
                // project_node_row is not called when returns are pure counts.
                unreachable!("count items handled at the executor level");
            }
            ReturnItem::Aggregate { expression, .. } => {
                return Err(QuerySurfaceError::unsupported(
                    "aggregate_projection_pending",
                    format!("SUM/AVG/MIN/MAX aggregations land in §P2-C pc.2.2: {expression}"),
                ));
            }
            ReturnItem::Path { binding, .. } => {
                return Err(QuerySurfaceError::unsupported(
                    "path_projection_on_node",
                    format!(
                        "RETURN path {binding} is only meaningful for chain or var-length patterns"
                    ),
                ));
            }
        }
    }
    Ok(Value::Object(row))
}

fn project_edge_row(
    items: &[ReturnItem],
    pattern: &EdgePattern,
    left: &NodeRecord,
    right: &NodeRecord,
) -> Result<Value, QuerySurfaceError> {
    let mut row = serde_json::Map::new();
    for item in items {
        match item {
            ReturnItem::Variable(binding) if binding == &pattern.left.binding => {
                row.insert(binding.clone(), json!(left));
            }
            ReturnItem::Variable(binding) if binding == &pattern.right.binding => {
                row.insert(binding.clone(), json!(right));
            }
            ReturnItem::Property {
                binding,
                key,
                expression,
            } if binding == &pattern.left.binding => {
                row.insert(expression.clone(), property_value(left, key));
            }
            ReturnItem::Property {
                binding,
                key,
                expression,
            } if binding == &pattern.right.binding => {
                row.insert(expression.clone(), property_value(right, key));
            }
            ReturnItem::Variable(binding) | ReturnItem::Property { binding, .. } => {
                return Err(QuerySurfaceError::invalid(
                    "invalid_return_binding",
                    format!("RETURN binding {binding} does not exist in the MATCH pattern"),
                ))
            }
            ReturnItem::Count { .. } => {
                unreachable!("count items handled at the executor level");
            }
            ReturnItem::Aggregate { expression, .. } => {
                return Err(QuerySurfaceError::unsupported(
                    "aggregate_projection_pending",
                    format!("SUM/AVG/MIN/MAX aggregations land in §P2-C pc.2.2: {expression}"),
                ));
            }
            ReturnItem::Path { binding, .. } => {
                return Err(QuerySurfaceError::unsupported(
                    "path_projection_on_edge",
                    format!(
                        "RETURN path {binding} is only meaningful for chain or var-length patterns"
                    ),
                ));
            }
        }
    }
    Ok(Value::Object(row))
}

fn node_matches_pattern(node: &NodeRecord, pattern: &NodePattern) -> bool {
    if let Some(label) = pattern.label.as_ref() {
        if !node.labels.iter().any(|candidate| candidate == label) {
            return false;
        }
    }
    pattern
        .properties
        .iter()
        .all(|(key, value)| property_value(node, key) == *value)
}

fn node_matches_filter(node: &NodeRecord, filter: &PropertyFilter) -> bool {
    property_value(node, &filter.key) == filter.value
}

fn property_value(node: &NodeRecord, key: &str) -> Value {
    node.properties.get(key).cloned().unwrap_or(Value::Null)
}

fn parse_property_block(
    block: &str,
    params: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, QuerySurfaceError> {
    let block = block.trim();
    if !block.starts_with('{') || !block.ends_with('}') {
        return Err(QuerySurfaceError::invalid(
            "invalid_property_block",
            format!("invalid property block {block}"),
        ));
    }
    let mut properties = BTreeMap::new();
    let inner = block[1..block.len() - 1].trim();
    if inner.is_empty() {
        return Ok(properties);
    }
    for entry in split_top_level(inner, ',') {
        let (key, value) = entry.split_once(':').ok_or_else(|| {
            QuerySurfaceError::invalid(
                "invalid_property_filter",
                format!("property filter must be key: value, got {entry}"),
            )
        })?;
        properties.insert(
            key.trim().to_string(),
            parse_scalar_value(value.trim(), params)?,
        );
    }
    Ok(properties)
}

fn parse_scalar_value(
    source: &str,
    params: &BTreeMap<String, Value>,
) -> Result<Value, QuerySurfaceError> {
    if let Some(name) = source.strip_prefix('$') {
        return params
            .get(name)
            .cloned()
            .ok_or_else(|| QuerySurfaceError::missing_param(name));
    }
    if source.len() >= 2
        && ((source.starts_with('"') && source.ends_with('"'))
            || (source.starts_with('\'') && source.ends_with('\'')))
    {
        return Ok(Value::String(source[1..source.len() - 1].to_string()));
    }
    match source {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "null" => return Ok(Value::Null),
        _ => {}
    }
    if let Ok(value) = source.parse::<i64>() {
        return Ok(json!(value));
    }
    if let Ok(value) = source.parse::<f64>() {
        return Ok(json!(value));
    }
    Err(QuerySurfaceError::invalid(
        "invalid_cypher_literal",
        format!("unsupported literal {source}"),
    ))
}

fn parse_limit(limit: Option<&Value>) -> Result<usize, QuerySurfaceError> {
    match limit {
        Some(Value::Number(number)) => {
            let limit = number.as_u64().ok_or_else(|| {
                QuerySurfaceError::invalid("invalid_limit", "limit must be a positive integer")
            })?;
            if limit == 0 {
                return Err(QuerySurfaceError::invalid(
                    "invalid_limit",
                    "limit must be greater than zero",
                ));
            }
            Ok((limit as usize).min(MAX_LIMIT))
        }
        Some(_) => Err(QuerySurfaceError::invalid(
            "invalid_limit",
            "limit must be a positive integer",
        )),
        None => Ok(DEFAULT_LIMIT),
    }
}

fn normalize_query(query: &str) -> String {
    let trimmed = query.trim().trim_end_matches(';');
    let mut normalized = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut last_was_space = false;
    for ch in trimmed.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                normalized.push(ch);
                last_was_space = false;
            }
            '"' if !in_single => {
                in_double = !in_double;
                normalized.push(ch);
                last_was_space = false;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !last_was_space {
                    normalized.push(' ');
                    last_was_space = true;
                }
            }
            _ => {
                normalized.push(ch);
                last_was_space = false;
            }
        }
    }
    normalized.trim().to_string()
}

fn split_top_level(source: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut brace_depth = 0usize;
    for ch in source.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '{' if !in_single && !in_double => {
                brace_depth += 1;
                current.push(ch);
            }
            '}' if !in_single && !in_double => {
                brace_depth = brace_depth.saturating_sub(1);
                current.push(ch);
            }
            c if c == separator && !in_single && !in_double && brace_depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

fn take_identifier(source: &str) -> (&str, &str) {
    let mut end = 0usize;
    for (index, ch) in source.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    source.split_at(end)
}

fn take_braced_block(source: &str) -> Result<(&str, &str), QuerySurfaceError> {
    let mut in_single = false;
    let mut in_double = false;
    let mut depth = 0usize;
    for (index, ch) in source.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '{' if !in_single && !in_double => depth += 1,
            '}' if !in_single && !in_double => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let split = index + ch.len_utf8();
                    return Ok(source.split_at(split));
                }
            }
            _ => {}
        }
    }
    Err(QuerySurfaceError::invalid(
        "invalid_property_block",
        format!("unterminated property block {source}"),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use thg_core::{EdgeRecord, GraphMutation, NodeRecord, RedCoreDurability};

    use super::{
        cypher_compatibility_matrix, execute_cypher_query, execute_public_query,
        explain_cypher_query, explain_public_query, parse_cypher, parse_tx_cypher_mutations,
        resolve_tenant_id, PublicCypherBody,
    };
    use crate::{
        config::{Config, StorageMode},
        state::{AppState, TenantGraphStore},
    };

    fn graph_store() -> TenantGraphStore {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::None,
            snapshot_interval_writes: 0,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: Default::default(),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: thg_core::HybridScoringConfig::default(),
            redis_url: "not-a-redis-url".to_string(),
            redis_key_prefix: "rusty-red".to_string(),
            require_auth: false,
            allowed_origins: Vec::new(),
            api_tokens: Vec::new(),
            service_name: "rusty-red".to_string(),
            api_title: "Rusty Red".to_string(),
            public_url: None,
            mcp_enabled: true,
            mcp_read_only: true,
            mcp_allow_admin: false,
            mcp_default_tenant: "default".to_string(),
        });
        let mut store = state.tenant_graph_store("tenant-a").unwrap();
        store
            .upsert_node(NodeRecord::new(
                "file:lib",
                ["File"],
                json!({ "path": "src/lib.rs", "repo": "rusty-red" }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "file:main",
                ["File"],
                json!({ "path": "src/main.rs", "repo": "rusty-red" }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "symbol:parse",
                ["Symbol"],
                json!({ "name": "parse" }),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:imports",
                "file:lib",
                "IMPORTS",
                "file:main",
                json!({}),
            ))
            .unwrap();
        store
    }

    #[test]
    fn resolves_tenant_from_explicit_or_default() {
        assert_eq!(
            resolve_tenant_id(Some("tenant-a"), "default").unwrap(),
            "tenant-a"
        );
        assert_eq!(resolve_tenant_id(None, "default").unwrap(), "default");
    }

    #[test]
    fn public_query_supports_node_match_and_neighbors() {
        let store = graph_store();

        let node_match = execute_public_query(
            &store,
            "demo",
            &json!({
                "operation": "node_match",
                "label": "File",
                "properties": { "path": "src/lib.rs" },
                "limit": 10
            }),
        )
        .unwrap();
        assert_eq!(node_match["nodes"][0]["id"], "file:lib");

        let neighbors = execute_public_query(
            &store,
            "demo",
            &json!({
                "operation": "neighbors",
                "node_id": "file:lib",
                "direction": "out",
                "edge_type": "IMPORTS",
                "limit": 10
            }),
        )
        .unwrap();
        assert_eq!(neighbors["neighbors"][0]["node_id"], "file:main");
    }

    #[test]
    fn public_query_explain_names_supported_subset() {
        let explain = explain_public_query(
            "demo",
            &json!({
                "operation": "node_match",
                "label": "File",
                "properties": { "path": "src/lib.rs" }
            }),
        )
        .unwrap();

        assert_eq!(explain["operation"], "node_match");
        assert_eq!(
            explain["compatibility"]["supported_operations"][0],
            "node_match"
        );
    }

    #[test]
    fn cypher_count_star_aggregates_to_scalar() {
        let mut store = graph_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:File) RETURN COUNT(*)".to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };
        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();
        assert_eq!(response["row_count"], 1);
        assert_eq!(response["rows"][0]["COUNT(*)"], 2);
        assert_eq!(response["stats"]["plan_operation"], "aggregate_count");
    }

    #[test]
    fn cypher_count_binding_aggregates_filtered_rows() {
        let mut store = graph_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:Symbol) RETURN COUNT(n)".to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };
        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();
        assert_eq!(response["row_count"], 1);
        assert_eq!(response["rows"][0]["COUNT(n)"], 1);
    }

    fn doc_store() -> TenantGraphStore {
        let mut store = graph_store();
        store
            .upsert_node(NodeRecord::new(
                "doc:a",
                ["Doc"],
                json!({ "category": "blue", "score": 5 }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "doc:b",
                ["Doc"],
                json!({ "category": "blue", "score": 7 }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "doc:c",
                ["Doc"],
                json!({ "category": "red", "score": 3 }),
            ))
            .unwrap();
        store
    }

    #[test]
    fn cypher_sum_aggregates_across_match_set() {
        let mut store = doc_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:Doc) RETURN sum(n.score)".to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };
        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();
        assert_eq!(response["row_count"], 1);
        assert_eq!(response["rows"][0]["sum(n.score)"], 15);
        assert_eq!(
            response["stats"]["plan_operation"],
            "node_implicit_aggregate"
        );
    }

    #[test]
    fn cypher_with_clause_groups_count_and_orders_desc() {
        let mut store = doc_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query:
                "MATCH (n:Doc) WITH n.category AS cat, count(n) AS c RETURN cat, c ORDER BY c DESC LIMIT 10"
                    .to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };
        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();
        let rows = response["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        // ORDER BY c DESC: blue=2 first, red=1 second.
        assert_eq!(rows[0]["cat"], "blue");
        assert_eq!(rows[0]["c"], 2);
        assert_eq!(rows[1]["cat"], "red");
        assert_eq!(rows[1]["c"], 1);
        assert_eq!(response["stats"]["plan_operation"], "node_with_aggregate");
    }

    #[test]
    fn cypher_order_by_desc_without_with_still_pipelines() {
        let mut store = doc_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:Doc) RETURN n ORDER BY n.score DESC LIMIT 2".to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };
        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();
        let rows = response["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["n"]["id"], "doc:b");
        assert_eq!(rows[1]["n"]["id"], "doc:a");
    }

    #[test]
    fn cypher_skip_drops_leading_rows() {
        let mut store = doc_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:Doc) RETURN n ORDER BY n.score ASC SKIP 1 LIMIT 5".to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };
        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();
        let rows = response["rows"].as_array().expect("rows");
        // 3 Doc rows ordered ASC by score = [3, 5, 7]; SKIP 1 drops 3.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["n"]["properties"]["score"], 5);
        assert_eq!(rows[1]["n"]["properties"]["score"], 7);
    }

    #[test]
    fn cypher_subset_executes_node_match_with_where() {
        let mut store = graph_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:File) WHERE n.path = $path RETURN n LIMIT 5".to_string(),
            params: BTreeMap::from([(String::from("path"), json!("src/lib.rs"))]),
            tx_id: None,
        };

        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();

        assert_eq!(response["row_count"], 1);
        assert_eq!(response["rows"][0]["n"]["id"], "file:lib");
    }

    #[test]
    fn cypher_subset_executes_relationship_projection() {
        let mut store = graph_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query:
                "MATCH (a:File {path: $path})-[:IMPORTS]->(b:File) RETURN a.path, b.path LIMIT 10"
                    .to_string(),
            params: BTreeMap::from([(String::from("path"), json!("src/lib.rs"))]),
            tx_id: None,
        };

        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();

        assert_eq!(response["rows"][0]["a.path"], "src/lib.rs");
        assert_eq!(response["rows"][0]["b.path"], "src/main.rs");
    }

    #[test]
    fn cypher_subset_executes_variable_length_path_projection() {
        let mut store = graph_store();
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH p = (a:File {path: $path})-[:IMPORTS*1..2]->(b:File) RETURN p LIMIT 10"
                .to_string(),
            params: BTreeMap::from([(String::from("path"), json!("src/lib.rs"))]),
            tx_id: None,
        };

        let response = execute_cypher_query(&mut store, "demo", &body).unwrap();

        assert_eq!(response["row_count"], 1);
        assert_eq!(
            response["stats"]["plan_operation"],
            "variable_length_expand"
        );
        assert_eq!(response["rows"][0]["p"], json!(["file:lib", "file:main"]));
    }

    #[test]
    fn cypher_explain_includes_compatibility_matrix() {
        let body = PublicCypherBody {
            tenant_id: Some("demo".to_string()),
            query: "MATCH (n:File) RETURN n LIMIT 10".to_string(),
            params: BTreeMap::new(),
            tx_id: None,
        };

        let response = explain_cypher_query("demo", &body).unwrap();

        assert_eq!(response["subset"], "opencypher_v0_1_read_only");
        assert_eq!(
            response["compatibility"]["supported"][0],
            "MATCH (n) RETURN n LIMIT <n>"
        );
    }

    #[test]
    fn cypher_subset_parses_writes_and_var_expands() {
        // §P3-A landed: CREATE without an `id` property still errors, but on
        // compile rather than the pre-check. We just assert parse succeeds.
        let parsed_write = parse_cypher("CREATE (n:File {id: 'a'})", &BTreeMap::new()).unwrap();
        assert_eq!(parsed_write.writes.len(), 1);

        // Var-length expand now parses (§P2-B grammar landed).
        let parsed = parse_cypher(
            "MATCH p = (a:File)-[:IMPORTS*1..2]->(b:File) RETURN p",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(matches!(
            parsed.pattern,
            crate::cypher::ast::CypherPattern::EdgeVarLength(_)
        ));

        // REMOVE remains unsupported.
        let remove_error =
            parse_cypher("MATCH (n:File {id: 'a'}) REMOVE n.path", &BTreeMap::new()).unwrap_err();
        assert_eq!(
            remove_error.payload()["error"],
            "unsupported_cypher_feature"
        );
    }

    #[test]
    fn cypher_tx_mode_parses_node_create_mutation() {
        let mutations = parse_tx_cypher_mutations(
            "CREATE (n:File {id: $id, path: $path})",
            &BTreeMap::from([
                (String::from("id"), json!("node:main")),
                (String::from("path"), json!("src/main.rs")),
            ]),
        )
        .unwrap();

        assert_eq!(mutations.mutations.len(), 1);
        match &mutations.mutations[0] {
            GraphMutation::NodeUpsert(node) => {
                assert_eq!(node.id, "node:main");
                assert_eq!(node.labels, vec!["File"]);
                assert_eq!(node.properties["path"], json!("src/main.rs"));
            }
            _ => panic!("expected node upsert mutation"),
        }
    }

    #[test]
    fn cypher_tx_mode_parses_edge_create_mutation() {
        let mutations = parse_tx_cypher_mutations(
            "CREATE (a:File {id: $from_id})-[:IMPORTS {id: $edge_id, weight: 0.5}]->(b:File {id: $to_id})",
            &BTreeMap::from([
                (String::from("from_id"), json!("file:main")),
                (String::from("to_id"), json!("file:lib")),
                (String::from("edge_id"), json!("edge:main-lib")),
            ]),
        )
        .unwrap();

        assert_eq!(mutations.mutations.len(), 1);
        match &mutations.mutations[0] {
            GraphMutation::EdgeUpsert(edge) => {
                assert_eq!(edge.id, "edge:main-lib");
                assert_eq!(edge.from_id, "file:main");
                assert_eq!(edge.to_id, "file:lib");
                assert_eq!(edge.edge_type, "IMPORTS");
            }
            _ => panic!("expected edge upsert mutation"),
        }
    }

    #[test]
    fn cypher_tx_mode_rejects_read_only_queries() {
        let error =
            parse_tx_cypher_mutations("MATCH (n:File) RETURN n", &BTreeMap::new()).unwrap_err();

        assert_eq!(error.payload()["error"], "unsupported_cypher_feature");
    }

    #[test]
    fn compatibility_matrix_lists_supported_var_length_and_pending_sorting() {
        let matrix = cypher_compatibility_matrix();

        assert!(matrix["supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry.as_str().unwrap_or("").contains("*1..3")));
        assert_eq!(matrix["pending"][0], "sorting and aggregation");
    }
}
