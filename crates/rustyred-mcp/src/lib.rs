// `result_large_err`/`too_many_arguments`: mirrors rustyred-core; the shared
// error enum is rich and several adapter calls are parameter-heavy by design.
#![allow(clippy::result_large_err, clippy::too_many_arguments)]

use std::collections::HashMap;

use rustyred_core::{
    builtin_plugin_registry, checkout_graph_version, compile_graph_pack, diff_graph_snapshots,
    dispatch_operation, graph_version_log, merge_graph_snapshots, update_graph_ref, AlgorithmGraph,
    CodeKgManifest, Direction, EdgeRecord, EpistemicType, GraphCompileOptions, GraphCounts,
    GraphMergeOptions, GraphSnapshot, GraphStats, GraphStoreError, GraphStoreResult,
    GraphVersionRepository, HarnessInstantKg, HybridScoringConfig, InMemoryGraphStore, NeighborHit,
    NeighborQuery, NodeQuery, NodeRecord, OperationError, RedCoreGraphStore, SessionDelta,
    VectorDesignation, VerifyReport,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const JSONRPC_VERSION: &str = "2.0";
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

pub trait McpGraphBackend {
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>>;
    fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>>;
    fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>>;
    fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>>;
    fn stats(&self) -> GraphStoreResult<GraphStats>;
    fn verify(&self) -> GraphStoreResult<VerifyReport>;
    fn labels(&self) -> GraphStoreResult<Vec<String>>;
    fn edge_types(&self) -> GraphStoreResult<Vec<String>>;
    fn property_keys(&self) -> GraphStoreResult<Vec<String>>;
    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "list_edges is not supported by this MCP backend",
        ))
    }
    fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        let stats = self.stats()?;
        let nodes = self.query_nodes(NodeQuery {
            limit: Some(stats.nodes_total.max(1)),
            ..NodeQuery::default()
        })?;
        let edges = self.list_edges()?;
        Ok(GraphSnapshot {
            version: stats.version,
            nodes,
            edges,
        })
    }
    fn upsert_node(&mut self, _node: NodeRecord) -> GraphStoreResult<()> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "node bulk upsert is not supported by this MCP backend",
        ))
    }
    fn upsert_edge(&mut self, _edge: EdgeRecord) -> GraphStoreResult<()> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "edge bulk upsert is not supported by this MCP backend",
        ))
    }
    fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>>;
    fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()>;
    fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>>;
    fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>>;
    fn hybrid_scoring_config(&self) -> HybridScoringConfig {
        HybridScoringConfig::default()
    }
    fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.hybrid_search(
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            config.alpha,
        )
    }
    fn designate_fulltext_property(
        &mut self,
        _label: &str,
        _property: &str,
    ) -> GraphStoreResult<()> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "full-text designation is not supported by this MCP backend",
        ))
    }
    fn fulltext_search(
        &self,
        _label: Option<&str>,
        _property: &str,
        _query: &str,
        _k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "full-text search is not supported by this MCP backend",
        ))
    }
    fn designate_spatial_property(
        &mut self,
        _label: &str,
        _lat_property: &str,
        _lon_property: &str,
        _resolution: u8,
    ) -> GraphStoreResult<()> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "spatial designation is not supported by this MCP backend",
        ))
    }
    fn spatial_radius_search(
        &self,
        _label: &str,
        _lat_property: &str,
        _lon_property: &str,
        _lat: f64,
        _lon: f64,
        _radius_km: f64,
    ) -> GraphStoreResult<Vec<String>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "spatial radius search is not supported by this MCP backend",
        ))
    }
    fn spatial_bbox_search(
        &self,
        _label: &str,
        _lat_property: &str,
        _lon_property: &str,
        _min_lat: f64,
        _min_lon: f64,
        _max_lat: f64,
        _max_lon: f64,
    ) -> GraphStoreResult<Vec<String>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "spatial bbox search is not supported by this MCP backend",
        ))
    }
    fn designate_geometry_property(
        &mut self,
        _label: &str,
        _property: &str,
        _encoding: &str,
        _resolution: u8,
    ) -> GraphStoreResult<()> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "geometry designation is not supported by this MCP backend",
        ))
    }
    fn spatial_contains_point(
        &self,
        _label: &str,
        _property: &str,
        _lat: f64,
        _lon: f64,
    ) -> GraphStoreResult<Vec<String>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "geometry contains search is not supported by this MCP backend",
        ))
    }
    fn spatial_intersects_geometry(
        &self,
        _label: &str,
        _property: &str,
        _encoding: &str,
        _geometry: &Value,
    ) -> GraphStoreResult<Vec<String>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "geometry intersects search is not supported by this MCP backend",
        ))
    }
    fn spatial_within_geometry(
        &self,
        _label: &str,
        _property: &str,
        _encoding: &str,
        _geometry: &Value,
    ) -> GraphStoreResult<Vec<String>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "geometry within search is not supported by this MCP backend",
        ))
    }
    fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>>;

    /// Personalized PageRank. Default impl walks `list_edges()` to build the
    /// adjacency map then calls `rustyred_core::personalized_pagerank`.
    fn algo_ppr(
        &self,
        seeds: &HashMap<String, f64>,
        alpha: f64,
        epsilon: f64,
        max_pushes: usize,
    ) -> GraphStoreResult<HashMap<String, f64>> {
        let edges = self.list_edges()?;
        let mut adjacency: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for edge in edges.iter() {
            if edge.tombstone {
                continue;
            }
            adjacency
                .entry(edge.from_id.clone())
                .or_default()
                .push((edge.to_id.clone(), edge.effective_confidence()));
        }
        Ok(rustyred_core::personalized_pagerank(
            &adjacency, seeds, alpha, epsilon, max_pushes,
        ))
    }

    /// Connected components. Default impl uses `rustyred_core::connected_components`.
    fn algo_components(&self, directed: bool) -> GraphStoreResult<Vec<Vec<String>>> {
        let edges = self.list_edges()?;
        Ok(rustyred_core::connected_components(&edges, directed))
    }

    /// Power-iteration PageRank. Default impl uses `rustyred_core::pagerank`.
    fn algo_pagerank(
        &self,
        damping: f64,
        max_iter: usize,
        tolerance: f64,
    ) -> GraphStoreResult<HashMap<String, f64>> {
        let edges = self.list_edges()?;
        Ok(rustyred_core::pagerank(
            &edges, damping, max_iter, tolerance,
        ))
    }

    /// Community detection + modularity via label-propagation. Default impl
    /// uses `rustyred_core::label_propagation_communities` (the modern replacement
    /// for the now-deprecated `louvain_communities` re-export).
    fn algo_communities(&self) -> GraphStoreResult<(HashMap<String, u64>, f64)> {
        let edges = self.list_edges()?;
        Ok(rustyred_core::label_propagation_communities(&edges))
    }

    /// Bulk upsert NodeRecords. Default impl loops `upsert_node` per record;
    /// concrete impls that have a faster batch primitive can override.
    fn bulk_upsert_nodes(&mut self, records: Vec<NodeRecord>) -> GraphStoreResult<(usize, usize)> {
        let mut inserted = 0usize;
        let mut failed = 0usize;
        for record in records {
            match self.upsert_node(record) {
                Ok(_) => inserted += 1,
                Err(_) => failed += 1,
            }
        }
        Ok((inserted, failed))
    }

    /// Bulk upsert EdgeRecords. Default impl loops `upsert_edge` per record.
    fn bulk_upsert_edges(&mut self, records: Vec<EdgeRecord>) -> GraphStoreResult<(usize, usize)> {
        let mut inserted = 0usize;
        let mut failed = 0usize;
        for record in records {
            match self.upsert_edge(record) {
                Ok(_) => inserted += 1,
                Err(_) => failed += 1,
            }
        }
        Ok((inserted, failed))
    }
}

pub trait McpGraphProvider {
    type Backend: McpGraphBackend;

    fn backend_for_tenant(&self, tenant: &str) -> Result<Self::Backend, McpError>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpServerConfig {
    pub name: String,
    pub version: String,
    pub default_tenant: String,
    pub read_only: bool,
    pub allow_admin: bool,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: "rusty-red-graph-database".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_tenant: "default".to_string(),
            read_only: true,
            allow_admin: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpRequestContext {
    pub scopes: Vec<String>,
}

impl McpRequestContext {
    pub fn with_scopes(scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            scopes: scopes.into_iter().map(Into::into).collect(),
        }
    }

    fn allows(&self, required_scope: &str) -> bool {
        self.scopes.iter().any(|scope| {
            scope == "*"
                || scope == required_scope
                || mcp_scope_alias(scope.as_str()) == required_scope
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl McpError {
    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("MCP method {method} is not supported"),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }
}

impl From<GraphStoreError> for McpError {
    fn from(error: GraphStoreError) -> Self {
        Self {
            code: -32603,
            message: error.message,
            data: Some(json!({ "code": error.code })),
        }
    }
}

pub fn handle_mcp_request<P: McpGraphProvider>(
    provider: &P,
    config: &McpServerConfig,
    payload: Value,
) -> Value {
    handle_mcp_request_with_context(provider, config, &McpRequestContext::default(), payload)
}

pub fn handle_mcp_request_with_context<P: McpGraphProvider>(
    provider: &P,
    config: &McpServerConfig,
    context: &McpRequestContext,
    payload: Value,
) -> Value {
    let request = match serde_json::from_value::<JsonRpcRequest>(payload) {
        Ok(request) => request,
        Err(error) => {
            return jsonrpc_error(
                None,
                McpError::parse(format!("invalid JSON-RPC request: {error}")),
            )
        }
    };

    if request.jsonrpc.as_deref().unwrap_or(JSONRPC_VERSION) != JSONRPC_VERSION {
        return jsonrpc_error(
            request.id,
            McpError::invalid_request("jsonrpc must be \"2.0\""),
        );
    }

    match dispatch(provider, config, context, &request) {
        Ok(result) => json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": request.id,
            "result": result,
        }),
        Err(error) => jsonrpc_error(request.id, error),
    }
}

pub fn mcp_manifest(base_url: Option<&str>, config: &McpServerConfig) -> Value {
    let endpoint = base_url
        .map(|url| format!("{}/mcp", url.trim_end_matches('/')))
        .unwrap_or_else(|| "/mcp".to_string());
    json!({
        "name": config.name,
        "description": "MCP agent port for Rusty Red Graph Database. Exposes graph-native tools over RustyRed GraphStore APIs; raw Redis is never exposed.",
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "transport": {
            "type": "streamable-http",
            "endpoint": endpoint,
            "auth": "bearer"
        },
        "defaults": {
            "readOnly": config.read_only,
            "allowAdmin": config.allow_admin && !config.read_only,
            "rawRedis": false
        },
        "tools": tool_definitions(config),
        "resourceTemplates": resource_templates(),
        "prompts": prompt_definitions()
    })
}

pub fn agent_manifest(base_url: Option<&str>, config: &McpServerConfig) -> Value {
    json!({
        "name": "Rusty Red Graph Database Agent Port",
        "description": "Agent discovery for the RustyRed/Rusty Red first-class MCP endpoint.",
        "mcp": mcp_manifest(base_url, config),
        "wellKnown": {
            "mcp": "/.well-known/mcp/rustyred.json",
            "agent": "/.well-known/agent.json"
        }
    })
}

fn dispatch<P: McpGraphProvider>(
    provider: &P,
    config: &McpServerConfig,
    context: &McpRequestContext,
    request: &JsonRpcRequest,
) -> Result<Value, McpError> {
    match request.method.as_str() {
        "initialize" => Ok(initialize_result(config)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions(config) })),
        "tools/call" => call_tool(provider, config, context, &request.params),
        "resources/list" => Ok(json!({ "resources": resources(config) })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": resource_templates() })),
        "resources/read" => read_resource(provider, config, &request.params),
        "prompts/list" => Ok(json!({ "prompts": prompt_definitions() })),
        "prompts/get" => get_prompt(&request.params),
        method => Err(McpError::method_not_found(method)),
    }
}

fn initialize_result(config: &McpServerConfig) -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false },
            "prompts": { "listChanged": false }
        },
        "serverInfo": {
            "name": config.name,
            "version": config.version
        },
        "instructions": "Use graph-native RustyRed tools and resources. Raw Redis keys are not exposed. This first MCP slice is read-only unless the server explicitly enables admin tools."
    })
}

