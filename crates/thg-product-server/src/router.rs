use axum::{
    extract::{Path, State},
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thg_core::commands::{ThgCommand, ThgRequest, ThgResponse};
use thg_core::errors::ThgError;
use thg_core::executor::{StoreBackedThgExecutor, ThgExecutor};
use thg_core::{
    stable_hash, EdgeRecord, EpistemicType, GraphStats, GraphStoreError, NeighborQuery, NodeQuery,
    NodeRecord,
};
use thg_mcp::{agent_manifest, handle_mcp_request_with_context, mcp_manifest, McpRequestContext};
use tower_http::cors::{Any, CorsLayer};

use crate::auth::require_scope;
use crate::graph_cache::{
    GraphCacheInvalidateBody, GraphCacheLookupBody, GraphCachePutBody, GraphCacheStatsBody,
};
use crate::query_surface::{
    execute_cypher_query, execute_public_query, explain_cypher_query, parse_tx_cypher_mutations,
    resolve_tenant_id, PublicCypherBody, QuerySurfaceError,
};
use crate::state::{AppState, StoreAccessError, TenantGraphStore};

#[derive(Debug, Deserialize)]
pub struct CommandBody {
    pub command: String,
    #[serde(default, alias = "payload")]
    pub args: Value,
}

#[derive(Debug, Deserialize)]
pub struct BatchBody {
    #[serde(default)]
    pub commands: Vec<CommandBody>,
}

#[derive(Debug, Deserialize)]
pub struct RootCommandBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    pub command: String,
    #[serde(default, alias = "payload")]
    pub args: Value,
}

#[derive(Debug, Deserialize)]
pub struct RootBatchBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub commands: Vec<CommandBody>,
}

#[derive(Debug, Deserialize)]
pub struct GraphQueryBody {
    pub query: String,
    #[serde(default)]
    pub graph: Value,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Deserialize)]
pub struct NodeWriteBody {
    pub id: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: Value,
    #[serde(default)]
    pub tombstone: bool,
}

impl NodeWriteBody {
    fn into_record(self) -> NodeRecord {
        let mut node = NodeRecord::new(self.id, self.labels, self.properties);
        node.tombstone = self.tombstone;
        node
    }
}

#[derive(Debug, Deserialize)]
pub struct EdgeWriteBody {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default)]
    pub properties: Value,
    #[serde(default)]
    pub tombstone: bool,
}

impl EdgeWriteBody {
    fn into_record(self) -> EdgeRecord {
        let mut edge = EdgeRecord::new(
            self.id,
            self.from_id,
            self.edge_type,
            self.to_id,
            self.properties,
        );
        edge.tombstone = self.tombstone;
        edge
    }
}

#[derive(Debug, Serialize)]
pub struct HealthBody {
    pub status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct VectorDesignateBody {
    pub label: String,
    pub property: String,
    pub dimension: usize,
}

#[derive(Debug, Deserialize)]
pub struct VectorSearchBody {
    pub query: Vec<f32>,
    #[serde(default = "default_k")]
    pub k: usize,
    pub label: Option<String>,
    pub property: String,
}

#[derive(Debug, Deserialize)]
pub struct HybridSearchBody {
    pub query: Vec<f32>,
    #[serde(default = "default_k")]
    pub k: usize,
    pub label: Option<String>,
    pub property: String,
    pub graph_seeds: Vec<String>,
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    #[serde(default = "default_alpha")]
    pub alpha: f32,
}

#[derive(Debug, Deserialize)]
pub struct EpistemicNeighborsBody {
    pub node_id: String,
    #[serde(default)]
    pub epistemic_types: Option<Vec<EpistemicType>>,
    #[serde(default)]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionBeginBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionMutationBody {
    pub tx_id: String,
    #[serde(default)]
    pub tenant_id: Option<String>,
}

fn default_k() -> usize {
    10
}
fn default_max_hops() -> usize {
    3
}
fn default_alpha() -> f32 {
    0.5
}

pub fn build_router(state: AppState) -> Router {
    let cors = cors_layer(&state);
    Router::new()
        .route("/health", get(health))
        .route("/health/", get(health))
        .route("/ready", get(ready))
        .route("/ready/", get(ready))
        .route("/openapi.json", get(crate::openapi::openapi))
        .route("/.well-known/mcp/thg.json", get(mcp_well_known))
        .route("/.well-known/agent.json", get(agent_well_known))
        .route("/mcp", post(mcp_post))
        .route("/metrics", get(crate::metrics::metrics))
        .route(
            "/v1/diagnostics/slow_queries",
            get(crate::metrics::slow_queries),
        )
        .route(
            "/v1/diagnostics/config",
            get(crate::metrics::diagnostics_config),
        )
        .route("/v1/command", post(root_command))
        .route("/v1/batch", post(root_batch))
        .route("/v1/query", post(public_query))
        .route("/v1/cypher", post(public_cypher))
        .route("/v1/cypher/explain", post(public_cypher_explain))
        .route("/v1/transactions/begin", post(transaction_begin))
        .route("/v1/transactions/commit", post(transaction_commit))
        .route("/v1/transactions/rollback", post(transaction_rollback))
        .route("/v1/cache/put", post(root_cache_put))
        .route("/v1/cache/get", post(root_cache_get))
        .route("/v1/cache/check", post(root_cache_check))
        .route("/v1/cache/explain", post(root_cache_explain))
        .route("/v1/cache/invalidate", post(root_cache_invalidate))
        .route("/v1/cache/stats", post(root_cache_stats))
        .route("/v1/tenants/:tenant_id/command", post(command))
        .route("/v1/tenants/:tenant_id/batch", post(batch))
        .route("/v1/tenants/:tenant_id/runs/:run_id", get(run_get))
        .route("/v1/tenants/:tenant_id/graph/query", post(graph_query))
        .route(
            "/v1/tenants/:tenant_id/graph/nodes",
            post(graph_node_upsert),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/nodes/query",
            post(graph_node_query),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/nodes/:node_id",
            get(graph_node_get),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/edges",
            post(graph_edge_upsert),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/edges/:edge_id",
            get(graph_edge_get),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/neighbors",
            post(graph_neighbors),
        )
        .route("/v1/tenants/:tenant_id/graph/stats", get(graph_stats))
        .route("/v1/tenants/:tenant_id/graph/verify", get(graph_verify))
        .route(
            "/v1/tenants/:tenant_id/graph/rebuild-indexes",
            post(graph_rebuild_indexes),
        )
        .route("/v1/tenants/:tenant_id/context/pack", post(context_pack))
        .route(
            "/v1/tenants/:tenant_id/graph/vector/designate",
            post(graph_vector_designate),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/vector/search",
            post(graph_vector_search),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/vector/hybrid",
            post(graph_vector_hybrid),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/epistemic-neighbors",
            post(graph_epistemic_neighbors),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/ppr",
            post(graph_algorithm_ppr),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/components",
            post(graph_algorithm_components),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/pagerank",
            post(graph_algorithm_pagerank),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/communities",
            post(graph_algorithm_communities),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/spatial/designate",
            post(graph_spatial_designate),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/spatial/radius",
            post(graph_spatial_radius),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/spatial/bbox",
            post(graph_spatial_bbox),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/fulltext/designate",
            post(graph_fulltext_designate),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/fulltext/search",
            post(graph_fulltext_search),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/bulk/nodes",
            post(graph_bulk_nodes),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/bulk/edges",
            post(graph_bulk_edges),
        )
        .layer(cors)
        .with_state(state)
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "ok" })
}

