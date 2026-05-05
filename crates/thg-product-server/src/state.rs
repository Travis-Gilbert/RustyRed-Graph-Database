use std::sync::Arc;

use thg_core::store::RedisThgStore;

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
        let safe_tenant = tenant_id
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .collect::<String>();
        let key = format!("{}:{}:state:v1", self.config.redis_key_prefix, safe_tenant);
        RedisThgStore::new(&self.config.redis_url, key)
    }
}