fn call_tool<P: McpGraphProvider>(
    provider: &P,
    config: &McpServerConfig,
    context: &McpRequestContext,
    params: &Value,
) -> Result<Value, McpError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_params("tools/call requires params.name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let tenant = tenant_from_args(&arguments, config);
    let mut backend = provider.backend_for_tenant(&tenant)?;

    let payload = match name {
        "rustyred.graph.neighbors" => {
            let query = neighbor_query_from_value(&arguments)?;
            let mut neighbors = backend.neighbors(query)?;
            let budget = Budget::from_args(&arguments);
            let truncated = apply_neighbor_budget(&mut neighbors, budget);
            json!({
                "tenant": tenant,
                "neighbors": neighbors,
                "stats": { "returned": neighbors.len(), "truncated": truncated }
            })
        }
        "rustyred.graph.schema" => schema_payload(&tenant, &backend)?,
        "rustyred.graph.index_status" => index_status_payload(&tenant, &backend)?,
        "rustyred.graph.explain" => explain_payload(&tenant, &arguments),
        "rustyred.graph.query" => query_payload(&tenant, &backend, &arguments)?,
        "rustyred.graph.version.compile" | "rustyred.git.compile" => {
            let snapshot = backend.graph_snapshot()?;
            let options = serde_json::from_value::<GraphCompileOptions>(arguments.clone())
                .map_err(|error| {
                    McpError::invalid_params(format!("invalid graph compile options: {error}"))
                })?;
            json!({
                "tenant": tenant,
                "pack": compile_graph_pack(&snapshot, options)
            })
        }
        "rustyred.graph.version.diff" | "rustyred.git.diff" => {
            let base = arguments.get("base").cloned().ok_or_else(|| {
                McpError::invalid_params("rustyred.graph.version.diff requires base snapshot")
            })?;
            let base = serde_json::from_value::<GraphSnapshot>(base).map_err(|error| {
                McpError::invalid_params(format!("base must be a graph snapshot: {error}"))
            })?;
            let target = match arguments.get("target").cloned() {
                Some(value) => serde_json::from_value::<GraphSnapshot>(value).map_err(|error| {
                    McpError::invalid_params(format!("target must be a graph snapshot: {error}"))
                })?,
                None => backend.graph_snapshot()?,
            };
            json!({
                "tenant": tenant,
                "diff": diff_graph_snapshots(&base, &target)
            })
        }
        "rustyred.graph.version.ref" | "rustyred.git.ref" => {
            let snapshot = backend.graph_snapshot()?;
            let options = serde_json::from_value::<GraphCompileOptions>(arguments.clone())
                .map_err(|error| {
                    McpError::invalid_params(format!("invalid graph compile options: {error}"))
                })?;
            let repository = optional_repository(&arguments)?;
            let branch = arguments
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_string);
            let updated_at_unix_ms = arguments.get("updated_at_unix_ms").and_then(Value::as_u64);
            let pack = compile_graph_pack(&snapshot, options);
            json!({
                "tenant": tenant,
                "ref_update": update_graph_ref(repository, pack, branch, updated_at_unix_ms.map(u128::from))
            })
        }
        "rustyred.graph.version.log" | "rustyred.git.log" => {
            let repository = required_repository(&arguments, name)?;
            let target = arguments.get("target").and_then(Value::as_str);
            json!({
                "tenant": tenant,
                "log": graph_version_log(&repository, target)
            })
        }
        "rustyred.graph.version.checkout" | "rustyred.git.checkout" => {
            let repository = required_repository(&arguments, name)?;
            let target = required_str(&arguments, "target", name)?;
            let checkout = checkout_graph_version(&repository, target).ok_or_else(|| {
                McpError::invalid_params(format!("target not found or has no payloads: {target}"))
            })?;
            json!({
                "tenant": tenant,
                "checkout": checkout
            })
        }
        "rustyred.graph.version.merge" | "rustyred.git.merge" => {
            let base = arguments.get("base").cloned().ok_or_else(|| {
                McpError::invalid_params("rustyred.graph.version.merge requires base snapshot")
            })?;
            let base = serde_json::from_value::<GraphSnapshot>(base).map_err(|error| {
                McpError::invalid_params(format!("base must be a graph snapshot: {error}"))
            })?;
            let ours = match arguments.get("ours").cloned() {
                Some(value) => serde_json::from_value::<GraphSnapshot>(value).map_err(|error| {
                    McpError::invalid_params(format!("ours must be a graph snapshot: {error}"))
                })?,
                None => backend.graph_snapshot()?,
            };
            let theirs = arguments.get("theirs").cloned().ok_or_else(|| {
                McpError::invalid_params("rustyred.graph.version.merge requires theirs snapshot")
            })?;
            let theirs = serde_json::from_value::<GraphSnapshot>(theirs).map_err(|error| {
                McpError::invalid_params(format!("theirs must be a graph snapshot: {error}"))
            })?;
            let options = serde_json::from_value::<GraphMergeOptions>(arguments.clone()).map_err(
                |error| McpError::invalid_params(format!("invalid graph merge options: {error}")),
            )?;
            json!({
                "tenant": tenant,
                "merge": merge_graph_snapshots(&base, &ours, &theirs, options)
            })
        }
        // §P6-B pb6.1: SPEC names `rustyred.algo.*` are aliases for the existing
        // `rustyred.algorithm.*` arms below. Either name reaches the same payload.
        "rustyred.algorithm.ppr" | "rustyred.algo.ppr" => {
            algorithm_ppr_payload(&tenant, &backend, &arguments)?
        }
        "rustyred.algorithm.components" | "rustyred.algo.components" => {
            algorithm_components_payload(&tenant, &backend, &arguments)?
        }
        "rustyred.algorithm.pagerank" | "rustyred.algo.pagerank" => {
            algorithm_pagerank_payload(&tenant, &backend, &arguments)?
        }
        "rustyred.algorithm.communities" | "rustyred.algo.communities" => {
            algorithm_communities_payload(&tenant, &backend)?
        }
        "rustyred.instant_kg.status" | "harness_kg_status" => {
            let view = instant_kg_view_payload(&tenant, &backend, &arguments)?;
            json!({
                "tenant": tenant,
                "status": view.status(),
                "stats": view.stats()
            })
        }
        "rustyred.instant_kg.ppr" | "harness_kg_ppr" => {
            let view = instant_kg_view_payload(&tenant, &backend, &arguments)?;
            let seeds: HashMap<String, f64> =
                serde_json::from_value(arguments.get("seeds").cloned().ok_or_else(|| {
                    McpError::invalid_params("harness_kg_ppr requires seeds object")
                })?)
                .map_err(|error| {
                    McpError::invalid_params(format!("seeds must be an object: {error}"))
                })?;
            let alpha = arguments
                .get("alpha")
                .and_then(Value::as_f64)
                .unwrap_or(0.15);
            let epsilon = arguments
                .get("epsilon")
                .and_then(Value::as_f64)
                .unwrap_or(1e-4);
            let max_pushes = arguments
                .get("max_pushes")
                .and_then(Value::as_u64)
                .unwrap_or(200_000) as usize;
            let top_k = arguments.get("top_k").and_then(Value::as_u64).unwrap_or(10) as usize;
            json!({
                "tenant": tenant,
                "status": view.status(),
                "results": view.ppr(&seeds, alpha, epsilon, max_pushes, top_k)
            })
        }
        "rustyred.instant_kg.impact" | "harness_kg_impact" => {
            let view = instant_kg_view_payload(&tenant, &backend, &arguments)?;
            let seed_arg = arguments
                .get("seed")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let symbol_arg = arguments
                .get("symbol_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let seed = if let Some(seed) = seed_arg {
                seed.to_string()
            } else if let Some(symbol_name) = symbol_arg {
                view.resolve_symbol_name(symbol_name).ok_or_else(|| {
                    McpError::invalid_params("harness_kg_impact could not resolve symbol_name")
                })?
            } else {
                return Err(McpError::invalid_params(
                    "harness_kg_impact requires seed or symbol_name",
                ));
            };
            let direction = instant_kg_direction(
                arguments
                    .get("direction")
                    .and_then(Value::as_str)
                    .unwrap_or("out"),
            );
            let max_depth = arguments
                .get("max_depth")
                .and_then(Value::as_u64)
                .unwrap_or(2) as usize;
            json!({
                "tenant": tenant,
                "seed": seed,
                "status": view.status(),
                "results": view.impact(&seed, direction, max_depth)
            })
        }
        "rustyred.instant_kg.related_objects" | "harness_kg_related_objects" => {
            let view = instant_kg_view_payload(&tenant, &backend, &arguments)?;
            let seed = required_str(&arguments, "seed", name)?;
            let kinds = string_array(&arguments, "kinds");
            let top_k = arguments.get("top_k").and_then(Value::as_u64).unwrap_or(10) as usize;
            json!({
                "tenant": tenant,
                "seed": seed,
                "status": view.status(),
                "results": view.related_objects(seed, &kinds, top_k)
            })
        }
        "rustyred.instant_kg.search" | "harness_kg_search" => {
            let view = instant_kg_view_payload(&tenant, &backend, &arguments)?;
            let query = required_str(&arguments, "query", name)?;
            let kinds = string_array(&arguments, "kinds");
            let top_k = arguments.get("top_k").and_then(Value::as_u64).unwrap_or(10) as usize;
            json!({
                "tenant": tenant,
                "query": query,
                "status": view.status(),
                "results": view.search(query, &kinds, top_k)
            })
        }
        "rustyred.instant_kg.explain_edge" | "harness_kg_explain_edge" => {
            let view = instant_kg_view_payload(&tenant, &backend, &arguments)?;
            let src = required_str(&arguments, "src", name)?;
            let dst = required_str(&arguments, "dst", name)?;
            json!({
                "tenant": tenant,
                "src": src,
                "dst": dst,
                "status": view.status(),
                "explanations": view.explain_edge(src, dst)
            })
        }
        "rustyred.fulltext.search" | "rustyred.graph.fulltext.search" => {
            let property = required_str(&arguments, "property", name)?;
            let query = required_str(&arguments, "query", name)?;
            let k = arguments.get("k").and_then(Value::as_u64).unwrap_or(10) as usize;
            let label = arguments.get("label").and_then(Value::as_str);
            let results = backend.fulltext_search(label, property, query, k)?;
            json!({
                "tenant": tenant,
                "results": results.iter().map(|(node_id, score)| json!({"node_id": node_id, "score": score})).collect::<Vec<_>>(),
                "stats": { "returned": results.len(), "k": k }
            })
        }
        "rustyred.fulltext.designate" | "rustyred.graph.fulltext.designate" => {
            if let Some(error) = require_write_tool(config, context, name) {
                return Ok(error);
            }
            let label = required_str(&arguments, "label", name)?;
            let property = required_str(&arguments, "property", name)?;
            backend.designate_fulltext_property(label, property)?;
            json!({
                "tenant": tenant,
                "designated": { "label": label, "property": property }
            })
        }
        "rustyred.spatial.radius" | "rustyred.graph.spatial.radius" => {
            let label = required_str(&arguments, "label", name)?;
            let lat_property = required_str(&arguments, "lat_property", name)?;
            let lon_property = required_str(&arguments, "lon_property", name)?;
            let lat = required_f64(&arguments, "lat", name)?;
            let lon = required_f64(&arguments, "lon", name)?;
            let radius_km = required_f64(&arguments, "radius_km", name)?;
            let node_ids = backend.spatial_radius_search(
                label,
                lat_property,
                lon_property,
                lat,
                lon,
                radius_km,
            )?;
            json!({
                "tenant": tenant,
                "node_ids": node_ids,
                "stats": { "returned": node_ids.len() }
            })
        }
        "rustyred.spatial.bbox" | "rustyred.graph.spatial.bbox" => {
            let label = required_str(&arguments, "label", name)?;
            let lat_property = required_str(&arguments, "lat_property", name)?;
            let lon_property = required_str(&arguments, "lon_property", name)?;
            let min_lat = required_f64(&arguments, "min_lat", name)?;
            let min_lon = required_f64(&arguments, "min_lon", name)?;
            let max_lat = required_f64(&arguments, "max_lat", name)?;
            let max_lon = required_f64(&arguments, "max_lon", name)?;
            let node_ids = backend.spatial_bbox_search(
                label,
                lat_property,
                lon_property,
                min_lat,
                min_lon,
                max_lat,
                max_lon,
            )?;
            json!({
                "tenant": tenant,
                "node_ids": node_ids,
                "stats": { "returned": node_ids.len() }
            })
        }
        "rustyred.spatial.designate" | "rustyred.graph.spatial.designate" => {
            if let Some(error) = require_write_tool(config, context, name) {
                return Ok(error);
            }
            let label = required_str(&arguments, "label", name)?;
            let lat_property = required_str(&arguments, "lat_property", name)?;
            let lon_property = required_str(&arguments, "lon_property", name)?;
            let resolution = arguments
                .get("resolution")
                .and_then(Value::as_u64)
                .unwrap_or(9)
                .min(u8::MAX as u64) as u8;
            backend.designate_spatial_property(label, lat_property, lon_property, resolution)?;
            json!({
                "tenant": tenant,
                "designated": {
                    "label": label,
                    "lat_property": lat_property,
                    "lon_property": lon_property,
                    "resolution": resolution
                }
            })
        }
        "rustyred.spatial.designate_geometry" | "rustyred.graph.spatial.designate_geometry" => {
            if let Some(error) = require_write_tool(config, context, name) {
                return Ok(error);
            }
            let label = required_str(&arguments, "label", name)?;
            let property = required_str(&arguments, "property", name)?;
            let encoding = arguments
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("wkb");
            let resolution = arguments
                .get("resolution")
                .and_then(Value::as_u64)
                .unwrap_or(9)
                .min(u8::MAX as u64) as u8;
            backend.designate_geometry_property(label, property, encoding, resolution)?;
            json!({
                "tenant": tenant,
                "designated": {
                    "label": label,
                    "property": property,
                    "encoding": encoding,
                    "resolution": resolution
                }
            })
        }
        "rustyred.spatial.contains" | "rustyred.graph.spatial.contains" => {
            let label = required_str(&arguments, "label", name)?;
            let property = required_str(&arguments, "property", name)?;
            let lat = required_f64(&arguments, "lat", name)?;
            let lon = required_f64(&arguments, "lon", name)?;
            let node_ids = backend.spatial_contains_point(label, property, lat, lon)?;
            json!({
                "tenant": tenant,
                "node_ids": node_ids,
                "stats": { "returned": node_ids.len() }
            })
        }
        "rustyred.spatial.intersects" | "rustyred.graph.spatial.intersects" => {
            let label = required_str(&arguments, "label", name)?;
            let property = required_str(&arguments, "property", name)?;
            let encoding = arguments
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("wkt");
            let geometry = arguments.get("geometry").ok_or_else(|| {
                McpError::invalid_params("rustyred.spatial.intersects requires geometry")
            })?;
            let node_ids =
                backend.spatial_intersects_geometry(label, property, encoding, geometry)?;
            json!({
                "tenant": tenant,
                "node_ids": node_ids,
                "stats": { "returned": node_ids.len() }
            })
        }
        "rustyred.spatial.within" | "rustyred.graph.spatial.within" => {
            let label = required_str(&arguments, "label", name)?;
            let property = required_str(&arguments, "property", name)?;
            let encoding = arguments
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("wkt");
            let geometry = arguments.get("geometry").ok_or_else(|| {
                McpError::invalid_params("rustyred.spatial.within requires geometry")
            })?;
            let node_ids = backend.spatial_within_geometry(label, property, encoding, geometry)?;
            json!({
                "tenant": tenant,
                "node_ids": node_ids,
                "stats": { "returned": node_ids.len() }
            })
        }
        "rustyred.bulk.nodes" | "rustyred.graph.bulk.nodes" => {
            if let Some(error) = require_write_tool(config, context, name) {
                return Ok(error);
            }
            let records = arguments
                .get("nodes")
                .or_else(|| arguments.get("records"))
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.bulk.nodes requires nodes array")
                })?;
            let mut inserted = 0usize;
            let mut errors = Vec::new();
            for (idx, raw) in records.iter().enumerate() {
                match parse_node_record(raw) {
                    Ok(node) => match backend.upsert_node(node.clone()) {
                        Ok(()) => inserted += 1,
                        Err(error) => errors.push(json!({
                            "line": idx + 1,
                            "code": error.code,
                            "message": error.message,
                            "record_id": node.id,
                        })),
                    },
                    Err(error) => errors.push(json!({
                        "line": idx + 1,
                        "code": "invalid_node_record",
                        "message": error.message,
                    })),
                }
            }
            json!({
                "tenant": tenant,
                "ok": errors.is_empty(),
                "inserted": inserted,
                "failed": errors.len(),
                "errors": errors,
            })
        }
        "rustyred.bulk.edges" | "rustyred.graph.bulk.edges" => {
            if let Some(error) = require_write_tool(config, context, name) {
                return Ok(error);
            }
            let records = arguments
                .get("edges")
                .or_else(|| arguments.get("records"))
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.bulk.edges requires edges array")
                })?;
            let mut inserted = 0usize;
            let mut errors = Vec::new();
            for (idx, raw) in records.iter().enumerate() {
                match parse_edge_record(raw) {
                    Ok(edge) => match backend.upsert_edge(edge.clone()) {
                        Ok(()) => inserted += 1,
                        Err(error) => errors.push(json!({
                            "line": idx + 1,
                            "code": error.code,
                            "message": error.message,
                            "record_id": edge.id,
                        })),
                    },
                    Err(error) => errors.push(json!({
                        "line": idx + 1,
                        "code": "invalid_edge_record",
                        "message": error.message,
                    })),
                }
            }
            json!({
                "tenant": tenant,
                "ok": errors.is_empty(),
                "inserted": inserted,
                "failed": errors.len(),
                "errors": errors,
            })
        }
        "rustyred.vector.search" => {
            let property = arguments
                .get("property")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.vector.search requires property")
                })?;
            let query = parse_f32_array(&arguments, "query")?;
            let k = arguments.get("k").and_then(Value::as_u64).unwrap_or(10) as usize;
            let label = arguments.get("label").and_then(Value::as_str);
            let results = backend.vector_search(label, property, &query, k)?;
            json!({
                "tenant": tenant,
                "results": results.iter().map(|(id, score)| json!({"node_id": id, "score": score})).collect::<Vec<_>>(),
                "stats": { "returned": results.len(), "k": k }
            })
        }
        "rustyred.vector.hybrid" => {
            let property = arguments
                .get("property")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.vector.hybrid requires property")
                })?;
            let query = parse_f32_array(&arguments, "query")?;
            let k = arguments.get("k").and_then(Value::as_u64).unwrap_or(10) as usize;
            let label = arguments.get("label").and_then(Value::as_str);
            let graph_seeds: Vec<String> = arguments
                .get("graph_seeds")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.vector.hybrid requires graph_seeds")
                })?;
            let max_hops = arguments
                .get("max_hops")
                .and_then(Value::as_u64)
                .unwrap_or(3) as usize;
            let alpha = arguments
                .get("alpha")
                .and_then(Value::as_f64)
                .map(|value| value as f32);
            let mut scoring = backend.hybrid_scoring_config();
            if let Some(alpha) = alpha {
                scoring = scoring.with_alpha(alpha);
            }
            if let Some(confidence_weighted) = arguments
                .get("confidence_weighted_graph_distance")
                .and_then(Value::as_bool)
            {
                scoring.confidence_weighted_graph_distance = confidence_weighted;
            }
            if let Some(weights) = arguments.get("edge_type_weights") {
                scoring.edge_type_weights =
                    serde_json::from_value(weights.clone()).map_err(|error| {
                        McpError::invalid_params(format!(
                            "edge_type_weights must be an object of number weights: {error}"
                        ))
                    })?;
            }
            let results = backend.hybrid_search_with_config(
                label,
                property,
                &query,
                k,
                &graph_seeds,
                max_hops,
                &scoring,
            )?;
            json!({
                "tenant": tenant,
                "results": results.iter().map(|(id, score)| json!({"node_id": id, "score": score})).collect::<Vec<_>>(),
                "stats": {
                    "returned": results.len(),
                    "k": k,
                    "alpha": scoring.alpha,
                    "max_hops": max_hops,
                    "confidence_weighted_graph_distance": scoring.confidence_weighted_graph_distance,
                    "edge_type_weights": scoring.edge_type_weights
                }
            })
        }
        "rustyred.vector.designate" => {
            if let Some(error) = require_write_tool(config, context, name) {
                return Ok(error);
            }
            let label = arguments
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.vector.designate requires label")
                })?;
            let property = arguments
                .get("property")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.vector.designate requires property")
                })?;
            let dimension = arguments
                .get("dimension")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.vector.designate requires dimension")
                })? as usize;
            backend.designate_vector_property(label, property, dimension)?;
            json!({
                "tenant": tenant,
                "designated": { "label": label, "property": property, "dimension": dimension }
            })
        }
        "rustyred.epistemic.neighbors" => {
            let node_id = arguments
                .get("node_id")
                .or_else(|| arguments.get("nodeId"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::invalid_params("rustyred.epistemic.neighbors requires node_id")
                })?;
            let epistemic_types: Option<Vec<EpistemicType>> = arguments
                .get("epistemic_types")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.parse::<EpistemicType>())
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(McpError::from)?;
            let min_confidence = arguments.get("min_confidence").and_then(Value::as_f64);
            let max_depth = arguments
                .get("max_depth")
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            let results = backend.epistemic_neighbors(
                node_id,
                epistemic_types.as_deref(),
                min_confidence,
                max_depth,
            )?;
            json!({
                "tenant": tenant,
                "node_id": node_id,
                "results": results.iter().map(|(edge, node)| json!({"edge": edge, "node": node})).collect::<Vec<_>>(),
                "stats": { "returned": results.len() }
            })
        }
        "rustyred.admin.verify" if config.read_only => {
            return Ok(tool_result_error(json!({
                "error": "mcp_read_only",
                "message": "admin MCP tools are unavailable while RUSTYRED_MCP_READ_ONLY/RUSTY_RED_MCP_READ_ONLY is true."
            })))
        }
        "rustyred.admin.verify" if !context.allows("admin:read") => {
            return Ok(tool_result_error(json!({
                "error": "admin_scope_required",
                "message": "rustyred.admin.verify requires admin:read or rustyred:graph:admin:verify scope."
            })))
        }
        "rustyred.admin.verify" if config.allow_admin => {
            json!({ "tenant": tenant, "verify": backend.verify()? })
        }
        "rustyred.admin.verify" => {
            return Ok(tool_result_error(json!({
                "error": "admin_tools_disabled",
                "message": "rustyred.admin.verify is hidden unless RUSTYRED_MCP_ALLOW_ADMIN/RUSTY_RED_MCP_ALLOW_ADMIN is true."
            })))
        }
        // Generated algorithm operations: any tool name registered through the
        // plugin registry is dispatched here with no per-tool wiring. Unknown
        // names fall through to method_not_found inside the helper.
        other => run_algorithm_operation_tool(other, &mut backend, &tenant, &arguments)?,
    };

    Ok(tool_result(payload))
}

