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

#[derive(Debug, Serialize)]
pub struct HealthBody {
    pub status: &'static str,
}

pub fn build_router(state: AppState) -> Router {
    let cors = cors_layer(&state);
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/openapi.json", get(crate::openapi::openapi))
        .route("/metrics", get(crate::metrics::metrics))
        .route("/v1/tenants/:tenant_id/command", post(command))
        .route("/v1/tenants/:tenant_id/batch", post(batch))
        .route("/v1/tenants/:tenant_id/runs/:run_id", get(run_get))
        .route("/v1/tenants/:tenant_id/graph/query", post(graph_query))
        .route("/v1/tenants/:tenant_id/context/pack", post(context_pack))
        .layer(cors)
        .with_state(state)
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "ok" })
}

async fn ready() -> Json<HealthBody> {
    Json(HealthBody { status: "ready" })
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

fn execute_tenant_command(
    state: &AppState,
    tenant_id: &str,
    command: &str,
    args: Value,
) -> axum::response::Response {
    let store = match state.tenant_store(tenant_id) {
        Ok(store) => store,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut executor = StoreBackedThgExecutor::new(store);
    let response = executor.execute_request(ThgRequest::new(command, args));
    Json(response).into_response()
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

#[cfg(test)]
mod tests {
    use super::required_scope_for_command;

    #[test]
    fn maps_core_commands_to_product_scopes() {
        assert_eq!(required_scope_for_command("THG.RUN.GET"), "run:read");
        assert_eq!(required_scope_for_command("THG.RUN.BEGIN"), "run:write");
        assert_eq!(required_scope_for_command("THG.CONTEXT.PACK"), "context:write");
        assert_eq!(required_scope_for_command("THG.DEBUG.CYPHER"), "graph:read");
    }
}
