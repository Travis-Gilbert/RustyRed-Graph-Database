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
use thg_core::commands::ThgRequest;
use thg_core::executor::{StoreBackedThgExecutor, ThgExecutor};
use thg_core::{EdgeRecord, GraphStoreError, NeighborQuery, NodeRecord};
use thg_mcp::{agent_manifest, handle_mcp_request_with_context, mcp_manifest, McpRequestContext};
use tower_http::cors::{Any, CorsLayer};

use crate::auth::require_scope;
use crate::state::AppState;

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
        .route("/v1/tenants/:tenant_id/command", post(command))
        .route("/v1/tenants/:tenant_id/batch", post(batch))
        .route("/v1/tenants/:tenant_id/runs/:run_id", get(run_get))
        .route("/v1/tenants/:tenant_id/graph/query", post(graph_query))
        .route(
            "/v1/tenants/:tenant_id/graph/nodes",
            post(graph_node_upsert),
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
        .route("/v1/tenants/:tenant_id/context/pack", post(context_pack))
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
    if state.store_ready().is_ok() {
        return Json(json!({
            "status": "ready",
            "store": "ready"
        }))
        .into_response();
    }

    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "status": "not_ready",
            "store": "unavailable"
        })),
    )
        .into_response()
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

    if state.store_ready().is_err() {
        return store_unavailable_response();
    }
    let store = match state.tenant_store(&tenant_id) {
        Ok(store) => store,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut executor = StoreBackedThgExecutor::new(store);
    let results = body
        .commands
        .into_iter()
        .map(|item| executor.execute_request(ThgRequest::new(item.command, item.args)))
        .collect::<Vec<_>>();
    let state_hash = executor.state().hash();
    Json(json!({ "ok": true, "results": results, "state_hash": state_hash })).into_response()
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
        Err(error) => return graph_store_error_response(error.into()),
    };
    match store.upsert_node(body.into_record()) {
        Ok(result) => Json(json!({ "ok": true, "node": result })).into_response(),
        Err(error) => graph_store_error_response(error),
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
        Err(error) => return graph_store_error_response(error.into()),
    };
    match store.get_node(&node_id) {
        Ok(Some(node)) => Json(json!({ "ok": true, "node": node })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
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
        Err(error) => return graph_store_error_response(error.into()),
    };
    match store.upsert_edge(body.into_record()) {
        Ok(result) => Json(json!({ "ok": true, "edge": result })).into_response(),
        Err(error) => graph_store_error_response(error),
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
        Err(error) => return graph_store_error_response(error.into()),
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
        Err(error) => return graph_store_error_response(error.into()),
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
        Err(error) => return graph_store_error_response(error.into()),
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
        Err(error) => return graph_store_error_response(error.into()),
    };
    match store.verify() {
        Ok(report) => Json(json!({ "ok": report.ok, "verify": report })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

fn execute_tenant_command(
    state: &AppState,
    tenant_id: &str,
    command: &str,
    args: Value,
) -> axum::response::Response {
    if state.store_ready().is_err() {
        return store_unavailable_response();
    }
    let store = match state.tenant_store(tenant_id) {
        Ok(store) => store,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut executor = StoreBackedThgExecutor::new(store);
    let response = executor.execute_request(ThgRequest::new(command, args));
    Json(response).into_response()
}

fn store_unavailable_response() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "store_unavailable",
            "message": "Redis-compatible backing store is unavailable; check THG_REDIS_URL or REDIS_URL."
        })),
    )
        .into_response()
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
        | "missing_graph_endpoint"
        | "tombstoned_graph_endpoint"
        | "invalid_graph_record" => StatusCode::BAD_REQUEST,
        "redis_graph_store_error" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn required_scope_for_command(command: &str) -> &'static str {
    match command.trim().to_ascii_uppercase().as_str() {
        "THG.RUN.GET" => "run:read",
        "THG.RUN.BEGIN" | "THG.RUN.STEP" => "run:write",
        "THG.CONTEXT.GET" => "context:read",
        "THG.CONTEXT.PACK" => "context:write",
        "THG.STATE.HASH" | "THG.DEBUG.CYPHER" | "THG.CYPHER" => "graph:read",
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

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    use super::{graph_error_status, mcp_origin_allowed, required_scope_for_command};

    #[test]
    fn maps_core_commands_to_product_scopes() {
        assert_eq!(required_scope_for_command("THG.RUN.GET"), "run:read");
        assert_eq!(required_scope_for_command("THG.RUN.BEGIN"), "run:write");
        assert_eq!(
            required_scope_for_command("THG.CONTEXT.PACK"),
            "context:write"
        );
        assert_eq!(required_scope_for_command("THG.DEBUG.CYPHER"), "graph:read");
    }

    #[test]
    fn maps_graph_store_errors_to_http_statuses() {
        assert_eq!(
            graph_error_status("missing_graph_endpoint"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            graph_error_status("redis_graph_store_error"),
            StatusCode::SERVICE_UNAVAILABLE
        );
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
}