/// Adapter exposing an MCP graph backend as the core [`AlgorithmGraph`] view, so
/// registered algorithm operations run over the tenant graph. A newtype is
/// required because a blanket impl would violate the orphan rule.
struct McpAlgorithmGraph<'a, B: McpGraphBackend>(&'a mut B);

impl<B: McpGraphBackend> AlgorithmGraph for McpAlgorithmGraph<'_, B> {
    fn graph_counts(&self) -> GraphStoreResult<GraphCounts> {
        let snapshot = self.0.graph_snapshot()?;
        Ok(GraphCounts {
            node_count: snapshot.nodes.iter().filter(|node| !node.tombstone).count(),
            relationship_count: snapshot.edges.iter().filter(|edge| !edge.tombstone).count(),
        })
    }
    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        Ok(self
            .0
            .list_edges()?
            .into_iter()
            .filter(|edge| !edge.tombstone)
            .collect())
    }
    fn nodes_with_label(&self, label: &str) -> GraphStoreResult<Vec<NodeRecord>> {
        self.0.query_nodes(NodeQuery::label(label))
    }
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        self.0.get_node(id)
    }
    fn vector_top_k(
        &self,
        label: &str,
        property: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        Ok(self
            .0
            .vector_search(Some(label), property, query, k)?
            .into_iter()
            .map(|(id, distance)| (id, 1.0 - distance))
            .collect())
    }
    fn write_node_property(&mut self, id: &str, key: &str, value: Value) -> GraphStoreResult<()> {
        let mut node = self.0.get_node(id)?.ok_or_else(|| {
            GraphStoreError::new("node_not_found", format!("no node with id {id}"))
        })?;
        rustyred_core::operation::set_property(&mut node.properties, key, value);
        self.0.upsert_node(node)
    }
    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        self.0.upsert_edge(edge)
    }
}