async fn mcp_well_known(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.mcp_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let config = state.mcp_config();
    Json(mcp_manifest(state.config.public_url.as_deref(), &config)).into_response()
}

async fn agent_well_known(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.mcp_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let config = state.mcp_config();
    Json(agent_manifest(state.config.public_url.as_deref(), &config)).into_response()
}

async fn mcp_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !state.config.mcp_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !mcp_origin_allowed(&headers, &state.config.allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let auth_context = match require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        Ok(context) => context,
        Err(status) => return status.into_response(),
    };

    let config = state.mcp_config();
    let mcp_context = McpRequestContext::with_scopes(auth_context.scopes);
    Json(handle_mcp_request_with_context(
        &state,
        &config,
        &mcp_context,
        payload,
    ))
    .into_response()
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.store_ready() {
        Ok(report) => Json(json!({
            "status": "ready",
            "store": report.store,
            "mode": report.mode,
            "durability": report.durability,
            "strict_acid": report.strict_acid,
            "require_volume": report.require_volume,
            "data_dir": report.data_dir
        }))
        .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "not_ready",
                "store": "unavailable",
                "mode": state.config.storage_mode.as_str(),
                "error": error.code,
                "message": error.message
            })),
        )
            .into_response(),
    }
}

async fn command(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandBody>,
) -> impl IntoResponse {
    let scope = required_scope_for_command(&body.command);
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        scope,
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(&state, &tenant_id, &body.command, body.args)
}

async fn root_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RootCommandBody>,
) -> impl IntoResponse {
    let scope = required_scope_for_command(&body.command);
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        scope,
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    execute_tenant_command(&state, &tenant_id, &body.command, body.args)
}

async fn batch(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BatchBody>,
) -> impl IntoResponse {
    for item in &body.commands {
        let scope = required_scope_for_command(&item.command);
        if let Err(status) = require_scope(
            &headers,
            &state.config.api_tokens,
            scope,
            state.config.require_auth,
        ) {
            return status.into_response();
        }
    }
    execute_batch_commands(&state, &tenant_id, body.commands)
}

async fn root_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RootBatchBody>,
) -> impl IntoResponse {
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    for item in &body.commands {
        let scope = required_scope_for_command(&item.command);
        if let Err(status) = require_scope(
            &headers,
            &state.config.api_tokens,
            scope,
            state.config.require_auth,
        ) {
            return status.into_response();
        }
    }
    execute_batch_commands(&state, &tenant_id, body.commands)
}

async fn root_cache_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCachePutBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_put(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheLookupBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_get(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheLookupBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_check(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_explain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheLookupBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_explain(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_invalidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheInvalidateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_invalidate(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheStatsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_stats(&state, &tenant_id) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn run_get(
    State(state): State<AppState>,
    Path((tenant_id, run_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "run:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(
        &state,
        &tenant_id,
        "THG.RUN.GET",
        json!({ "run_id": run_id }),
    )
}

async fn public_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id = match resolve_tenant_id(
        body.get("tenant_id").and_then(Value::as_str),
        &state.config.mcp_default_tenant,
    ) {
        Ok(tenant_id) => tenant_id,
        Err(error) => return query_surface_error_response(error),
    };
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match execute_public_query(&store, &tenant_id, &body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => query_surface_error_response(error),
    }
}

async fn public_cypher(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PublicCypherBody>,
) -> impl IntoResponse {
    let write_scope = body.tx_id.is_some();
    let scope = if write_scope {
        "graph:write"
    } else {
        "graph:read"
    };

    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        scope,
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    if let Some(tx_id) = body.tx_id.as_deref() {
        if tx_id.trim().is_empty() {
            return query_surface_error_response(QuerySurfaceError::invalid(
                "missing_tx_id",
                "tx_id is required when staging transactional Cypher statements",
            ));
        }
        let mutations = match parse_tx_cypher_mutations(&body.query, &body.params) {
            Ok(mutations) => mutations,
            Err(error) => return query_surface_error_response(error),
        };
        let staged_mutations =
            match state.append_graph_transaction_mutations(&tenant_id, tx_id, mutations) {
                Ok(staged_mutations) => staged_mutations,
                Err(error) => return graph_store_error_response(transaction_state_error(error)),
            };
        return Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "query": body.query,
            "tx_id": tx_id,
            "subset": "opencypher_v0_1_write_tx",
            "staged_mutations": staged_mutations,
        }))
        .into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    state.observability.record_cypher();
    let start = std::time::Instant::now();
    let outcome = execute_cypher_query(&store, &tenant_id, &body);
    let nanos = start.elapsed().as_nanos() as u64;
    state.observability.record_query_timing(
        "cypher",
        body.query.chars().take(120).collect::<String>().as_str(),
        nanos,
        0,
        0,
    );
    match outcome {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            state.observability.record_error();
            query_surface_error_response(error)
        }
    }
}

async fn transaction_begin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransactionBeginBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    let tx_id = match state.begin_graph_transaction(&tenant_id) {
        Ok(tx_id) => tx_id,
        Err(error) => {
            state.observability.record_error();
            return graph_store_error_response(transaction_state_error(error));
        }
    };
    state.observability.record_transaction_begin();
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "tx_id": tx_id,
    }))
    .into_response()
}

async fn transaction_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransactionMutationBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tx_id = body.tx_id.trim();
    if tx_id.is_empty() {
        return query_surface_error_response(QuerySurfaceError::invalid(
            "missing_tx_id",
            "tx_id is required for transaction commit",
        ));
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    let transaction = match state.commit_graph_transaction(&tenant_id, tx_id) {
        Ok(transaction) => transaction,
        Err(error) => {
            state.observability.record_error();
            return graph_store_error_response(transaction_state_error(error));
        }
    };
    state.observability.record_transaction_commit();
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "transaction": transaction,
    }))
    .into_response()
}

async fn transaction_rollback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransactionMutationBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tx_id = body.tx_id.trim();
    if tx_id.is_empty() {
        return query_surface_error_response(QuerySurfaceError::invalid(
            "missing_tx_id",
            "tx_id is required for transaction rollback",
        ));
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    if let Err(error) = state.rollback_graph_transaction(&tenant_id, tx_id) {
        state.observability.record_error();
        return graph_store_error_response(transaction_state_error(error));
    }
    state.observability.record_transaction_rollback();
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "tx_id": tx_id,
        "status": "rolled_back",
    }))
    .into_response()
}

