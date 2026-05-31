use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Map, Value};

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
    let tenant_override_detail = state
        .config
        .tenant_config_overrides
        .iter()
        .map(|(tenant, config)| {
            let mut detail = Map::new();
            if let Some(durability) = &config.durability {
                detail.insert(
                    "durability".to_string(),
                    Value::String(durability.as_str().to_string()),
                );
            }
            if let Some(snapshot_interval_writes) = config.snapshot_interval_writes {
                detail.insert(
                    "snapshot_interval_writes".to_string(),
                    json!(snapshot_interval_writes),
                );
            }
            if let Some(strict_acid) = config.strict_acid {
                detail.insert("strict_acid".to_string(), json!(strict_acid));
            }
            if let Some(tenant_memory_quota_bytes) = config.tenant_memory_quota_bytes {
                detail.insert(
                    "tenant_memory_quota_bytes".to_string(),
                    json!(tenant_memory_quota_bytes),
                );
            }
            if let Some(hybrid_scoring) = &config.hybrid_scoring {
                detail.insert(
                    "hybrid_scoring".to_string(),
                    json!({
                        "alpha": hybrid_scoring.alpha,
                        "confidence_weighted_graph_distance": hybrid_scoring.confidence_weighted_graph_distance,
                        "edge_type_weights": &hybrid_scoring.edge_type_weights,
                    }),
                );
            }
            (tenant.clone(), Value::Object(detail))
        })
        .collect::<Map<String, Value>>();
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
            && state.config.tenant_memory_quota_bytes > 0,
        "slow_query_threshold_nanos": state.config.slow_query_threshold_nanos,
        "slow_query_capacity": state.config.slow_query_capacity,
        "slow_query_log_enabled": state.config.slow_query_log.is_some(),
        "federation": {
            "enabled": state.config.federate,
            "hub_configured": state.config.federate_hub_url.is_some(),
            "token_configured": state.config.federate_token.is_some(),
            "peer_id_configured": state.config.federate_peer_id.is_some(),
            "private_key_configured": state.config.federate_private_key.is_some(),
            "provenance_enabled": state.config.federate_provenance,
            "snapshot_text_bytes": state.config.federate_snapshot_text_bytes,
        },
        "hybrid_scoring": {
            "alpha": state.config.hybrid_scoring.alpha,
            "confidence_weighted_graph_distance": state.config.hybrid_scoring.confidence_weighted_graph_distance,
            "edge_type_weights": &state.config.hybrid_scoring.edge_type_weights,
        },
        "tenant_config_overrides": state.config.tenant_config_overrides.len(),
        "tenant_config_runtime_mutation_supported": false,
        "tenant_config_tenants": state
            .config
            .tenant_config_overrides
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        "tenant_config_overrides_detail": tenant_override_detail,
    })))
}