/// Run a registered algorithm operation over any MCP graph backend, honoring the
/// mode-plus-estimate contract. Returns `Ok(None)` when `command` is not a
/// registered algorithm operation. This is the single generation/dispatch entry
/// shared by the MCP tool surface and the HTTP route surface, so adding an
/// operation in `rustyred-core` surfaces it on both with no per-adapter edits.
pub fn run_algorithm_operation<B: McpGraphBackend>(
    command: &str,
    backend: &mut B,
    args: &Value,
) -> Result<Option<Value>, OperationError> {
    let registry = builtin_plugin_registry();
    let Some(operation) = registry.algorithm_operation(command) else {
        return Ok(None);
    };
    let operation = operation.clone();
    let mut adapter = McpAlgorithmGraph(backend);
    Ok(Some(dispatch_operation(
        operation.as_ref(),
        &mut adapter,
        args,
    )?))
}

/// Dispatch a registered algorithm operation as a generated MCP tool. Returns
/// `method_not_found` when the name is not a registered operation. The tenant id
/// is merged into the result payload to match the hand-wired algorithm tools.
fn run_algorithm_operation_tool<B: McpGraphBackend>(
    name: &str,
    backend: &mut B,
    tenant: &str,
    arguments: &Value,
) -> Result<Value, McpError> {
    match run_algorithm_operation(name, backend, arguments).map_err(operation_error_to_mcp)? {
        Some(mut payload) => {
            if let Some(object) = payload.as_object_mut() {
                object.insert("tenant".to_string(), json!(tenant));
            }
            Ok(payload)
        }
        None => Err(McpError::method_not_found(name)),
    }
}

fn operation_error_to_mcp(error: OperationError) -> McpError {
    match error.code.as_str() {
        "invalid_params" | "unsupported_mode" => McpError::invalid_params(error.message),
        _ => McpError::internal(format!("{}: {}", error.code, error.message)),
    }
}

fn read_resource<P: McpGraphProvider>(
    provider: &P,
    _config: &McpServerConfig,
    params: &Value,
) -> Result<Value, McpError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_params("resources/read requires params.uri"))?;
    let resource = ParsedResource::parse(uri)?;
    let backend = provider.backend_for_tenant(&resource.tenant)?;
    let payload = match resource.kind.as_str() {
        "schema" => schema_payload(&resource.tenant, &backend)?,
        "labels" => json!({ "tenant": resource.tenant, "labels": backend.labels()? }),
        "edge-types" => json!({ "tenant": resource.tenant, "edgeTypes": backend.edge_types()? }),
        "indexes" => index_status_payload(&resource.tenant, &backend)?,
        "stats" => json!({ "tenant": resource.tenant, "stats": backend.stats()? }),
        "verify" if resource.rest.as_deref() == Some("latest") => {
            json!({ "tenant": resource.tenant, "verify": backend.verify()? })
        }
        "node" => {
            let id = resource
                .rest
                .as_deref()
                .ok_or_else(|| McpError::invalid_params("node resource requires an id"))?;
            json!({ "tenant": resource.tenant, "node": backend.get_node(id)? })
        }
        "edge" => {
            let id = resource
                .rest
                .as_deref()
                .ok_or_else(|| McpError::invalid_params("edge resource requires an id"))?;
            json!({ "tenant": resource.tenant, "edge": backend.get_edge(id)? })
        }
        "neighbors" => {
            let id = resource
                .rest
                .as_deref()
                .ok_or_else(|| McpError::invalid_params("neighbors resource requires a node id"))?;
            json!({
                "tenant": resource.tenant,
                "node_id": id,
                "neighbors": backend.neighbors(NeighborQuery::out(id))?
            })
        }
        _ => {
            return Err(McpError::invalid_params(format!(
                "unsupported RustyRed resource URI {uri}"
            )))
        }
    };
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        }]
    }))
}

fn get_prompt(params: &Value) -> Result<Value, McpError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_params("prompts/get requires params.name"))?;
    let text = match name {
        "rustyred-query" => "Construct a bounded RustyRed graph query, then call rustyred.graph.explain before rustyred.graph.query. Keep max_depth and max_edges_touched explicit.",
        "rustyred-explain-plan" => "Explain a RustyRed graph query plan, naming the starting index, traversal direction, expected edge touches, and risk of fallback scans.",
        "rustyred-compile-context-pack" => "Use RustyRed schema, index status, and neighbor tools to compile a small context pack with reasons and hydrate URIs.",
        "rustyred-debug-indexes" => "Inspect rustyred.graph.index_status and rustyred.admin.verify output, then propose a safe rebuild or compaction follow-up without applying mutations.",
        other => return Err(McpError::method_not_found(other)),
    };
    Ok(json!({
        "description": prompt_description(name),
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": text }
        }]
    }))
}

fn schema_payload(tenant: &str, backend: &impl McpGraphBackend) -> Result<Value, McpError> {
    Ok(json!({
        "tenant": tenant,
        "labels": backend.labels()?,
        "edgeTypes": backend.edge_types()?,
        "propertyKeys": backend.property_keys()?,
        "stats": backend.stats()?,
        "propertyIndexes": "exact_scalar",
        "notes": [
            "This slice exposes label, edge-type, adjacency, and exact scalar property indexes.",
            "Full OpenCypher/GQL parsing and full-text indexes are still explicit follow-up work."
        ]
    }))
}

fn index_status_payload(tenant: &str, backend: &impl McpGraphBackend) -> Result<Value, McpError> {
    let verify = backend.verify()?;
    Ok(json!({
        "tenant": tenant,
        "healthy": verify.ok,
        "indexes": {
            "outAdjacency": "active",
            "inAdjacency": "active",
            "labels": "active",
            "edgeTypes": "active",
            "properties": "active_exact_scalar"
        },
        "stats": verify.stats,
        "problems": verify.problems
    }))
}

fn explain_payload(tenant: &str, arguments: &Value) -> Value {
    let operation = arguments
        .get("operation")
        .or_else(|| arguments.get("op"))
        .and_then(Value::as_str)
        .unwrap_or("neighbors");
    let query_step = match operation {
        "node_match" | "node_index_seek" => json!({
            "op": "node_index_seek",
            "cost": "O(label_set intersect property_set + returned_nodes)",
            "index": "label_index plus property_index",
            "bounded": true
        }),
        _ => json!({
            "op": "adjacency_lookup",
            "cost": "O(edge_types_for_node + returned_edges)",
            "index": "out_adjacency or in_adjacency",
            "bounded": true
        }),
    };
    json!({
        "tenant": tenant,
        "operation": operation,
        "plan": [{
            "op": "resolve_tenant_graph_store",
            "cost": "O(1)",
            "usesRawRedis": false
        }, query_step],
        "warnings": if matches!(operation, "neighbors" | "node_match" | "node_index_seek") {
            json!([])
        } else {
            json!(["Only neighbors and exact scalar node_match query execution are implemented in this slice."])
        }
    })
}

fn query_payload(
    tenant: &str,
    backend: &impl McpGraphBackend,
    arguments: &Value,
) -> Result<Value, McpError> {
    let operation = arguments
        .get("operation")
        .or_else(|| arguments.get("op"))
        .and_then(Value::as_str)
        .unwrap_or("neighbors");
    if matches!(operation, "node_match" | "node_index_seek") {
        let mut query = node_query_from_value(arguments)?;
        let budget = Budget::from_args(arguments);
        query.limit = Some(budget.max_nodes_returned.saturating_add(1));
        let mut nodes = backend.query_nodes(query)?;
        let truncated = nodes.len() > budget.max_nodes_returned;
        if truncated {
            nodes.truncate(budget.max_nodes_returned);
        }
        return Ok(json!({
            "tenant": tenant,
            "operation": "node_match",
            "nodes": nodes,
            "stats": { "returned": nodes.len(), "truncated": truncated },
            "explain": explain_payload(tenant, arguments)
        }));
    }
    if operation != "neighbors" {
        return Ok(json!({
            "tenant": tenant,
            "unsupported": operation,
            "supportedOperations": ["neighbors", "node_match"],
            "explain": explain_payload(tenant, arguments)
        }));
    }
    let query = neighbor_query_from_value(arguments)?;
    let mut neighbors = backend.neighbors(query)?;
    let budget = Budget::from_args(arguments);
    let truncated = apply_neighbor_budget(&mut neighbors, budget);
    Ok(json!({
        "tenant": tenant,
        "operation": "neighbors",
        "neighbors": neighbors,
        "stats": { "returned": neighbors.len(), "truncated": truncated },
        "explain": explain_payload(tenant, arguments)
    }))
}

fn algorithm_ppr_payload(
    tenant: &str,
    backend: &impl McpGraphBackend,
    arguments: &Value,
) -> Result<Value, McpError> {
    let edges = backend.list_edges()?;
    let seeds: HashMap<String, f64> =
        serde_json::from_value(arguments.get("seeds").cloned().ok_or_else(|| {
            McpError::invalid_params("rustyred.algorithm.ppr requires seeds object")
        })?)
        .map_err(|error| McpError::invalid_params(format!("seeds must be an object: {error}")))?;
    let alpha = arguments
        .get("alpha")
        .and_then(Value::as_f64)
        .unwrap_or(0.15);
    let epsilon = arguments
        .get("epsilon")
        .and_then(Value::as_f64)
        .unwrap_or(1e-4);
    let max_pushes = arguments
        .get("max_pushes")
        .and_then(Value::as_u64)
        .unwrap_or(200_000) as usize;
    let top_k = arguments
        .get("top_k")
        .and_then(Value::as_u64)
        .map(|k| k as usize);
    let mut adjacency: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    for edge in edges.iter().filter(|edge| !edge.tombstone) {
        adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .push((edge.to_id.clone(), edge.effective_confidence()));
    }
    let mut entries: Vec<(String, f64)> =
        rustyred_core::personalized_pagerank(&adjacency, &seeds, alpha, epsilon, max_pushes)
            .into_iter()
            .collect();
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(k) = top_k {
        entries.truncate(k);
    }
    Ok(json!({
        "tenant": tenant,
        "alpha": alpha,
        "epsilon": epsilon,
        "scores": entries.into_iter().map(|(node_id, score)| json!({
            "node_id": node_id,
            "score": score,
        })).collect::<Vec<_>>()
    }))
}