async fn public_cypher_explain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PublicCypherBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match explain_cypher_query(&tenant_id, &body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => query_surface_error_response(error),
    }
}

async fn graph_query(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphQueryBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(
        &state,
        &tenant_id,
        "THG.DEBUG.CYPHER",
        json!({ "query": body.query, "graph": body.graph, "params": body.params }),
    )
}

async fn context_pack(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(args): Json<Value>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "context:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(&state, &tenant_id, "THG.CONTEXT.PACK", args)
}

async fn graph_vector_designate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<VectorDesignateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.designate_vector_property(&body.label, &body.property, body.dimension) {
        Ok(()) => Json(json!({
            "ok": true,
            "label": body.label,
            "property": body.property,
            "dimension": body.dimension
        }))
        .into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_vector_search(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<VectorSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };

    state.observability.record_vector_search();
    let label_ref = body.label.as_deref();
    match store.vector_search(label_ref, &body.property, &body.query, body.k) {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(node_id, distance)| {
                    let node = store.get_node(&node_id).ok().flatten();
                    json!({ "node_id": node_id, "distance": distance, "node": node })
                })
                .collect();
            Json(json!({ "ok": true, "results": items })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_vector_hybrid(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HybridSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };

    state.observability.record_vector_search();
    let label_ref = body.label.as_deref();
    match store.hybrid_search(
        label_ref,
        &body.property,
        &body.query,
        body.k,
        &body.graph_seeds,
        body.max_hops,
        body.alpha,
    ) {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(node_id, score)| {
                    let node = store.get_node(&node_id).ok().flatten();
                    json!({ "node_id": node_id, "score": score, "node": node })
                })
                .collect();
            Json(json!({ "ok": true, "results": items })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_epistemic_neighbors(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<EpistemicNeighborsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };

    let types_ref = body.epistemic_types.as_deref();
    match store.epistemic_neighbors(
        &body.node_id,
        types_ref,
        body.min_confidence,
        body.max_depth,
    ) {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(edge, node)| json!({ "edge": edge, "node": node }))
                .collect();
            Json(json!({ "ok": true, "results": items })).into_response()
        }
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_node_upsert(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NodeWriteBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    let record = body.into_record();
    let index_clone = record.clone();
    match store.upsert_node(record) {
        Ok(result) => {
            state.observability.record_mutation();
            state.maybe_index_node_spatially(&tenant_id, &index_clone);
            state.maybe_index_node_fulltext(&tenant_id, &index_clone);
            Json(json!({ "ok": true, "node": result })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_node_get(
    State(state): State<AppState>,
    Path((tenant_id, node_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.get_node(&node_id) {
        Ok(Some(node)) => Json(json!({ "ok": true, "node": node })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_node_query(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(query): Json<NodeQuery>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.query_nodes(query) {
        Ok(nodes) => Json(json!({ "ok": true, "nodes": nodes })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_edge_upsert(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<EdgeWriteBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.upsert_edge(body.into_record()) {
        Ok(result) => {
            state.observability.record_mutation();
            Json(json!({ "ok": true, "edge": result })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_edge_get(
    State(state): State<AppState>,
    Path((tenant_id, edge_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.get_edge(&edge_id) {
        Ok(Some(edge)) => Json(json!({ "ok": true, "edge": edge })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_neighbors(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(query): Json<NeighborQuery>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.neighbors(query) {
        Ok(neighbors) => Json(json!({ "ok": true, "neighbors": neighbors })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_stats(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.stats() {
        Ok(stats) => Json(json!({ "ok": true, "stats": stats })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_verify(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.verify() {
        Ok(report) => Json(json!({ "ok": report.ok, "verify": report })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_rebuild_indexes(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.rebuild_indexes() {
        Ok(report) => Json(json!({
            "ok": report.after.ok,
            "rebuild": report
        }))
        .into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

fn execute_tenant_command(
    state: &AppState,
    tenant_id: &str,
    command: &str,
    args: Value,
) -> axum::response::Response {
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    if is_graph_command(command) {
        return Json(execute_tenant_graph_command(
            state, tenant_id, command, args,
        ))
        .into_response();
    }
    if is_cache_command(command) {
        return Json(execute_tenant_cache_command(
            state, tenant_id, command, args,
        ))
        .into_response();
    }
    let store = match state.tenant_store(tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    let mut executor = StoreBackedThgExecutor::new(store);
    let response = executor.execute_request(ThgRequest::new(command, args));
    Json(response).into_response()
}

fn execute_tenant_graph_command(
    state: &AppState,
    tenant_id: &str,
    command_name: &str,
    args: Value,
) -> ThgResponse {
    if let Err(error) = state.store_ready() {
        return ThgResponse::err(
            command_name,
            ThgError::new(error.code, error.message),
            "graph:unavailable",
        );
    }
    let mut store = match state.tenant_graph_store(tenant_id) {
        Ok(store) => store,
        Err(error) => {
            return ThgResponse::err(
                command_name,
                ThgError::new(error.code, error.message),
                "graph:unavailable",
            )
        }
    };
    execute_graph_store_command(&mut store, command_name, args)
}

fn execute_tenant_cache_command(
    state: &AppState,
    tenant_id: &str,
    command_name: &str,
    args: Value,
) -> ThgResponse {
    let cache = match state.tenant_graph_cache(tenant_id) {
        Ok(cache) => cache,
        Err(error) => {
            return ThgResponse::err(
                command_name,
                ThgError::new(error.code, error.message),
                "graph:unavailable",
            )
        }
    };
    let graph_version = match current_graph_version(state, tenant_id) {
        Ok(version) => version,
        Err(error) => {
            return ThgResponse::err(
                command_name,
                ThgError::new(error.code, error.message),
                "graph:unavailable",
            )
        }
    };
    execute_graph_cache_command(&cache, command_name, args, graph_version)
}

fn execute_graph_store_command(
    store: &mut TenantGraphStore,
    command_name: &str,
    args: Value,
) -> ThgResponse {
    let command = match ThgCommand::from_name(command_name) {
        Ok(command) => command,
        Err(error) => return ThgResponse::err(command_name, error, "graph:unavailable"),
    };
    match command {
        ThgCommand::GraphNodeUpsert => {
            let node = match serde_json::from_value::<NodeWriteBody>(args) {
                Ok(body) => body.into_record(),
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            let response_node = thg_core::ThgNode {
                id: node.id.clone(),
                labels: node.labels.clone(),
                properties: node.properties.clone(),
            };
            match store.upsert_node(node) {
                Ok(write) => {
                    let mut response = ThgResponse::ok(
                        command.name(),
                        "ok",
                        json!({ "write": write, "node": response_node }),
                        graph_response_hash(store),
                    );
                    response.nodes.push(response_node);
                    response
                }
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        ThgCommand::GraphEdgeUpsert => {
            let edge = match serde_json::from_value::<EdgeWriteBody>(args) {
                Ok(body) => body.into_record(),
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            let response_edge = thg_core::ThgEdge {
                from_id: edge.from_id.clone(),
                edge_type: edge.edge_type.clone(),
                to_id: edge.to_id.clone(),
                properties: edge.properties.clone(),
            };
            match store.upsert_edge(edge) {
                Ok(write) => {
                    let mut response = ThgResponse::ok(
                        command.name(),
                        "ok",
                        json!({ "write": write, "edge": response_edge }),
                        graph_response_hash(store),
                    );
                    response.edges.push(response_edge);
                    response
                }
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        ThgCommand::GraphNodesQuery => {
            let query = match serde_json::from_value::<NodeQuery>(args) {
                Ok(query) => query,
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            let operation = if query.label.is_some() || !query.properties.is_empty() {
                "node_index_seek"
            } else {
                "node_scan"
            };
            match store.query_nodes(query) {
                Ok(hits) => {
                    let nodes = hits
                        .iter()
                        .map(|node| thg_core::ThgNode {
                            id: node.id.clone(),
                            labels: node.labels.clone(),
                            properties: node.properties.clone(),
                        })
                        .collect::<Vec<_>>();
                    let mut response = ThgResponse::ok(
                        command.name(),
                        "ok",
                        json!({
                            "nodes": hits,
                            "plan": { "operation": operation },
                            "stats": { "returned": nodes.len() },
                        }),
                        graph_response_hash(store),
                    );
                    response.nodes = nodes;
                    response
                }
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        ThgCommand::GraphNeighbors => {
            let query = match serde_json::from_value::<NeighborQuery>(args) {
                Ok(query) => query,
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            match store.neighbors(query) {
                Ok(hits) => ThgResponse::ok(
                    command.name(),
                    "ok",
                    json!({
                        "neighbors": hits,
                        "plan": { "operation": "adjacency_seek" },
                        "stats": { "returned": hits.len() },
                    }),
                    graph_response_hash(store),
                ),
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        ThgCommand::GraphStats => match store.stats() {
            Ok(stats) => ThgResponse::ok(
                command.name(),
                "ok",
                json!({ "stats": stats }),
                graph_stats_hash(&stats),
            ),
            Err(error) => graph_command_error(command.name(), error, store),
        },
        ThgCommand::GraphVerify => match store.verify() {
            Ok(report) => ThgResponse::ok(
                command.name(),
                if report.ok { "ok" } else { "drift_detected" },
                json!({ "report": report }),
                graph_response_hash(store),
            ),
            Err(error) => graph_command_error(command.name(), error, store),
        },
        ThgCommand::GraphRebuildIndexes => match store.rebuild_indexes() {
            Ok(report) => ThgResponse::ok(
                command.name(),
                if report.after.ok {
                    "ok"
                } else {
                    "canonical_graph_problem"
                },
                json!({ "report": report }),
                graph_response_hash(store),
            ),
            Err(error) => graph_command_error(command.name(), error, store),
        },
        _ => ThgResponse::err(
            command.name(),
            ThgError::unsupported_command(command.name()),
            graph_response_hash(store),
        ),
    }
}

fn is_graph_command(command: &str) -> bool {
    matches!(
        command.trim().to_ascii_uppercase().as_str(),
        "THG.GRAPH.NODE.UPSERT"
            | "THG.GRAPH.EDGE.UPSERT"
            | "THG.GRAPH.NODES.QUERY"
            | "THG.GRAPH.NEIGHBORS"
            | "THG.GRAPH.STATS"
            | "THG.GRAPH.VERIFY"
            | "THG.GRAPH.REBUILD_INDEXES"
            | "THG.GRAPH.REBUILD"
    )
}

fn is_cache_command(command: &str) -> bool {
    matches!(
        command.trim().to_ascii_uppercase().as_str(),
        "THG.CACHE.PUT"
            | "THG.CACHE.STORE"
            | "THG.CACHE.GET"
            | "THG.CACHE.CHECK"
            | "THG.CACHE.EXPLAIN"
            | "THG.CACHE.INVALIDATE"
            | "THG.CACHE.STATS"
    )
}

fn graph_command_invalid_params(
    command: &str,
    message: String,
    store: &TenantGraphStore,
) -> ThgResponse {
    ThgResponse::err(
        command,
        ThgError::new("invalid_graph_query", message),
        graph_response_hash(store),
    )
}

fn graph_command_error(
    command: &str,
    error: GraphStoreError,
    store: &TenantGraphStore,
) -> ThgResponse {
    ThgResponse::err(
        command,
        ThgError::new(error.code, error.message),
        graph_response_hash(store),
    )
}

fn execute_graph_cache_command(
    cache: &std::sync::Arc<crate::graph_cache::GraphCacheTenant>,
    command_name: &str,
    args: Value,
    graph_version: u64,
) -> ThgResponse {
    let upper = command_name.trim().to_ascii_uppercase();
    let result = match upper.as_str() {
        "THG.CACHE.PUT" | "THG.CACHE.STORE" => serde_json::from_value::<GraphCachePutBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.put(body, graph_version))
            .map(|payload| {
                ThgResponse::ok(
                    command_name,
                    "stored",
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "THG.CACHE.GET" => serde_json::from_value::<GraphCacheLookupBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.get(body, graph_version))
            .map(|payload| {
                ThgResponse::ok(
                    command_name,
                    if payload.accepted {
                        "hit"
                    } else {
                        payload.reason.as_str()
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "THG.CACHE.CHECK" => serde_json::from_value::<GraphCacheLookupBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.check(body, graph_version))
            .map(|payload| {
                ThgResponse::ok(
                    command_name,
                    if payload.accepted {
                        "hit"
                    } else {
                        payload.reason.as_str()
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "THG.CACHE.EXPLAIN" => serde_json::from_value::<GraphCacheLookupBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.explain(body, graph_version))
            .map(|payload| {
                ThgResponse::ok(
                    command_name,
                    if payload.accepted {
                        "explain_hit"
                    } else {
                        payload.reason.as_str()
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "THG.CACHE.INVALIDATE" => serde_json::from_value::<GraphCacheInvalidateBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.invalidate(body, graph_version))
            .map(|payload| {
                ThgResponse::ok(
                    command_name,
                    if payload.removed > 0 {
                        "invalidated"
                    } else {
                        "no_match"
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "THG.CACHE.STATS" => cache.stats(graph_version).map(|payload| {
            ThgResponse::ok(
                command_name,
                "ok",
                json!({ "cache": payload }),
                cache_state_hash(cache, graph_version),
            )
        }),
        _ => Err(GraphStoreError::new(
            "unsupported_graph_cache_command",
            format!("unsupported graph cache command: {command_name}"),
        )),
    };
    result.unwrap_or_else(|error| {
        ThgResponse::err(
            command_name,
            ThgError::new(error.code, error.message),
            cache_state_hash(cache, graph_version),
        )
    })
}

fn graph_response_hash(store: &TenantGraphStore) -> String {
    store
        .stats()
        .map(|stats| graph_stats_hash(&stats))
        .unwrap_or_else(|_| "graph:unavailable".to_string())
}

fn cache_state_hash(
    cache: &std::sync::Arc<crate::graph_cache::GraphCacheTenant>,
    graph_version: u64,
) -> String {
    cache
        .stats(graph_version)
        .map(|stats| stable_hash(stats))
        .unwrap_or_else(|_| format!("cache:unavailable:{graph_version}"))
}

fn graph_stats_hash(stats: &GraphStats) -> String {
    stable_hash(stats)
}

fn current_graph_version(state: &AppState, tenant_id: &str) -> Result<u64, GraphStoreError> {
    let store = state
        .tenant_graph_store(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    Ok(store.stats()?.version)
}

fn execute_cache_put(
    state: &AppState,
    tenant_id: &str,
    body: GraphCachePutBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.put(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_get(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheLookupBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.get(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_check(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheLookupBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.check(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_explain(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheLookupBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.explain(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_invalidate(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheInvalidateBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.invalidate(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_stats(state: &AppState, tenant_id: &str) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.stats(graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_batch_commands(
    state: &AppState,
    tenant_id: &str,
    commands: Vec<CommandBody>,
) -> axum::response::Response {
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let needs_state_store = commands
        .iter()
        .any(|item| !is_graph_command(&item.command) && !is_cache_command(&item.command));
    let mut executor = if needs_state_store {
        let store = match state.tenant_store(tenant_id) {
            Ok(store) => store,
            Err(error) => return store_unavailable_response(error),
        };
        Some(StoreBackedThgExecutor::new(store))
    } else {
        None
    };
    let mut graph_store: Option<TenantGraphStore> = None;
    let mut graph_cache = None;
    let mut results = Vec::with_capacity(commands.len());
    for item in commands {
        let command = item.command;
        let args = item.args;
        let response = if is_graph_command(&command) {
            if graph_store.is_none() {
                match state.tenant_graph_store(tenant_id) {
                    Ok(store) => graph_store = Some(store),
                    Err(error) => {
                        results.push(ThgResponse::err(
                            command,
                            ThgError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            }
            execute_graph_store_command(
                graph_store.as_mut().expect("graph store initialized"),
                &command,
                args,
            )
        } else if is_cache_command(&command) {
            if graph_cache.is_none() {
                match state.tenant_graph_cache(tenant_id) {
                    Ok(cache) => graph_cache = Some(cache),
                    Err(error) => {
                        results.push(ThgResponse::err(
                            command,
                            ThgError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            }
            let graph_version = if let Some(store) = graph_store.as_ref() {
                match store.stats() {
                    Ok(stats) => stats.version,
                    Err(error) => {
                        results.push(ThgResponse::err(
                            command,
                            ThgError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            } else {
                match current_graph_version(state, tenant_id) {
                    Ok(version) => version,
                    Err(error) => {
                        results.push(ThgResponse::err(
                            command,
                            ThgError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            };
            execute_graph_cache_command(
                graph_cache.as_ref().expect("graph cache initialized"),
                &command,
                args,
                graph_version,
            )
        } else {
            executor
                .as_mut()
                .expect("state executor initialized for non-graph command")
                .execute_request(ThgRequest::new(command, args))
        };
        results.push(response);
    }
    let state_hash = executor
        .as_ref()
        .map(|executor| executor.state().hash())
        .unwrap_or_else(|| {
            graph_store
                .as_ref()
                .map(graph_response_hash)
                .or_else(|| {
                    graph_cache.as_ref().map(|cache| {
                        cache_state_hash(
                            cache,
                            current_graph_version(state, tenant_id).unwrap_or(0),
                        )
                    })
                })
                .unwrap_or_else(|| "graph:empty_batch".to_string())
        });
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "results": results,
        "state_hash": state_hash
    }))
    .into_response()
}

fn query_surface_error_response(error: QuerySurfaceError) -> axum::response::Response {
    (error.status(), Json(error.payload())).into_response()
}

fn store_unavailable_response(error: StoreAccessError) -> axum::response::Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(error.as_payload())).into_response()
}

fn transaction_state_error(error: StoreAccessError) -> GraphStoreError {
    if error.code == "store_mode_unsupported" {
        GraphStoreError::new("unsupported_operation", error.message)
    } else {
        GraphStoreError::new(error.code, error.message)
    }
}

fn graph_store_error_response(error: GraphStoreError) -> axum::response::Response {
    (
        graph_error_status(error.code.as_str()),
        Json(json!({
            "error": error.code,
            "message": error.message
        })),
    )
        .into_response()
}

fn graph_error_status(code: &str) -> StatusCode {
    match code {
        "empty_graph_field"
        | "empty_graph_transaction"
        | "missing_graph_endpoint"
        | "tombstoned_graph_endpoint"
        | "invalid_graph_record"
        | "invalid_graph_cache_request"
        | "unsupported_graph_cache_kind"
        | "unsupported_graph_cache_command"
        | "dimension_mismatch"
        | "invalid_vector_designation"
        | "unsupported_operation" => StatusCode::BAD_REQUEST,
        "tenant_memory_quota_exceeded" => StatusCode::TOO_MANY_REQUESTS,
        "redis_graph_store_error"
        | "redcore_io_error"
        | "redcore_aof_frame_invalid"
        | "redcore_aof_checksum_mismatch"
        | "redcore_lock_poisoned"
        | "redcore_lock_unavailable"
        | "redcore_strict_mode_invalid"
        | "redcore_writer_lock_poisoned"
        | "redcore_snapshot_lock_poisoned"
        | "graph_cache_lock_poisoned" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn required_scope_for_command(command: &str) -> &'static str {
    match command.trim().to_ascii_uppercase().as_str() {
        "THG.RUN.GET" => "run:read",
        "THG.RUN.BEGIN" | "THG.RUN.STEP" => "run:write",
        "THG.CONTEXT.GET" => "context:read",
        "THG.CONTEXT.PACK" => "context:write",
        "THG.GRAPH.NODE.UPSERT" | "THG.GRAPH.EDGE.UPSERT" => "graph:write",
        "THG.STATE.HASH"
        | "THG.DEBUG.CYPHER"
        | "THG.CYPHER"
        | "THG.GRAPH.NODES.QUERY"
        | "THG.GRAPH.NEIGHBORS"
        | "THG.GRAPH.STATS"
        | "THG.GRAPH.VERIFY"
        | "THG.CACHE.GET"
        | "THG.CACHE.CHECK"
        | "THG.CACHE.EXPLAIN"
        | "THG.CACHE.STATS" => "graph:read",
        "THG.GRAPH.REBUILD_INDEXES" | "THG.GRAPH.REBUILD" => "graph:write",
        "THG.CACHE.PUT" | "THG.CACHE.STORE" | "THG.CACHE.INVALIDATE" => "graph:write",
        _ => "run:write",
    }
}

fn cors_layer(state: &AppState) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE]);
    if state
        .config
        .allowed_origins
        .iter()
        .any(|origin| origin == "*")
    {
        layer = layer.allow_origin(Any);
    } else {
        let origins = state
            .config
            .allowed_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect::<Vec<_>>();
        if !origins.is_empty() {
            layer = layer.allow_origin(origins);
        }
    }
    layer
}

fn mcp_origin_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return true;
    };
    allowed_origins.iter().any(|allowed| {
        allowed == "*" || allowed.trim_end_matches('/') == origin.trim_end_matches('/')
    })
}

// ===== Phase 6: Graph algorithm endpoints =====

#[derive(Debug, Deserialize)]
struct PprBody {
    seeds: std::collections::HashMap<String, f64>,
    #[serde(default = "default_ppr_alpha")]
    alpha: f64,
    #[serde(default = "default_ppr_epsilon")]
    epsilon: f64,
    #[serde(default = "default_ppr_max_pushes")]
    max_pushes: usize,
    #[serde(default)]
    top_k: Option<usize>,
}

fn default_ppr_alpha() -> f64 {
    0.15
}
fn default_ppr_epsilon() -> f64 {
    1e-4
}
fn default_ppr_max_pushes() -> usize {
    200_000
}

#[derive(Debug, Deserialize)]
struct ComponentsBody {
    #[serde(default)]
    directed: bool,
}

#[derive(Debug, Deserialize)]
struct PageRankBody {
    #[serde(default = "default_pr_damping")]
    damping: f64,
    #[serde(default = "default_pr_max_iter")]
    max_iter: usize,
    #[serde(default = "default_pr_tolerance")]
    tolerance: f64,
    #[serde(default)]
    top_k: Option<usize>,
}

fn default_pr_damping() -> f64 {
    0.85
}
fn default_pr_max_iter() -> usize {
    100
}
fn default_pr_tolerance() -> f64 {
    1e-6
}

#[derive(Debug, Deserialize, Default)]
struct CommunitiesBody {}

async fn graph_algorithm_ppr(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PprBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    let edges = match store.list_edges() {
        Ok(e) => e,
        Err(error) => return graph_store_error_response(error),
    };
    let mut adjacency: std::collections::HashMap<String, Vec<(String, f64)>> =
        std::collections::HashMap::new();
    for edge in edges.iter() {
        if edge.tombstone {
            continue;
        }
        adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .push((edge.to_id.clone(), edge.effective_confidence()));
    }
    state.observability.record_ppr();
    let scores = thg_core::personalized_pagerank(
        &adjacency,
        &body.seeds,
        body.alpha,
        body.epsilon,
        body.max_pushes,
    );
    let mut entries: Vec<(String, f64)> = scores.into_iter().collect();
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(k) = body.top_k {
        entries.truncate(k);
    }
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "alpha": body.alpha,
        "epsilon": body.epsilon,
        "scores": entries
            .into_iter()
            .map(|(node_id, score)| json!({ "node_id": node_id, "score": score }))
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn graph_algorithm_components(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ComponentsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    let edges = match store.list_edges() {
        Ok(e) => e,
        Err(error) => return graph_store_error_response(error),
    };
    state.observability.record_components();
    let components = thg_core::connected_components(&edges, body.directed);
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "directed": body.directed,
        "components": components,
        "count": components.len(),
    }))
    .into_response()
}

async fn graph_algorithm_pagerank(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PageRankBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    let edges = match store.list_edges() {
        Ok(e) => e,
        Err(error) => return graph_store_error_response(error),
    };
    state.observability.record_pagerank();
    let rank = thg_core::pagerank(&edges, body.damping, body.max_iter, body.tolerance);
    let mut entries: Vec<(String, f64)> = rank.into_iter().collect();
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(k) = body.top_k {
        entries.truncate(k);
    }
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "damping": body.damping,
        "scores": entries
            .into_iter()
            .map(|(node_id, score)| json!({ "node_id": node_id, "score": score }))
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

// ===== Phase 3: Bulk loader =====
//
// JSONL-only for now (one record per line). CSV with headers is straightforward
// to add later; JSONL keeps the contract simple and avoids ambiguity around
// quoting/escaping for nested properties.

#[derive(Debug, Deserialize)]
struct BulkNodesBody {
    /// JSONL: one node per line. Each line must be `{"id": "...", "labels": [...], "properties": {...}}`.
    jsonl: String,
    #[serde(default = "default_bulk_batch_size")]
    batch_size: usize,
}

#[derive(Debug, Deserialize)]
struct BulkEdgesBody {
    /// JSONL: one edge per line. Each line must be
    /// `{"id": "...", "from_id": "...", "to_id": "...", "edge_type": "...", "properties": {...}}`.
    jsonl: String,
    #[serde(default = "default_bulk_batch_size")]
    batch_size: usize,
}

fn default_bulk_batch_size() -> usize {
    500
}

async fn graph_bulk_nodes(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BulkNodesBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    let mut inserted = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<Value> = Vec::new();
    for (line_no, line) in body.jsonl.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Result<NodeRecord, _> = serde_json::from_str(trimmed);
        let node = match parsed {
            Ok(node) => node,
            Err(err) => {
                failed += 1;
                if errors.len() < 32 {
                    errors.push(json!({ "line": line_no + 1, "error": err.to_string() }));
                }
                continue;
            }
        };
        match store.upsert_node(node.clone()) {
            Ok(_) => {
                inserted += 1;
                state.observability.record_mutation();
                state.maybe_index_node_spatially(&tenant_id, &node);
                state.maybe_index_node_fulltext(&tenant_id, &node);
            }
            Err(err) => {
                failed += 1;
                if errors.len() < 32 {
                    errors.push(json!({ "line": line_no + 1, "error": format!("{err:?}") }));
                }
            }
        }
        // batch_size acts as a cooperative yielding boundary for future
        // async chunking; today every record is a separate write txn.
        let _ = body.batch_size;
    }
    Json(json!({
        "ok": failed == 0,
        "tenant": tenant_id,
        "inserted": inserted,
        "failed": failed,
        "errors": errors,
    }))
    .into_response()
}

async fn graph_bulk_edges(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BulkEdgesBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    let mut inserted = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<Value> = Vec::new();
    for (line_no, line) in body.jsonl.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Result<EdgeRecord, _> = serde_json::from_str(trimmed);
        let edge = match parsed {
            Ok(edge) => edge,
            Err(err) => {
                failed += 1;
                if errors.len() < 32 {
                    errors.push(json!({ "line": line_no + 1, "error": err.to_string() }));
                }
                continue;
            }
        };
        match store.upsert_edge(edge) {
            Ok(_) => {
                inserted += 1;
                state.observability.record_mutation();
            }
            Err(err) => {
                failed += 1;
                if errors.len() < 32 {
                    errors.push(json!({ "line": line_no + 1, "error": format!("{err:?}") }));
                }
            }
        }
        let _ = body.batch_size;
    }
    Json(json!({
        "ok": failed == 0,
        "tenant": tenant_id,
        "inserted": inserted,
        "failed": failed,
        "errors": errors,
    }))
    .into_response()
}

// ===== Phase 5: Full-text endpoints =====

#[derive(Debug, Deserialize)]
struct FullTextDesignateBody {
    label: String,
    property: String,
}

#[derive(Debug, Deserialize)]
struct FullTextSearchBody {
    #[serde(default)]
    label: Option<String>,
    property: String,
    query: String,
    #[serde(default = "default_fulltext_k")]
    k: usize,
}

fn default_fulltext_k() -> usize {
    10
}

async fn graph_fulltext_designate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<FullTextDesignateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    match state.designate_fulltext_property(&tenant_id, &body.label, &body.property) {
        Ok(()) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "label": body.label,
            "property": body.property,
        }))
        .into_response(),
        Err(error) => {
            state.observability.record_error();
            store_unavailable_response(error)
        }
    }
}

async fn graph_fulltext_search(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<FullTextSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    state.observability.record_fulltext_search();
    match state.fulltext_search(
        &tenant_id,
        body.label.as_deref(),
        &body.property,
        &body.query,
        body.k,
    ) {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(id, score)| json!({ "node_id": id, "score": score }))
                .collect();
            Json(json!({
                "ok": true,
                "tenant": tenant_id,
                "results": items,
            }))
            .into_response()
        }
        Err(error) => store_unavailable_response(error),
    }
}

// ===== Phase 8: Spatial endpoints =====

#[derive(Debug, Deserialize)]
struct SpatialDesignateBody {
    label: String,
    lat_property: String,
    lon_property: String,
    #[serde(default = "default_h3_resolution")]
    resolution: u8,
}

fn default_h3_resolution() -> u8 {
    8
}

#[derive(Debug, Deserialize)]
struct SpatialRadiusBody {
    label: String,
    lat_property: String,
    lon_property: String,
    lat: f64,
    lon: f64,
    radius_km: f64,
}

#[derive(Debug, Deserialize)]
struct SpatialBboxBody {
    label: String,
    lat_property: String,
    lon_property: String,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
}

async fn graph_spatial_designate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SpatialDesignateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    match state.designate_spatial_property(
        &tenant_id,
        &body.label,
        &body.lat_property,
        &body.lon_property,
        body.resolution,
    ) {
        Ok(()) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "label": body.label,
            "lat_property": body.lat_property,
            "lon_property": body.lon_property,
            "resolution": body.resolution,
        }))
        .into_response(),
        Err(error) => {
            state.observability.record_error();
            store_unavailable_response(error)
        }
    }
}

async fn graph_spatial_radius(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SpatialRadiusBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    state.observability.record_spatial_search();
    match state.spatial_radius_search(
        &tenant_id,
        &body.label,
        &body.lat_property,
        &body.lon_property,
        body.lat,
        body.lon,
        body.radius_km,
    ) {
        Ok(ids) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "count": ids.len(),
            "node_ids": ids,
        }))
        .into_response(),
        Err(error) => store_unavailable_response(error),
    }
}

async fn graph_spatial_bbox(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SpatialBboxBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    state.observability.record_spatial_search();
    match state.spatial_bbox_search(
        &tenant_id,
        &body.label,
        &body.lat_property,
        &body.lon_property,
        body.min_lat,
        body.min_lon,
        body.max_lat,
        body.max_lon,
    ) {
        Ok(ids) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "count": ids.len(),
            "node_ids": ids,
        }))
        .into_response(),
        Err(error) => store_unavailable_response(error),
    }
}

async fn graph_algorithm_communities(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(_body): Json<CommunitiesBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    let edges = match store.list_edges() {
        Ok(e) => e,
        Err(error) => return graph_store_error_response(error),
    };
    state.observability.record_communities();
    let (community, modularity) = thg_core::louvain_communities(&edges);
    let mut entries: Vec<Value> = community
        .into_iter()
        .map(|(node_id, c)| json!({ "node_id": node_id, "community_id": c }))
        .collect();
    entries.sort_by(|a, b| {
        a["node_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["node_id"].as_str().unwrap_or(""))
    });
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "communities": entries,
        "modularity": modularity,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::body::to_bytes;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::Json;
    use serde_json::{json, Value};

    use super::{
        execute_graph_store_command, execute_tenant_cache_command, execute_tenant_command,
        graph_error_status, is_cache_command, is_graph_command, mcp_origin_allowed, public_cypher,
        required_scope_for_command, transaction_begin, transaction_commit, transaction_rollback,
        PublicCypherBody, TransactionBeginBody, TransactionMutationBody,
    };
    use crate::{
        config::{Config, StorageMode},
        state::AppState,
    };
    use thg_core::RedCoreDurability;

    async fn response_payload_json(response: axum::response::Response) -> Value {
        serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    fn memory_product_state() -> AppState {
        AppState::new(Config {
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
        })
    }

    #[test]
    fn maps_core_commands_to_product_scopes() {
        assert_eq!(required_scope_for_command("THG.RUN.GET"), "run:read");
        assert_eq!(required_scope_for_command("THG.RUN.BEGIN"), "run:write");
        assert_eq!(
            required_scope_for_command("THG.CONTEXT.PACK"),
            "context:write"
        );
        assert_eq!(required_scope_for_command("THG.DEBUG.CYPHER"), "graph:read");
        assert_eq!(
            required_scope_for_command("THG.GRAPH.NODE.UPSERT"),
            "graph:write"
        );
        assert_eq!(
            required_scope_for_command("THG.GRAPH.EDGE.UPSERT"),
            "graph:write"
        );
        assert_eq!(
            required_scope_for_command("THG.GRAPH.NODES.QUERY"),
            "graph:read"
        );
        assert_eq!(required_scope_for_command("THG.GRAPH.STATS"), "graph:read");
        assert_eq!(required_scope_for_command("THG.GRAPH.VERIFY"), "graph:read");
        assert_eq!(
            required_scope_for_command("THG.GRAPH.REBUILD_INDEXES"),
            "graph:write"
        );
        assert_eq!(required_scope_for_command("THG.CACHE.CHECK"), "graph:read");
        assert_eq!(required_scope_for_command("THG.CACHE.PUT"), "graph:write");
    }

    #[test]
    fn detects_graph_commands_case_insensitively() {
        assert!(is_graph_command("thg.graph.node.upsert"));
        assert!(is_graph_command(" THG.GRAPH.NEIGHBORS "));
        assert!(is_graph_command("THG.GRAPH.VERIFY"));
        assert!(is_graph_command("thg.graph.rebuild_indexes"));
        assert!(!is_graph_command("THG.RUN.BEGIN"));
        assert!(is_cache_command("thg.cache.check"));
        assert!(is_cache_command(" THG.CACHE.PUT "));
        assert!(!is_cache_command("THG.RUN.BEGIN"));
    }

    #[test]
    fn graph_commands_share_store_unavailable_http_status() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Redis,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::AofEverysec,
            snapshot_interval_writes: 1_000,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
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

        let response = execute_tenant_command(&state, "tenant-a", "THG.GRAPH.STATS", json!({}));

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn graph_rebuild_command_returns_before_and_after_reports() {
        let state = memory_product_state();
        let mut store = state.tenant_graph_store("tenant-a").unwrap();

        let write = execute_graph_store_command(
            &mut store,
            "THG.GRAPH.NODE.UPSERT",
            json!({
                "id": "node:a",
                "labels": ["File"],
                "properties": { "path": "src/lib.rs" }
            }),
        );
        let rebuild =
            execute_graph_store_command(&mut store, "THG.GRAPH.REBUILD_INDEXES", json!({}));

        assert!(write.ok);
        assert!(rebuild.ok);
        assert_eq!(rebuild.status, "ok");
        assert_eq!(rebuild.payload["report"]["before"]["ok"], true);
        assert_eq!(rebuild.payload["report"]["after"]["ok"], true);
    }

    #[test]
    fn maps_graph_store_errors_to_http_statuses() {
        assert_eq!(
            graph_error_status("missing_graph_endpoint"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            graph_error_status("invalid_graph_cache_request"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            graph_error_status("redis_graph_store_error"),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            graph_error_status("tenant_memory_quota_exceeded"),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn cache_command_reports_stale_after_graph_write_advances_version() {
        let state = memory_product_state();

        let first_write = execute_tenant_command(
            &state,
            "tenant-a",
            "THG.GRAPH.NODE.UPSERT",
            json!({
                "id": "node:a",
                "labels": ["File"],
                "properties": { "path": "src/lib.rs" }
            }),
        );
        assert_eq!(first_write.status(), StatusCode::OK);

        let cache_put = execute_tenant_command(
            &state,
            "tenant-a",
            "THG.CACHE.PUT",
            json!({
                "kind": "query_result",
                "key": { "label": "File", "path": "src/lib.rs" },
                "value": { "nodes": ["node:a"] },
                "metadata": { "operation": "node_match" }
            }),
        );
        assert_eq!(cache_put.status(), StatusCode::OK);

        let second_write = execute_tenant_command(
            &state,
            "tenant-a",
            "THG.GRAPH.NODE.UPSERT",
            json!({
                "id": "node:b",
                "labels": ["File"],
                "properties": { "path": "src/main.rs" }
            }),
        );
        assert_eq!(second_write.status(), StatusCode::OK);

        let cache_check = execute_tenant_cache_command(
            &state,
            "tenant-a",
            "THG.CACHE.CHECK",
            json!({
                "kind": "query_result",
                "key": { "label": "File", "path": "src/lib.rs" }
            }),
        );
        assert!(cache_check.ok);
        assert_eq!(cache_check.status, "graph_version_mismatch");
        assert_eq!(cache_check.payload["cache"]["stale"], true);
        assert_eq!(cache_check.payload["cache"]["accepted"], false);
    }

    #[test]
    fn mcp_origin_check_allows_absent_or_configured_origin() {
        let allowed = vec!["https://app.example.com".to_string()];
        assert!(mcp_origin_allowed(&HeaderMap::new(), &allowed));

        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("https://app.example.com"),
        );
        assert!(mcp_origin_allowed(&headers, &allowed));

        headers.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(!mcp_origin_allowed(&headers, &allowed));
    }

    #[tokio::test]
    async fn transaction_routes_support_begin_stage_and_commit() {
        let state = memory_product_state();
        let begin_response = transaction_begin(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionBeginBody {
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(begin_response.status(), StatusCode::OK);

        let begin_payload = response_payload_json(begin_response).await;
        let tx_id = begin_payload["tx_id"]
            .as_str()
            .expect("transaction id in begin response");

        let stage_response = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-tx".to_string()),
                query: "CREATE (n:File {id: $id, path: $path})".to_string(),
                params: BTreeMap::from([
                    ("id".to_string(), json!("node:tx-commit")),
                    ("path".to_string(), json!("src/main.rs")),
                ]),
                tx_id: Some(tx_id.to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(stage_response.status(), StatusCode::OK);

        let stage_payload = response_payload_json(stage_response).await;
        assert_eq!(stage_payload["ok"], true);
        assert_eq!(stage_payload["staged_mutations"], 1);
        assert_eq!(stage_payload["tx_id"], tx_id);

        let commit_response = transaction_commit(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionMutationBody {
                tx_id: tx_id.to_string(),
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(commit_response.status(), StatusCode::OK);

        let commit_payload = response_payload_json(commit_response).await;
        assert_eq!(commit_payload["ok"], true);
        assert_eq!(commit_payload["tenant"], "tenant-tx");
        assert!(commit_payload["transaction"]["writes"].as_array().is_some());

        let store = state.tenant_graph_store("tenant-tx").unwrap();
        let node = store.get_node("node:tx-commit").unwrap().unwrap();
        assert_eq!(node.id, "node:tx-commit");
    }

    #[tokio::test]
    async fn transaction_routes_support_rollback() {
        let state = memory_product_state();
        let begin_response = transaction_begin(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionBeginBody {
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(begin_response.status(), StatusCode::OK);
        let begin_payload = response_payload_json(begin_response).await;
        let tx_id = begin_payload["tx_id"].as_str().unwrap();

        let stage_response = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-tx".to_string()),
                query: "CREATE (n:File {id: $id, path: $path})".to_string(),
                params: BTreeMap::from([
                    ("id".to_string(), json!("node:tx-rollback")),
                    ("path".to_string(), json!("src/rollback.rs")),
                ]),
                tx_id: Some(tx_id.to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(stage_response.status(), StatusCode::OK);

        let rollback_response = transaction_rollback(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionMutationBody {
                tx_id: tx_id.to_string(),
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(rollback_response.status(), StatusCode::OK);
        let rollback_payload = response_payload_json(rollback_response).await;
        assert_eq!(rollback_payload["status"], "rolled_back");
        assert_eq!(rollback_payload["tx_id"], tx_id);

        let store = state.tenant_graph_store("tenant-tx").unwrap();
        assert!(store.get_node("node:tx-rollback").unwrap().is_none());
    }

    #[tokio::test]
    async fn transaction_commit_rejects_missing_tx_id() {
        let state = memory_product_state();
        let response = transaction_commit(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionMutationBody {
                tx_id: String::new(),
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["error"], "missing_tx_id");
    }
}
