use std::sync::Arc;

use thg_core::sanitize_tenant_segment;
use thg_core::store::RedisThgStore;
use thg_core::RedisGraphStore;
use thg_mcp::{McpError, McpGraphProvider, McpServerConfig};

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    pub fn tenant_store(&self, tenant_id: &str) -> redis::RedisResult<RedisThgStore> {
        RedisThgStore::new(&self.config.redis_url, self.tenant_state_key(tenant_id))
    }

    pub fn tenant_state_key(&self, tenant_id: &str) -> String {
        let safe_tenant = sanitize_tenant_segment(tenant_id);
        format!("{}:{}:state:v1", self.config.redis_key_prefix, safe_tenant)
    }

    pub fn tenant_graph_store(&self, tenant_id: &str) -> redis::RedisResult<RedisGraphStore> {
        RedisGraphStore::tenant(
            &self.config.redis_url,
            &self.config.redis_key_prefix,
            tenant_id,
        )
    }

    pub fn store_ready(&self) -> redis::RedisResult<()> {
        let key = format!("{}:__ready__:state:v1", self.config.redis_key_prefix);
        RedisThgStore::new(&self.config.redis_url, key)?.ping()
    }

    pub fn mcp_config(&self) -> McpServerConfig {
        McpServerConfig {
            name: self.config.service_name.clone(),
            version: "0.1.0".to_string(),
            default_tenant: self.config.mcp_default_tenant.clone(),
            read_only: self.config.mcp_read_only,
            allow_admin: self.config.mcp_allow_admin,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    use super::AppState;

    #[test]
    fn tenant_state_keys_use_graph_store_tenant_normalization() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            redis_url: "redis://127.0.0.1:6379".to_string(),
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

        assert_eq!(
            state.tenant_state_key("Tenant.One!"),
            "rusty-red:TenantOne:state:v1"
        );
    }
}

impl McpGraphProvider for AppState {
    type Backend = RedisGraphStore;

    fn backend_for_tenant(&self, tenant: &str) -> Result<Self::Backend, McpError> {
        self.tenant_graph_store(tenant)
            .map_err(|error| McpError::internal(error.to_string()))
    }
}