fn algorithm_components_payload(
    tenant: &str,
    backend: &impl McpGraphBackend,
    arguments: &Value,
) -> Result<Value, McpError> {
    let edges = backend.list_edges()?;
    let directed = arguments
        .get("directed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let components = rustyred_core::connected_components(&edges, directed);
    Ok(json!({
        "tenant": tenant,
        "directed": directed,
        "components": components,
        "count": components.len(),
    }))
}

fn algorithm_pagerank_payload(
    tenant: &str,
    backend: &impl McpGraphBackend,
    arguments: &Value,
) -> Result<Value, McpError> {
    let edges = backend.list_edges()?;
    let damping = arguments
        .get("damping")
        .and_then(Value::as_f64)
        .unwrap_or(0.85);
    let max_iter = arguments
        .get("max_iter")
        .and_then(Value::as_u64)
        .unwrap_or(100) as usize;
    let tolerance = arguments
        .get("tolerance")
        .and_then(Value::as_f64)
        .unwrap_or(1e-6);
    let top_k = arguments
        .get("top_k")
        .and_then(Value::as_u64)
        .map(|k| k as usize);
    let mut entries: Vec<(String, f64)> =
        rustyred_core::pagerank(&edges, damping, max_iter, tolerance)
            .into_iter()
            .collect();
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(k) = top_k {
        entries.truncate(k);
    }
    Ok(json!({
        "tenant": tenant,
        "damping": damping,
        "scores": entries.into_iter().map(|(node_id, score)| json!({
            "node_id": node_id,
            "score": score,
        })).collect::<Vec<_>>()
    }))
}

fn algorithm_communities_payload(
    tenant: &str,
    backend: &impl McpGraphBackend,
) -> Result<Value, McpError> {
    let edges = backend.list_edges()?;
    let (community, modularity) = rustyred_core::label_propagation_communities(&edges);
    let mut entries: Vec<Value> = community
        .into_iter()
        .map(|(node_id, community_id)| {
            json!({
                "node_id": node_id,
                "community_id": community_id,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        a["node_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["node_id"].as_str().unwrap_or(""))
    });
    Ok(json!({
        "tenant": tenant,
        "algorithm": "label_propagation",
        "communities": entries,
        "modularity": modularity,
    }))
}

fn instant_kg_view_payload(
    tenant: &str,
    backend: &impl McpGraphBackend,
    arguments: &Value,
) -> Result<HarnessInstantKg, McpError> {
    let base = backend.graph_snapshot()?;
    let manifest: Option<CodeKgManifest> = match arguments.get("manifest") {
        Some(value) => Some(serde_json::from_value(value.clone()).map_err(|error| {
            McpError::invalid_params(format!("manifest must match instant KG schema: {error}"))
        })?),
        None => None,
    };
    let delta: SessionDelta = match arguments.get("delta") {
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
            McpError::invalid_params(format!("delta must match instant KG schema: {error}"))
        })?,
        None => SessionDelta::default(),
    };
    let manifest = manifest.or_else(|| {
        Some(CodeKgManifest::from_base_snapshot(
            tenant,
            format!("v{}", base.version),
            &base,
        ))
    });
    Ok(HarnessInstantKg::new(base, manifest, delta))
}

fn instant_kg_direction(value: &str) -> Direction {
    if value.eq_ignore_ascii_case("in") || value.eq_ignore_ascii_case("incoming") {
        Direction::In
    } else {
        Direction::Out
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Budget {
    max_nodes_returned: usize,
}

impl Budget {
    fn from_args(arguments: &Value) -> Self {
        let max_nodes_returned = arguments
            .get("limit")
            .or_else(|| {
                arguments
                    .get("budget")
                    .and_then(|budget| budget.get("max_nodes_returned"))
            })
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(100);
        Self { max_nodes_returned }
    }
}

fn apply_neighbor_budget(neighbors: &mut Vec<NeighborHit>, budget: Budget) -> bool {
    let truncated = neighbors.len() > budget.max_nodes_returned;
    if truncated {
        neighbors.truncate(budget.max_nodes_returned);
    }
    truncated
}

fn node_query_from_value(value: &Value) -> Result<NodeQuery, McpError> {
    let label = value
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_string);
    let properties = value
        .get("properties")
        .or_else(|| value.get("props"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let properties = serde_json::from_value(properties)
        .map_err(|err| McpError::invalid_params(format!("properties must be an object: {err}")))?;
    Ok(NodeQuery {
        label,
        properties,
        limit: Some(Budget::from_args(value).max_nodes_returned),
    })
}

fn neighbor_query_from_value(value: &Value) -> Result<NeighborQuery, McpError> {
    let node_id = value
        .get("node_id")
        .or_else(|| value.get("nodeId"))
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_params("neighbor query requires node_id"))?;
    let direction = match value
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("out")
    {
        "out" | "outgoing" => Direction::Out,
        "in" | "incoming" => Direction::In,
        other => {
            return Err(McpError::invalid_params(format!(
                "direction must be out or in, got {other}"
            )))
        }
    };
    let edge_type = value
        .get("edge_type")
        .or_else(|| value.get("edgeType"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    Ok(NeighborQuery {
        node_id: node_id.to_string(),
        direction,
        edge_type,
    })
}

fn tenant_from_args(arguments: &Value, config: &McpServerConfig) -> String {
    arguments
        .get("tenant")
        .or_else(|| arguments.get("tenant_id"))
        .or_else(|| arguments.get("tenantId"))
        .and_then(Value::as_str)
        .filter(|tenant| !tenant.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| config.default_tenant.clone())
}

fn required_str<'a>(arguments: &'a Value, key: &str, tool_name: &str) -> Result<&'a str, McpError> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::invalid_params(format!("{tool_name} requires {key}")))
}

fn required_f64(arguments: &Value, key: &str, tool_name: &str) -> Result<f64, McpError> {
    arguments
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| McpError::invalid_params(format!("{tool_name} requires numeric {key}")))
}

fn string_array(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_f32_array(arguments: &Value, key: &str) -> Result<Vec<f32>, McpError> {
    arguments
        .get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|v| {
                    v.as_f64().map(|f| f as f32).ok_or_else(|| {
                        McpError::invalid_params(format!("{key} must be an array of numbers"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_else(|| {
            Err(McpError::invalid_params(format!(
                "{key} is required and must be an array of numbers"
            )))
        })
}

fn optional_repository(arguments: &Value) -> Result<GraphVersionRepository, McpError> {
    match arguments.get("repository").cloned() {
        Some(value) => serde_json::from_value(value)
            .map_err(|error| McpError::invalid_params(format!("repository is invalid: {error}"))),
        None => Ok(GraphVersionRepository::default()),
    }
}

fn required_repository(
    arguments: &Value,
    tool_name: &str,
) -> Result<GraphVersionRepository, McpError> {
    let value = arguments.get("repository").cloned().ok_or_else(|| {
        McpError::invalid_params(format!("{tool_name} requires repository object"))
    })?;
    serde_json::from_value(value)
        .map_err(|error| McpError::invalid_params(format!("repository is invalid: {error}")))
}

fn parse_node_record(raw: &Value) -> Result<NodeRecord, McpError> {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::invalid_params("node record requires string id"))?;
    let labels = raw
        .get("labels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let properties = raw.get("properties").cloned().unwrap_or_else(|| json!({}));
    if !properties.is_object() {
        return Err(McpError::invalid_params(
            "node properties must be an object",
        ));
    }
    let mut node = NodeRecord::new(id, labels, properties);
    node.tombstone = raw
        .get("tombstone")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(node)
}

fn parse_edge_record(raw: &Value) -> Result<EdgeRecord, McpError> {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::invalid_params("edge record requires string id"))?;
    let from_id = raw
        .get("from_id")
        .or_else(|| raw.get("fromId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::invalid_params("edge record requires string from_id"))?;
    let to_id = raw
        .get("to_id")
        .or_else(|| raw.get("toId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::invalid_params("edge record requires string to_id"))?;
    let edge_type = raw
        .get("type")
        .or_else(|| raw.get("edge_type"))
        .or_else(|| raw.get("edgeType"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::invalid_params("edge record requires string type"))?;
    let properties = raw.get("properties").cloned().unwrap_or_else(|| json!({}));
    if !properties.is_object() {
        return Err(McpError::invalid_params(
            "edge properties must be an object",
        ));
    }
    let mut edge = EdgeRecord::new(id, from_id, edge_type, to_id, properties);
    edge.tombstone = raw
        .get("tombstone")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    edge.confidence = raw.get("confidence").and_then(Value::as_f64);
    Ok(edge)
}

fn tool_result(payload: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        }],
        "structuredContent": payload
    })
}

fn tool_result_error(payload: Value) -> Value {
    let mut result = tool_result(payload);
    if let Value::Object(map) = &mut result {
        map.insert("isError".to_string(), Value::Bool(true));
    }
    result
}

fn require_write_tool(
    config: &McpServerConfig,
    context: &McpRequestContext,
    tool_name: &str,
) -> Option<Value> {
    if config.read_only {
        return Some(tool_result_error(json!({
            "error": "mcp_read_only",
            "message": "Write tools are unavailable while read-only mode is active."
        })));
    }
    if !context.allows("graph:write") {
        return Some(tool_result_error(json!({
            "error": "graph_write_scope_required",
            "message": format!("{tool_name} requires graph:write or rustyred:graph:write:propose/apply scope.")
        })));
    }
    None
}

fn jsonrpc_error(id: Option<Value>, error: McpError) -> Value {
    let mut body = json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": {
            "code": error.code,
            "message": error.message,
        }
    });
    if let Some(data) = error.data {
        body["error"]["data"] = data;
    }
    body
}

fn resources(config: &McpServerConfig) -> Vec<Value> {
    let tenant = &config.default_tenant;
    vec![
        resource(
            "schema",
            format!("rustyred://tenant/{tenant}/schema"),
            "RustyRed schema",
        ),
        resource(
            "labels",
            format!("rustyred://tenant/{tenant}/labels"),
            "RustyRed labels",
        ),
        resource(
            "edge-types",
            format!("rustyred://tenant/{tenant}/edge-types"),
            "RustyRed edge types",
        ),
        resource(
            "indexes",
            format!("rustyred://tenant/{tenant}/indexes"),
            "RustyRed index status",
        ),
        resource(
            "stats",
            format!("rustyred://tenant/{tenant}/stats"),
            "RustyRed graph stats",
        ),
        resource(
            "verify-latest",
            format!("rustyred://tenant/{tenant}/verify/latest"),
            "Latest RustyRed verify report",
        ),
    ]
}

fn resource(name: &str, uri: String, description: &str) -> Value {
    json!({
        "name": name,
        "uri": uri,
        "description": description,
        "mimeType": "application/json"
    })
}

fn resource_templates() -> Vec<Value> {
    vec![
        json!({
            "name": "node",
            "uriTemplate": "rustyred://tenant/{tenant}/node/{node_id}",
            "description": "Read a graph node by id.",
            "mimeType": "application/json"
        }),
        json!({
            "name": "edge",
            "uriTemplate": "rustyred://tenant/{tenant}/edge/{edge_id}",
            "description": "Read a graph edge by id.",
            "mimeType": "application/json"
        }),
        json!({
            "name": "neighbors",
            "uriTemplate": "rustyred://tenant/{tenant}/neighbors/{node_id}",
            "description": "Read outgoing neighbors for a node.",
            "mimeType": "application/json"
        }),
    ]
}

fn tool_definitions(config: &McpServerConfig) -> Vec<Value> {
    let mut tools = vec![
        tool(
            "rustyred.graph.neighbors",
            "Read graph neighbors through RustyRed adjacency indexes.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "node_id": { "type": "string" },
                    "direction": { "type": "string", "enum": ["out", "in"] },
                    "edge_type": { "type": "string" },
                    "budget": { "type": "object" }
                },
                "required": ["node_id"]
            }),
        ),
        tool(
            "rustyred.graph.query",
            "Run a bounded graph query. Supports adjacency neighbors and exact scalar node_match.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "operation": { "type": "string", "enum": ["neighbors", "node_match"] },
                    "node_id": { "type": "string" },
                    "direction": { "type": "string", "enum": ["out", "in"] },
                    "edge_type": { "type": "string" },
                    "label": { "type": "string" },
                    "properties": { "type": "object" },
                    "budget": { "type": "object" }
                }
            }),
        ),
        tool(
            "rustyred.graph.explain",
            "Explain the bounded RustyRed query plan without executing raw Redis.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "operation": { "type": "string" },
                    "node_id": { "type": "string" },
                    "direction": { "type": "string" },
                    "edge_type": { "type": "string" },
                    "label": { "type": "string" },
                    "properties": { "type": "object" },
                    "budget": { "type": "object" }
                }
            }),
        ),
        tool(
            "rustyred.graph.schema",
            "Read labels, edge types, stats, and current graph-store capability notes.",
            json!({
                "type": "object",
                "properties": { "tenant": { "type": "string" } }
            }),
        ),
        tool(
            "rustyred.graph.index_status",
            "Read index health and verify drift without exposing Redis keys.",
            json!({
                "type": "object",
                "properties": { "tenant": { "type": "string" } }
            }),
        ),
        tool(
            "rustyred.graph.version.compile",
            "Compile the tenant graph into a public content-addressed Prolly-tree pack with a Git-like commit.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "name": { "type": "string" },
                    "branch": { "type": "string", "default": "main" },
                    "parent_commits": { "type": "array", "items": { "type": "string" } },
                    "author": { "type": "string" },
                    "message": { "type": "string" },
                    "timestamp_unix_ms": { "type": "integer" },
                    "include_payloads": { "type": "boolean", "default": true }
                }
            }),
        ),
        tool(
            "rustyred.graph.version.diff",
            "Diff a base graph snapshot against the current tenant graph or a provided target snapshot using content hashes.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "base": { "type": "object" },
                    "target": { "type": "object" }
                },
                "required": ["base"]
            }),
        ),
        tool(
            "rustyred.graph.version.ref",
            "Compile the current tenant graph and update a branch ref inside a caller-supplied graph repository value.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "repository": { "type": "object" },
                    "name": { "type": "string" },
                    "branch": { "type": "string", "default": "main" },
                    "parent_commits": { "type": "array", "items": { "type": "string" } },
                    "author": { "type": "string" },
                    "message": { "type": "string" },
                    "timestamp_unix_ms": { "type": "integer" },
                    "updated_at_unix_ms": { "type": "integer" },
                    "include_payloads": { "type": "boolean", "default": true }
                }
            }),
        ),
        tool(
            "rustyred.graph.version.log",
            "Walk graph commit history from a branch name or commit hash in a caller-supplied graph repository.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "repository": { "type": "object" },
                    "target": { "type": "string", "default": "main" }
                },
                "required": ["repository"]
            }),
        ),
        tool(
            "rustyred.graph.version.checkout",
            "Reconstruct a graph snapshot from a branch name or commit hash in a caller-supplied graph repository.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "repository": { "type": "object" },
                    "target": { "type": "string" }
                },
                "required": ["repository", "target"]
            }),
        ),
        tool(
            "rustyred.graph.version.merge",
            "Three-way merge graph snapshots with content-hash conflict detection and confidence-weighted edge resolution.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "base": { "type": "object" },
                    "ours": { "type": "object" },
                    "theirs": { "type": "object" },
                    "strategy": {
                        "type": "string",
                        "enum": ["auto_confidence", "prefer_ours", "prefer_theirs", "manual"],
                        "default": "auto_confidence"
                    },
                    "min_confidence_delta": { "type": "number", "default": 0.0 },
                    "branch": { "type": "string" },
                    "author": { "type": "string" },
                    "message": { "type": "string" },
                    "timestamp_unix_ms": { "type": "integer" },
                    "include_payloads": { "type": "boolean", "default": true }
                },
                "required": ["base", "theirs"]
            }),
        ),
        tool(
            "rustyred.algorithm.ppr",
            "Run Personalized PageRank over the tenant graph.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "seeds": { "type": "object", "additionalProperties": { "type": "number" } },
                    "alpha": { "type": "number", "default": 0.15 },
                    "epsilon": { "type": "number", "default": 0.0001 },
                    "max_pushes": { "type": "integer", "default": 200000 },
                    "top_k": { "type": "integer" }
                },
                "required": ["seeds"]
            }),
        ),
        tool(
            "rustyred.algorithm.components",
            "Run connected-components over the tenant graph.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "directed": { "type": "boolean", "default": false }
                }
            }),
        ),
        tool(
            "rustyred.algorithm.pagerank",
            "Run global PageRank over the tenant graph.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "damping": { "type": "number", "default": 0.85 },
                    "max_iter": { "type": "integer", "default": 100 },
                    "tolerance": { "type": "number", "default": 0.000001 },
                    "top_k": { "type": "integer" }
                }
            }),
        ),
        tool(
            "rustyred.algorithm.communities",
            "Run label-propagation community detection over the tenant graph.",
            json!({
                "type": "object",
                "properties": { "tenant": { "type": "string" } }
            }),
        ),
        tool(
            "harness_kg_status",
            "Return Harness Instant KG merged-view status for the tenant base graph plus an optional session delta.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "manifest": { "type": "object" },
                    "delta": { "type": "object" }
                }
            }),
        ),
        tool(
            "harness_kg_ppr",
            "Run Personalized PageRank over the Harness Instant KG merged base+delta view.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "manifest": { "type": "object" },
                    "delta": { "type": "object" },
                    "seeds": { "type": "object", "additionalProperties": { "type": "number" } },
                    "alpha": { "type": "number", "default": 0.15 },
                    "epsilon": { "type": "number", "default": 0.0001 },
                    "max_pushes": { "type": "integer", "default": 200000 },
                    "top_k": { "type": "integer", "default": 10 }
                },
                "required": ["seeds"]
            }),
        ),
        tool(
            "harness_kg_impact",
            "Compute the blast radius from a code object in the Harness Instant KG merged view.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "manifest": { "type": "object" },
                    "delta": { "type": "object" },
                    "seed": { "type": "string" },
                    "symbol_name": { "type": "string" },
                    "direction": { "type": "string", "enum": ["out", "in"], "default": "out" },
                    "max_depth": { "type": "integer", "default": 2 }
                }
            }),
        ),
        tool(
            "harness_kg_related_objects",
            "Find code objects related to a seed in the Harness Instant KG merged view.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "manifest": { "type": "object" },
                    "delta": { "type": "object" },
                    "seed": { "type": "string" },
                    "kinds": { "type": "array", "items": { "type": "string" } },
                    "top_k": { "type": "integer", "default": 10 }
                },
                "required": ["seed"]
            }),
        ),
        tool(
            "harness_kg_search",
            "Run lexical code-object search over the Harness Instant KG merged view.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "manifest": { "type": "object" },
                    "delta": { "type": "object" },
                    "query": { "type": "string" },
                    "kinds": { "type": "array", "items": { "type": "string" } },
                    "top_k": { "type": "integer", "default": 10 }
                },
                "required": ["query"]
            }),
        ),
        tool(
            "harness_kg_explain_edge",
            "Explain why a merged Instant KG edge exists between two objects.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "manifest": { "type": "object" },
                    "delta": { "type": "object" },
                    "src": { "type": "string" },
                    "dst": { "type": "string" }
                },
                "required": ["src", "dst"]
            }),
        ),
    ];
    tools.push(tool(
        "rustyred.fulltext.search",
        "Search a designated full-text node property.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "label": { "type": "string" },
                "property": { "type": "string" },
                "query": { "type": "string" },
                "k": { "type": "integer", "default": 10 }
            },
            "required": ["property", "query"]
        }),
    ));
    tools.push(tool(
        "rustyred.spatial.radius",
        "Search a designated spatial property within a radius in kilometers.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "label": { "type": "string" },
                "lat_property": { "type": "string" },
                "lon_property": { "type": "string" },
                "lat": { "type": "number" },
                "lon": { "type": "number" },
                "radius_km": { "type": "number" }
            },
            "required": ["label", "lat_property", "lon_property", "lat", "lon", "radius_km"]
        }),
    ));
    tools.push(tool(
        "rustyred.spatial.bbox",
        "Search a designated spatial property within a bounding box.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "label": { "type": "string" },
                "lat_property": { "type": "string" },
                "lon_property": { "type": "string" },
                "min_lat": { "type": "number" },
                "min_lon": { "type": "number" },
                "max_lat": { "type": "number" },
                "max_lon": { "type": "number" }
            },
            "required": ["label", "lat_property", "lon_property", "min_lat", "min_lon", "max_lat", "max_lon"]
        }),
    ));
    tools.push(tool(
        "rustyred.spatial.contains",
        "Search designated geometry properties for polygons containing a point.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "label": { "type": "string" },
                "property": { "type": "string" },
                "lat": { "type": "number" },
                "lon": { "type": "number" }
            },
            "required": ["label", "property", "lat", "lon"]
        }),
    ));
    tools.push(tool(
        "rustyred.spatial.intersects",
        "Search designated geometry properties that intersect a query geometry.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "label": { "type": "string" },
                "property": { "type": "string" },
                "encoding": { "type": "string", "enum": ["wkb", "wkt"], "default": "wkt" },
                "geometry": {}
            },
            "required": ["label", "property", "geometry"]
        }),
    ));
    tools.push(tool(
        "rustyred.spatial.within",
        "Search designated geometry properties contained within a query geometry.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "label": { "type": "string" },
                "property": { "type": "string" },
                "encoding": { "type": "string", "enum": ["wkb", "wkt"], "default": "wkt" },
                "geometry": {}
            },
            "required": ["label", "property", "geometry"]
        }),
    ));
    tools.push(tool(
        "rustyred.vector.search",
        "Run a pure vector similarity search using HNSW indexes. Returns top-k nearest nodes.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "property": { "type": "string", "description": "Name of the vector property to search" },
                "query": { "type": "array", "items": { "type": "number" }, "description": "Query vector" },
                "k": { "type": "integer", "default": 10 },
                "label": { "type": "string", "description": "Optional label filter" }
            },
            "required": ["property", "query"]
        }),
    ));
    tools.push(tool(
        "rustyred.vector.hybrid",
        "Hybrid search blending vector similarity with graph proximity. Graph seeds anchor the graph-distance component.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "property": { "type": "string" },
                "query": { "type": "array", "items": { "type": "number" } },
                "k": { "type": "integer", "default": 10 },
                "label": { "type": "string" },
                "graph_seeds": { "type": "array", "items": { "type": "string" }, "description": "Node IDs to seed graph distance calculation" },
                "max_hops": { "type": "integer", "default": 3 },
                "alpha": { "type": "number", "default": 0.5, "description": "Blend weight: 0.0 = pure vector, 1.0 = pure graph" },
                "confidence_weighted_graph_distance": { "type": "boolean", "default": true },
                "edge_type_weights": { "type": "object", "additionalProperties": { "type": "number" } }
            },
            "required": ["property", "query", "graph_seeds"]
        }),
    ));
    tools.push(tool(
        "rustyred.epistemic.neighbors",
        "Traverse epistemic-typed edges (supports, contradicts, refines, etc.) with optional confidence filtering.",
        json!({
            "type": "object",
            "properties": {
                "tenant": { "type": "string" },
                "node_id": { "type": "string" },
                "epistemic_types": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["supports", "contradicts", "tension", "derives", "cites"] }
                },
                "min_confidence": { "type": "number" },
                "max_depth": { "type": "integer", "default": 1 }
            },
            "required": ["node_id"]
        }),
    ));
    if !config.read_only {
        tools.push(tool_write(
            "rustyred.fulltext.designate",
            "Designate a node property for full-text search.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "label": { "type": "string" },
                    "property": { "type": "string" }
                },
                "required": ["label", "property"]
            }),
        ));
        tools.push(tool_write(
            "rustyred.spatial.designate",
            "Designate latitude/longitude node properties for spatial search.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "label": { "type": "string" },
                    "lat_property": { "type": "string" },
                    "lon_property": { "type": "string" },
                    "resolution": { "type": "integer", "default": 9 }
                },
                "required": ["label", "lat_property", "lon_property"]
            }),
        ));
        tools.push(tool_write(
            "rustyred.spatial.designate_geometry",
            "Designate a node geometry property for geometry predicate search.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "label": { "type": "string" },
                    "property": { "type": "string" },
                    "encoding": { "type": "string", "enum": ["point", "wkb", "wkt", "subgraph"], "default": "wkb" },
                    "resolution": { "type": "integer", "default": 9 }
                },
                "required": ["label", "property"]
            }),
        ));
        tools.push(tool_write(
            "rustyred.bulk.nodes",
            "Bulk upsert node records from a JSON array.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "nodes": { "type": "array", "items": { "type": "object" } },
                    "records": { "type": "array", "items": { "type": "object" } }
                }
            }),
        ));
        tools.push(tool_write(
            "rustyred.bulk.edges",
            "Bulk upsert edge records from a JSON array.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "edges": { "type": "array", "items": { "type": "object" } },
                    "records": { "type": "array", "items": { "type": "object" } }
                }
            }),
        ));
        tools.push(tool_write(
            "rustyred.vector.designate",
            "Designate a node property as a vector field with a fixed dimension. Creates HNSW index for that property.",
            json!({
                "type": "object",
                "properties": {
                    "tenant": { "type": "string" },
                    "label": { "type": "string", "description": "Node label to attach the vector designation to" },
                    "property": { "type": "string", "description": "Property name that holds vector data" },
                    "dimension": { "type": "integer", "description": "Vector dimensionality" }
                },
                "required": ["label", "property", "dimension"]
            }),
        ));
    }
    if config.allow_admin && !config.read_only {
        tools.push(tool(
            "rustyred.admin.verify",
            "Run graph verification. Hidden unless admin MCP mode is enabled.",
            json!({
                "type": "object",
                "properties": { "tenant": { "type": "string" } }
            }),
        ));
    }
    // Generated algorithm-operation tools: one per registered operation, surfaced
    // straight from the plugin registry. The four legacy algorithm tools above
    // (pagerank, ppr, components, communities) keep their hand-wired descriptors,
    // so they are skipped here to avoid duplicate listings. Adding a new
    // algorithm operation in rustyred-core surfaces a new tool with no edit here.
    const LEGACY_ALGORITHM_TOOLS: [&str; 4] = [
        "rustyred.algorithm.pagerank",
        "rustyred.algorithm.ppr",
        "rustyred.algorithm.components",
        "rustyred.algorithm.communities",
    ];
    for operation in builtin_plugin_registry().algorithm_operations() {
        if LEGACY_ALGORITHM_TOOLS.contains(&operation.command()) {
            continue;
        }
        tools.push(tool(
            operation.command(),
            operation.summary(),
            operation.input_schema(),
        ));
    }
    tools
}

