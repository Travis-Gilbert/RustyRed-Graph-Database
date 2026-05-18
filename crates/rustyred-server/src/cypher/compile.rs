//! Compile parsed Cypher write clauses to rustyred_core::GraphMutationBatch.
//!
//! Pure data shape conversion. CREATE clauses turn into immediate
//! NodeUpsert/EdgeUpsert mutations. MERGE / SET / DELETE require knowledge
//! of the post-MATCH state and are resolved by the executor in router.rs;
//! they show up as empty entries here.

use serde_json::{json, Map, Value};
use rustyred_core::{EdgeRecord, GraphMutation, GraphMutationBatch, NodeRecord};

use crate::cypher::ast::{EdgePattern, NodePattern, WriteClause};
use crate::query_surface::QuerySurfaceError;

pub fn compile_writes(writes: &[WriteClause]) -> Result<GraphMutationBatch, QuerySurfaceError> {
    let mut mutations: Vec<GraphMutation> = Vec::new();
    for write in writes {
        match write {
            WriteClause::CreateNode { node } => {
                let record = node_pattern_to_record(node)?;
                mutations.push(GraphMutation::NodeUpsert(record));
            }
            WriteClause::CreateEdge { edge } => {
                let record = edge_pattern_to_record(edge)?;
                mutations.push(GraphMutation::EdgeUpsert(record));
            }
            // The executor handles MERGE, SET, DELETE at runtime once nodes are matched.
            WriteClause::Merge { .. } | WriteClause::Set { .. } | WriteClause::Delete { .. } => {}
        }
    }
    Ok(GraphMutationBatch::new(mutations))
}

fn node_pattern_to_record(node: &NodePattern) -> Result<NodeRecord, QuerySurfaceError> {
    let id = node
        .properties
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            QuerySurfaceError::invalid(
                "missing_create_id",
                format!(
                    "CREATE/MERGE node pattern requires a property `id` (binding {})",
                    node.binding
                ),
            )
        })?;
    let labels = match &node.label {
        Some(label) => vec![label.clone()],
        None => Vec::new(),
    };
    let mut props_map: Map<String, Value> = Map::new();
    for (key, value) in node.properties.iter() {
        props_map.insert(key.clone(), value.clone());
    }
    Ok(NodeRecord::new(id, labels, Value::Object(props_map)))
}

fn edge_pattern_to_record(edge: &EdgePattern) -> Result<EdgeRecord, QuerySurfaceError> {
    let left_id = edge
        .left
        .properties
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            QuerySurfaceError::invalid(
                "missing_create_edge_endpoint",
                "CREATE edge requires `id` on left node",
            )
        })?;
    let right_id = edge
        .right
        .properties
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            QuerySurfaceError::invalid(
                "missing_create_edge_endpoint",
                "CREATE edge requires `id` on right node",
            )
        })?;
    let edge_id = match edge
        .left
        .properties
        .get("edge_id")
        .or_else(|| edge.right.properties.get("edge_id"))
        .and_then(Value::as_str)
    {
        Some(custom) => custom.to_string(),
        None => format!("{}-{}-{}", left_id, edge.edge_type, right_id),
    };
    Ok(EdgeRecord::new(
        edge_id,
        left_id,
        edge.edge_type.clone(),
        right_id,
        json!({}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::cypher::ast::{NodePattern, SetExpr, WriteClause};

    #[test]
    fn compile_create_node_emits_node_upsert() {
        let writes = vec![WriteClause::CreateNode {
            node: NodePattern {
                binding: "n".into(),
                label: Some("Doc".into()),
                properties: BTreeMap::from([("id".into(), serde_json::json!("a"))]),
            },
        }];
        let batch = compile_writes(&writes).unwrap();
        assert_eq!(batch.mutations.len(), 1);
        let GraphMutation::NodeUpsert(node) = &batch.mutations[0] else {
            panic!("expected NodeUpsert");
        };
        assert_eq!(node.labels, vec!["Doc".to_string()]);
        assert_eq!(node.properties, serde_json::json!({"id": "a"}));
    }

    #[test]
    fn compile_set_requires_runtime_resolution() {
        let writes = vec![WriteClause::Set {
            binding: "n".into(),
            key: "seen".into(),
            value: SetExpr::Literal(serde_json::json!(1)),
        }];
        let batch = compile_writes(&writes).unwrap();
        assert_eq!(batch.mutations.len(), 0);
    }
}
