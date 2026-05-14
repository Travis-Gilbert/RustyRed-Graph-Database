use std::env;

use crate::auth::ApiToken;

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub redis_url: String,
    pub redis_key_prefix: String,
    pub require_auth: bool,
    pub allowed_origins: Vec<String>,
    pub api_tokens: Vec<ApiToken>,
    pub service_name: String,
    pub api_title: String,
    pub public_url: Option<String>,
    pub mcp_enabled: bool,
    pub mcp_read_only: bool,
    pub mcp_allow_admin: bool,
    pub mcp_default_tenant: String,
}

impl Config {
    pub fn from_env() -> Self {
        let railway_port = env::var("PORT").ok();
        let host = env_first(&["RUSTY_RED_HOST", "THG_PRODUCT_HOST"]).unwrap_or_else(|_| {
            if railway_port.is_some() {
                "0.0.0.0".to_string()
            } else {
                "127.0.0.1".to_string()
            }
        });
        let port = railway_port
            .clone()
            .or_else(|| env_first(&["RUSTY_RED_PORT", "THG_PRODUCT_PORT"]).ok())
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(8380);
        let redis_url = env_first(&["RUSTY_RED_REDIS_URL", "THG_REDIS_URL"])
            .or_else(|_| env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let redis_key_prefix = env_first(&["RUSTY_RED_KEY_PREFIX", "THG_REDIS_KEY_PREFIX"])
            .unwrap_or_else(|_| "theseus:thg:tenant".to_string());
        let require_auth = env_first(&["RUSTY_RED_REQUIRE_AUTH", "THG_REQUIRE_AUTH"])
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let allowed_origins = env_first(&["RUSTY_RED_ALLOWED_ORIGINS", "THG_ALLOWED_ORIGINS"])
            .unwrap_or_else(|_| "http://localhost:3000".to_string())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
        let api_tokens = env_first(&["RUSTY_RED_API_TOKENS", "THG_API_TOKENS"])
            .unwrap_or_default()
            .split(',')
            .filter_map(ApiToken::parse)
            .collect();
        let service_name = env_first(&["RUSTY_RED_SERVICE_NAME", "THG_SERVICE_NAME"])
            .unwrap_or_else(|_| "thg-product".to_string());
        let api_title = env_first(&["RUSTY_RED_API_TITLE", "THG_API_TITLE"])
            .unwrap_or_else(|_| "Theorem Context THG API".to_string());
        let public_url = env_first(&["RUSTY_RED_PUBLIC_URL", "THG_PUBLIC_URL"]).ok();
        let mcp_enabled = env_bool(&["RUSTY_RED_MCP_ENABLED", "THG_MCP_ENABLED"], true);
        let mcp_read_only = env_bool(&["RUSTY_RED_MCP_READ_ONLY", "THG_MCP_READ_ONLY"], true);
        let mcp_allow_admin =
            env_bool(&["RUSTY_RED_MCP_ALLOW_ADMIN", "THG_MCP_ALLOW_ADMIN"], false);
        let mcp_default_tenant =
            env_first(&["RUSTY_RED_MCP_DEFAULT_TENANT", "THG_MCP_DEFAULT_TENANT"])
                .unwrap_or_else(|_| "default".to_string());

        Self {
            host,
            port,
            redis_url,
            redis_key_prefix,
            require_auth,
            allowed_origins,
            api_tokens,
            service_name,
            api_title,
            public_url,
            mcp_enabled,
            mcp_read_only,
            mcp_allow_admin,
            mcp_default_tenant,
        }
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn env_bool(keys: &[&str], default: bool) -> bool {
    env_first(keys)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_first(keys: &[&str]) -> Result<String, env::VarError> {
    for key in keys {
        match env::var(key) {
            Ok(value) if !value.trim().is_empty() => return Ok(value),
            Ok(_) => continue,
            Err(env::VarError::NotPresent) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(env::VarError::NotPresent)
}