fn mcp_scope_alias(scope: &str) -> &str {
    match scope {
        "rustyred:graph:read" | "rustyred:graph:query" | "rustyred:graph:index:read" => {
            "graph:read"
        }
        "rustyred:graph:write:propose" | "rustyred:graph:write:apply" => "graph:write",
        "rustyred:graph:context" => "context:read",
        "rustyred:graph:admin:verify" => "admin:read",
        other => other,
    }
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "openWorldHint": false
        }
    })
}

fn tool_write(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "openWorldHint": false
        }
    })
}

fn prompt_definitions() -> Vec<Value> {
    [
        "rustyred-query",
        "rustyred-explain-plan",
        "rustyred-compile-context-pack",
        "rustyred-debug-indexes",
    ]
    .into_iter()
    .map(|name| {
        json!({
            "name": name,
            "title": name.replace('-', " "),
            "description": prompt_description(name),
            "arguments": []
        })
    })
    .collect()
}

fn prompt_description(name: &str) -> &'static str {
    match name {
        "rustyred-query" => "Guide an agent through a bounded RustyRed graph query.",
        "rustyred-explain-plan" => "Explain a RustyRed query plan and index usage.",
        "rustyred-compile-context-pack" => {
            "Compile a small graph-backed context pack from RustyRed reads."
        }
        "rustyred-debug-indexes" => "Inspect index health and suggest safe follow-up actions.",
        _ => "RustyRed MCP prompt",
    }
}

