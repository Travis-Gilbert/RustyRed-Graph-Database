use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};

use crate::auth::require_scope;
use crate::config::StorageMode;
use crate::state::AppState;

/// `GET /metrics` — Prometheus text exposition.
///
/// Returns counters in `# HELP / # TYPE / name value\n` form. The mime type
/// is the Prometheus 0.0.4 text exposition format.
pub async fn metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    require_scope(
        &headers,
        &state.config.api_tokens,
        "admin:read",
        state.config.require_auth,
    )?;
    let body = state.observability.render_prometheus();
    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    Ok(resp)
}

/// `GET /v1/diagnostics/slow_queries` — returns the slow-query ring buffer.
pub async fn slow_queries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_scope(
        &headers,
        &state.config.api_tokens,
        "admin:read",
        state.config.require_auth,
    )?;
    let entries = state.observability.snapshot_slow_queries();
    let entries_json: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            json!({
                "recorded_at_unix_ms": e.recorded_at_unix_ms.to_string(),
                "nanos": e.nanos,
                "kind": e.kind,
                "detail": e.detail,
                "nodes_visited": e.nodes_visited,
                "edges_touched": e.edges_touched,
            })
        })
        .collect();
    Ok(Json(json!({
        "entries": entries_json,
        "count": entries_json.len(),
    })))
}

/// `GET /v1/diagnostics/config` — exposes static configuration (previously
/// served by `/metrics`). Kept for backward compatibility with operators.
pub async fn diagnostics_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_scope(
        &headers,
        &state.config.api_tokens,
        "admin:read",
        state.config.require_auth,
    )?;
    let tenant_memory_quota_supported = matches!(
        state.config.storage_mode,
        StorageMode::Embedded | StorageMode::Memory
    );
    Ok(Json(json!({
        "service": state.config.service_name.as_str(),
        "status": "ok",
        "auth_required": state.config.require_auth,
        "configured_origins": state.config.allowed_origins.len(),
        "storage_mode": state.config.storage_mode.as_str(),
        "durability": state.config.durability.as_str(),
        "strict_acid": state.config.strict_acid,
        "tenant_memory_quota_bytes": state.config.tenant_memory_quota_bytes,
        "tenant_memory_quota_supported": tenant_memory_quota_supported,
        "tenant_memory_quota_enforced": tenant_memory_quota_supported
            && state.config.tenant_memory_quota_bytes > 0
    })))
}
