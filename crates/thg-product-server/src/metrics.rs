use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde_json::{json, Value};

use crate::auth::require_scope;
use crate::config::StorageMode;
use crate::state::AppState;

pub async fn metrics(
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