struct ParsedResource {
    tenant: String,
    kind: String,
    rest: Option<String>,
}

impl ParsedResource {
    fn parse(uri: &str) -> Result<Self, McpError> {
        let raw = uri.strip_prefix("rustyred://tenant/").ok_or_else(|| {
            McpError::invalid_params("RustyRed resource URI must start with rustyred://tenant/")
        })?;
        let mut parts = raw.splitn(3, '/');
        let tenant = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| McpError::invalid_params("RustyRed resource URI is missing tenant"))?;
        let kind = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                McpError::invalid_params("RustyRed resource URI is missing resource kind")
            })?;
        let rest = parts.next().map(str::to_string);
        Ok(Self {
            tenant: tenant.to_string(),
            kind: kind.to_string(),
            rest,
        })
    }
}

impl McpGraphBackend for InMemoryGraphStore {
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        Ok(InMemoryGraphStore::get_node(self, id).cloned())
    }

    fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        Ok(InMemoryGraphStore::get_edge(self, id).cloned())
    }

    fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        Ok(InMemoryGraphStore::query_nodes(self, query))
    }

    fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        Ok(InMemoryGraphStore::neighbors(self, query))
    }

    fn stats(&self) -> GraphStoreResult<GraphStats> {
        Ok(InMemoryGraphStore::stats(self))
    }

    fn verify(&self) -> GraphStoreResult<VerifyReport> {
        Ok(InMemoryGraphStore::verify(self))
    }

    fn labels(&self) -> GraphStoreResult<Vec<String>> {
        Ok(InMemoryGraphStore::labels(self))
    }

    fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        Ok(InMemoryGraphStore::edge_types(self))
    }

    fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        Ok(InMemoryGraphStore::property_keys(self))
    }

    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        Ok(self.snapshot().edges)
    }

    fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        Ok(self.snapshot())
    }

    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<()> {
        InMemoryGraphStore::upsert_node(self, node).map(|_| ())
    }

    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        InMemoryGraphStore::upsert_edge(self, edge).map(|_| ())
    }

    fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        Ok(InMemoryGraphStore::vector_designations(self))
    }

    fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        InMemoryGraphStore::designate_vector_property(self, label, property_name, dimension)
    }

    fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        InMemoryGraphStore::vector_search(self, label, property_name, query, k)
    }

    fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        InMemoryGraphStore::hybrid_search(
            self,
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            alpha,
        )
    }

    fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        InMemoryGraphStore::hybrid_search_with_config(
            self,
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            config,
        )
    }

    fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        Ok(InMemoryGraphStore::epistemic_neighbors(
            self,
            node_id,
            epistemic_types,
            min_confidence,
            max_depth,
        ))
    }
}

impl McpGraphBackend for RedCoreGraphStore {
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        RedCoreGraphStore::get_node(self, id)
    }

    fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        RedCoreGraphStore::get_edge(self, id)
    }

    fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        RedCoreGraphStore::query_nodes(self, query)
    }

    fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        RedCoreGraphStore::neighbors(self, query)
    }

    fn stats(&self) -> GraphStoreResult<GraphStats> {
        RedCoreGraphStore::stats(self)
    }

    fn verify(&self) -> GraphStoreResult<VerifyReport> {
        RedCoreGraphStore::verify(self)
    }

    fn labels(&self) -> GraphStoreResult<Vec<String>> {
        RedCoreGraphStore::labels(self)
    }

    fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        RedCoreGraphStore::edge_types(self)
    }

    fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        RedCoreGraphStore::property_keys(self)
    }

    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        Ok(self.graph_snapshot().edges)
    }

    fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        Ok(RedCoreGraphStore::graph_snapshot(self))
    }

    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<()> {
        RedCoreGraphStore::upsert_node(self, node).map(|_| ())
    }

    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        RedCoreGraphStore::upsert_edge(self, edge).map(|_| ())
    }

    fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        Ok(RedCoreGraphStore::vector_designations(self))
    }

    fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        RedCoreGraphStore::designate_vector_property(self, label, property_name, dimension)
    }

    fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        RedCoreGraphStore::vector_search(self, label, property_name, query, k)
    }

    fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        RedCoreGraphStore::hybrid_search(
            self,
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            alpha,
        )
    }

    fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        RedCoreGraphStore::hybrid_search_with_config(
            self,
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            config,
        )
    }

    fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        Ok(RedCoreGraphStore::epistemic_neighbors(
            self,
            node_id,
            epistemic_types,
            min_confidence,
            max_depth,
        ))
    }
}

#[cfg(feature = "redis-store")]
impl McpGraphBackend for rustyred_core::RedisGraphStore {
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        rustyred_core::RedisGraphStore::get_node(self, id)
    }

    fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        rustyred_core::RedisGraphStore::get_edge(self, id)
    }

    fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        rustyred_core::RedisGraphStore::query_nodes(self, query)
    }

    fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        rustyred_core::RedisGraphStore::neighbors(self, query)
    }

    fn stats(&self) -> GraphStoreResult<GraphStats> {
        rustyred_core::RedisGraphStore::stats(self)
    }

    fn verify(&self) -> GraphStoreResult<VerifyReport> {
        rustyred_core::RedisGraphStore::verify(self)
    }

    fn labels(&self) -> GraphStoreResult<Vec<String>> {
        rustyred_core::RedisGraphStore::labels(self)
    }

    fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        rustyred_core::RedisGraphStore::edge_types(self)
    }

    fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        rustyred_core::RedisGraphStore::property_keys(self)
    }

    fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        Err(GraphStoreError::new(
            "legacy_redis_instant_kg_unsupported",
            "instant KG requires the native RedCore graph store; RUSTY_RED_MODE=redis is a legacy compatibility path and should be changed to RUSTY_RED_MODE=embedded",
        ))
    }

    fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "Vector designations are not available on the Redis backend",
        ))
    }

    fn designate_vector_property(
        &mut self,
        _label: &str,
        _property_name: &str,
        _dimension: usize,
    ) -> GraphStoreResult<()> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "Vector designation is not available on the Redis backend",
        ))
    }

    fn vector_search(
        &self,
        _label: Option<&str>,
        _property_name: &str,
        _query: &[f32],
        _k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "Vector search is not available on the Redis backend",
        ))
    }

    fn hybrid_search(
        &self,
        _label: Option<&str>,
        _property_name: &str,
        _query: &[f32],
        _k: usize,
        _graph_seeds: &[String],
        _max_hops: usize,
        _alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "Hybrid search is not available on the Redis backend",
        ))
    }

    fn epistemic_neighbors(
        &self,
        _node_id: &str,
        _epistemic_types: Option<&[EpistemicType]>,
        _min_confidence: Option<f64>,
        _max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        Err(GraphStoreError::new(
            "unsupported_operation",
            "Epistemic neighbors are not available on the Redis backend",
        ))
    }
}

#[cfg(test)]
mod tests {
    use rustyred_core::{EdgeRecord, InMemoryGraphStore, NodeRecord};
    use serde_json::json;

    use super::{
        handle_mcp_request, handle_mcp_request_with_context, McpError, McpGraphProvider,
        McpRequestContext, McpServerConfig,
    };

    struct FixtureProvider(InMemoryGraphStore);

    impl McpGraphProvider for FixtureProvider {
        type Backend = InMemoryGraphStore;

        fn backend_for_tenant(&self, _tenant: &str) -> Result<Self::Backend, McpError> {
            Ok(self.0.clone())
        }
    }

    fn fixture() -> (FixtureProvider, McpServerConfig) {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["Person"],
                json!({"name": "Ada"}),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:b",
                ["Person", "Engineer"],
                json!({"name": "Grace"}),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:c",
                ["Person"],
                json!({"name": "Katherine"}),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:ab",
                "node:a",
                "KNOWS",
                "node:b",
                json!({"since": 1952}),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:ac",
                "node:a",
                "KNOWS",
                "node:c",
                json!({"since": 1962}),
            ))
            .unwrap();
        (
            FixtureProvider(store),
            McpServerConfig {
                default_tenant: "smoke".to_string(),
                ..McpServerConfig::default()
            },
        )
    }

    #[test]
    fn initialize_returns_mcp_capabilities() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        );

        assert_eq!(response["result"]["serverInfo"]["name"], config.name);
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_exposes_read_only_graph_tools() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        );

        let tools = response["result"]["tools"].as_array().unwrap();
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.graph.neighbors"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.algorithm.pagerank"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.fulltext.search"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.spatial.radius"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.spatial.contains"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.spatial.intersects"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.spatial.within"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.graph.version.compile"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.graph.version.diff"));
        assert!(!tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.admin.verify"));
        assert!(!tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.bulk.nodes"));
    }

    #[test]
    fn tool_call_reads_neighbors_from_graph_store() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "neighbors",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.neighbors",
                    "arguments": {
                        "tenant": "smoke",
                        "node_id": "node:a",
                        "direction": "out",
                        "edge_type": "KNOWS"
                    }
                }
            }),
        );

        assert_eq!(
            response["result"]["structuredContent"]["neighbors"][0]["node_id"],
            "node:b"
        );
    }

    #[test]
    fn tool_call_enforces_neighbor_budget() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "neighbors",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.neighbors",
                    "arguments": {
                        "tenant": "smoke",
                        "node_id": "node:a",
                        "direction": "out",
                        "budget": { "max_nodes_returned": 1 }
                    }
                }
            }),
        );

        assert_eq!(
            response["result"]["structuredContent"]["neighbors"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            response["result"]["structuredContent"]["stats"]["truncated"],
            true
        );
    }

    #[test]
    fn graph_query_supports_property_indexed_node_match() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "node-match",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.query",
                    "arguments": {
                        "tenant": "smoke",
                        "operation": "node_match",
                        "label": "Person",
                        "properties": { "name": "Grace" },
                        "budget": { "max_nodes_returned": 5 }
                    }
                }
            }),
        );

        assert_eq!(
            response["result"]["structuredContent"]["nodes"][0]["id"],
            "node:b"
        );
        assert_eq!(
            response["result"]["structuredContent"]["explain"]["plan"][1]["op"],
            "node_index_seek"
        );
    }

    #[test]
    fn version_tools_compile_and_diff_graph_snapshots() {
        let (provider, config) = fixture();
        let base = provider.0.snapshot();
        let compile = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "compile",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.version.compile",
                    "arguments": {
                        "tenant": "smoke",
                        "name": "public-redcore",
                        "timestamp_unix_ms": 1
                    }
                }
            }),
        );
        assert_eq!(
            compile["result"]["structuredContent"]["pack"]["protocol_version"],
            rustyred_core::VERSIONED_GRAPH_PROTOCOL_VERSION
        );
        assert_eq!(
            compile["result"]["structuredContent"]["pack"]["manifest"]["objects_total"],
            5
        );

        let mut changed = base.clone();
        changed.nodes.push(NodeRecord::new(
            "node:new",
            ["Person"],
            json!({"name": "Dorothy"}),
        ));
        let diff = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "diff",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.version.diff",
                    "arguments": {
                        "tenant": "smoke",
                        "base": base,
                        "target": changed
                    }
                }
            }),
        );
        assert_eq!(
            diff["result"]["structuredContent"]["diff"]["added"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let ref_update = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "ref",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.version.ref",
                    "arguments": {
                        "tenant": "smoke",
                        "branch": "main",
                        "timestamp_unix_ms": 1,
                        "updated_at_unix_ms": 2
                    }
                }
            }),
        );
        let repository =
            ref_update["result"]["structuredContent"]["ref_update"]["repository"].clone();
        assert_eq!(
            ref_update["result"]["structuredContent"]["ref_update"]["reference"]["name"],
            "main"
        );

        let log = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "log",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.version.log",
                    "arguments": {
                        "tenant": "smoke",
                        "repository": repository.clone(),
                        "target": "main"
                    }
                }
            }),
        );
        assert_eq!(
            log["result"]["structuredContent"]["log"]["commits"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let checkout = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "checkout",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.version.checkout",
                    "arguments": {
                        "tenant": "smoke",
                        "repository": repository,
                        "target": "main"
                    }
                }
            }),
        );
        assert_eq!(
            checkout["result"]["structuredContent"]["checkout"]["snapshot"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            base.nodes.len()
        );

        let merge = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "merge",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.graph.version.merge",
                    "arguments": {
                        "tenant": "smoke",
                        "base": base,
                        "theirs": changed,
                        "timestamp_unix_ms": 3
                    }
                }
            }),
        );
        assert_eq!(
            merge["result"]["structuredContent"]["merge"]["status"],
            "clean"
        );
    }

    #[test]
    fn algorithm_tool_calls_run_over_graph_edges() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "pagerank",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.algorithm.pagerank",
                    "arguments": { "tenant": "smoke", "top_k": 2 }
                }
            }),
        );

        assert_eq!(
            response["result"]["structuredContent"]["scores"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn generated_algorithm_tools_dispatch_and_list() {
        let (provider, config) = fixture();

        // tools/list surfaces the generated operations beside the legacy four.
        let listed = handle_mcp_request(
            &provider,
            &config,
            json!({ "jsonrpc": "2.0", "id": "list", "method": "tools/list", "params": {} }),
        );
        let names: Vec<String> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "rustyred.algorithm.leiden",
            "rustyred.algorithm.betweenness",
            "rustyred.algorithm.scc",
            "rustyred.algorithm.similarity_knn",
            "rustyred.algorithm.node_similarity",
            "rustyred.algorithm.link_prediction",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing generated tool {expected}"
            );
        }
        // The legacy pagerank tool stays present exactly once (no duplicate).
        assert_eq!(
            names
                .iter()
                .filter(|n| n.as_str() == "rustyred.algorithm.pagerank")
                .count(),
            1
        );

        // Leiden dispatches through the generic generated path.
        let leiden = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0", "id": "leiden", "method": "tools/call",
                "params": { "name": "rustyred.algorithm.leiden", "arguments": { "tenant": "smoke", "mode": "stream" } }
            }),
        );
        assert_eq!(
            leiden["result"]["structuredContent"]["operation"],
            "rustyred.algorithm.leiden"
        );
        assert!(
            leiden["result"]["structuredContent"]["community_count"]
                .as_u64()
                .unwrap()
                >= 1
        );

        // SCC reports the fixture is acyclic (a -> b, a -> c).
        let scc = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0", "id": "scc", "method": "tools/call",
                "params": { "name": "rustyred.algorithm.scc", "arguments": { "tenant": "smoke", "mode": "stream" } }
            }),
        );
        assert_eq!(scc["result"]["structuredContent"]["is_dag"], true);

        // The estimate companion works through a generated tool.
        let estimate = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0", "id": "est", "method": "tools/call",
                "params": { "name": "rustyred.algorithm.betweenness", "arguments": { "tenant": "smoke", "mode": "estimate" } }
            }),
        );
        assert!(
            estimate["result"]["structuredContent"]["estimate"]["bytes_min"]
                .as_u64()
                .unwrap()
                > 0
        );
    }

    #[test]
    fn instant_kg_tools_resolve_symbol_names_and_reject_bad_delta() {
        let (provider, config) = fixture();
        let impact = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "impact",
                "method": "tools/call",
                "params": {
                    "name": "harness_kg_impact",
                    "arguments": {
                        "tenant": "smoke",
                        "symbol_name": "Ada",
                        "direction": "out",
                        "max_depth": 1
                    }
                }
            }),
        );

        assert_eq!(impact["result"]["structuredContent"]["seed"], "node:a");
        let impacted_ids: Vec<_> = impact["result"]["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["object_id"].as_str())
            .collect();
        assert!(impacted_ids.contains(&"node:b"));

        let bad_delta = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "bad-delta",
                "method": "tools/call",
                "params": {
                    "name": "harness_kg_status",
                    "arguments": {
                        "tenant": "smoke",
                        "delta": { "objects": "not-an-array" }
                    }
                }
            }),
        );

        assert_eq!(bad_delta["error"]["code"], -32602);
        assert!(bad_delta["error"]["message"]
            .as_str()
            .unwrap()
            .contains("delta must match instant KG schema"));
    }

    #[test]
    fn read_write_tools_list_exposes_bulk_and_designation_tools() {
        let (provider, mut config) = fixture();
        config.read_only = false;
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        );

        let tools = response["result"]["tools"].as_array().unwrap();
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.bulk.nodes"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.fulltext.designate"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.spatial.designate"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "rustyred.spatial.designate_geometry"));
    }

    #[test]
    fn write_tools_require_graph_write_scope_even_when_exposed() {
        let (provider, mut config) = fixture();
        config.read_only = false;

        for tool_name in [
            "rustyred.fulltext.designate",
            "rustyred.spatial.designate",
            "rustyred.spatial.designate_geometry",
            "rustyred.bulk.nodes",
            "rustyred.bulk.edges",
            "rustyred.vector.designate",
        ] {
            let response = handle_mcp_request_with_context(
                &provider,
                &config,
                &McpRequestContext::with_scopes(["graph:read"]),
                json!({
                    "jsonrpc": "2.0",
                    "id": tool_name,
                    "method": "tools/call",
                    "params": {
                        "name": tool_name,
                        "arguments": { "tenant": "smoke" }
                    }
                }),
            );

            assert_eq!(
                response["result"]["structuredContent"]["error"],
                "graph_write_scope_required"
            );
        }

        let allowed = handle_mcp_request_with_context(
            &provider,
            &config,
            &McpRequestContext::with_scopes(["rustyred:graph:write:apply"]),
            json!({
                "jsonrpc": "2.0",
                "id": "bulk-nodes",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.bulk.nodes",
                    "arguments": {
                        "tenant": "smoke",
                        "nodes": [{ "id": "node:write", "labels": ["Person"] }]
                    }
                }
            }),
        );

        assert_eq!(allowed["result"]["structuredContent"]["ok"], true);
        assert_eq!(allowed["result"]["structuredContent"]["inserted"], 1);
    }

    #[test]
    fn read_only_mode_blocks_write_tools_before_scope_check() {
        let (provider, mut config) = fixture();
        config.read_only = true;

        let blocked = handle_mcp_request_with_context(
            &provider,
            &config,
            &McpRequestContext::with_scopes(["graph:write"]),
            json!({
                "jsonrpc": "2.0",
                "id": "bulk-nodes",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.bulk.nodes",
                    "arguments": {
                        "tenant": "smoke",
                        "nodes": [{ "id": "node:write", "labels": ["Person"] }]
                    }
                }
            }),
        );

        assert_eq!(
            blocked["result"]["structuredContent"]["error"],
            "mcp_read_only"
        );
    }

    #[test]
    fn admin_tool_requires_read_write_mcp_mode_and_admin_scope() {
        let (provider, mut config) = fixture();
        config.read_only = false;
        config.allow_admin = true;

        let no_admin = handle_mcp_request_with_context(
            &provider,
            &config,
            &McpRequestContext::with_scopes(["graph:read"]),
            json!({
                "jsonrpc": "2.0",
                "id": "verify",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.admin.verify",
                    "arguments": { "tenant": "smoke" }
                }
            }),
        );
        assert_eq!(
            no_admin["result"]["structuredContent"]["error"],
            "admin_scope_required"
        );

        let with_admin = handle_mcp_request_with_context(
            &provider,
            &config,
            &McpRequestContext::with_scopes(["rustyred:graph:admin:verify"]),
            json!({
                "jsonrpc": "2.0",
                "id": "verify",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.admin.verify",
                    "arguments": { "tenant": "smoke" }
                }
            }),
        );
        assert_eq!(
            with_admin["result"]["structuredContent"]["verify"]["ok"],
            true
        );
    }

    #[test]
    fn read_only_mode_hides_and_blocks_admin_tools() {
        let (provider, mut config) = fixture();
        config.read_only = true;
        config.allow_admin = true;

        let list = handle_mcp_request(
            &provider,
            &config,
            json!({"jsonrpc": "2.0", "id": "list", "method": "tools/list"}),
        );
        assert!(!list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "rustyred.admin.verify"));

        let blocked = handle_mcp_request_with_context(
            &provider,
            &config,
            &McpRequestContext::with_scopes(["admin:read"]),
            json!({
                "jsonrpc": "2.0",
                "id": "verify",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.admin.verify",
                    "arguments": { "tenant": "smoke" }
                }
            }),
        );
        assert_eq!(
            blocked["result"]["structuredContent"]["error"],
            "mcp_read_only"
        );
    }

    #[test]
    fn resources_read_supports_node_uri() {
        let (provider, config) = fixture();
        let response = handle_mcp_request(
            &provider,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "node",
                "method": "resources/read",
                "params": { "uri": "rustyred://tenant/smoke/node/node:a" }
            }),
        );

        let text = response["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"node:a\""));
    }

    // ---- §P6-B pb6.1 algo + bulk trait defaults --------------------------

    fn store_with_two_components() -> InMemoryGraphStore {
        // Two disconnected edges => two connected components: {a, b} and {c, d}.
        // `connected_components` ignores nodes that don't appear in any edge,
        // so a dangling node won't form its own component.
        let mut store = InMemoryGraphStore::default();
        store
            .upsert_node(NodeRecord::new("a", ["Doc"], json!({})))
            .unwrap();
        store
            .upsert_node(NodeRecord::new("b", ["Doc"], json!({})))
            .unwrap();
        store
            .upsert_node(NodeRecord::new("c", ["Doc"], json!({})))
            .unwrap();
        store
            .upsert_node(NodeRecord::new("d", ["Doc"], json!({})))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new("e1", "a", "T", "b", json!({})))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new("e2", "c", "T", "d", json!({})))
            .unwrap();
        store
    }

    #[test]
    fn backend_components_returns_partition() {
        use super::McpGraphBackend;
        let store = store_with_two_components();
        let components = store.algo_components(false).unwrap();
        // {a, b} and {c}
        assert_eq!(components.len(), 2);
    }

    #[test]
    fn backend_pagerank_returns_score_map() {
        use super::McpGraphBackend;
        let store = store_with_two_components();
        let ranks = store.algo_pagerank(0.85, 50, 1e-6).unwrap();
        assert!(ranks.contains_key("a"));
        assert!(ranks.contains_key("b"));
    }

    #[test]
    fn backend_bulk_upsert_nodes_counts_inserts() {
        use super::McpGraphBackend;
        let mut store = InMemoryGraphStore::default();
        let records = vec![
            NodeRecord::new("x", ["Doc"], json!({})),
            NodeRecord::new("y", ["Doc"], json!({})),
        ];
        let (inserted, failed) = store.bulk_upsert_nodes(records).unwrap();
        assert_eq!(inserted, 2);
        assert_eq!(failed, 0);
    }
}
