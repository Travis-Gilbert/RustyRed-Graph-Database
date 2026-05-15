use std::env;

use crate::auth::ApiToken;
use thg_core::RedCoreDurability;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageMode {
    Embedded,
    Memory,
    Redis,
}

impl StorageMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "redis" | "legacy_redis" => Self::Redis,
            "memory" | "ram" => Self::Memory,
            _ => Self::Embedded,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Memory => "memory",
            Self::Redis => "redis",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub storage_mode: StorageMode,
    pub data_dir: String,
    pub require_volume: bool,
    pub volume_available: bool,
    pub durability: RedCoreDurability,
    pub snapshot_interval_writes: u64,
    pub strict_acid: bool,
    pub concurrency: String,
    pub txn_isolation: String,
    pub tenant_memory_quota_bytes: usize,
    pub tenant_memory_quota_config_error: Option<String>,
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
        let storage_mode = env_first(&["RUSTY_RED_MODE", "THG_PRODUCT_STORE"])
            .map(|value| StorageMode::parse(&value))
            .unwrap_or(StorageMode::Embedded);
        let railway_volume_mount_path = env::var("RAILWAY_VOLUME_MOUNT_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let data_dir = env_first(&["RUSTY_RED_DATA_DIR", "THG_PRODUCT_DATA_DIR"])
            .or_else(|_| {
                railway_volume_mount_path
                    .clone()
                    .ok_or(env::VarError::NotPresent)
            })
            .unwrap_or_else(|_| {
                if railway_port.is_some() {
                    "/app/data/rusty-red".to_string()
                } else {
                    "data/rusty-red".to_string()
                }
            });
        let require_volume = env_bool(
            &["RUSTY_RED_REQUIRE_VOLUME", "THG_PRODUCT_REQUIRE_VOLUME"],
            railway_port.is_some(),
        );
        let volume_available = railway_volume_mount_path.is_some()
            || env_bool(
                &["RUSTY_RED_VOLUME_MOUNTED", "THG_PRODUCT_VOLUME_MOUNTED"],
                false,
            );
        let durability = env_first(&["RUSTY_RED_DURABILITY", "THG_PRODUCT_DURABILITY"])
            .map(|value| RedCoreDurability::parse(&value))
            .unwrap_or(RedCoreDurability::AofEverysec);
        let snapshot_interval_writes = env_first(&[
            "RUSTY_RED_SNAPSHOT_INTERVAL_WRITES",
            "THG_PRODUCT_SNAPSHOT_INTERVAL_WRITES",
        ])
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000);
        let strict_acid = env_bool(&["RUSTY_RED_STRICT_ACID", "THG_PRODUCT_STRICT_ACID"], false);
        let concurrency = env_first(&["RUSTY_RED_CONCURRENCY", "THG_PRODUCT_CONCURRENCY"])
            .unwrap_or_else(|_| "single_writer".to_string());
        let txn_isolation = env_first(&["RUSTY_RED_TXN_ISOLATION", "THG_PRODUCT_TXN_ISOLATION"])
            .unwrap_or_else(|_| {
                if strict_acid {
                    "serializable".to_string()
                } else {
                    "snapshot".to_string()
                }
            });
        let (tenant_memory_quota_bytes, tenant_memory_quota_config_error) = env_usize(
            &[
                "RUSTY_RED_TENANT_MEMORY_QUOTA_BYTES",
                "THG_PRODUCT_TENANT_MEMORY_QUOTA_BYTES",
            ],
            0,
        );
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
            storage_mode,
            data_dir,
            require_volume,
            volume_available,
            durability,
            snapshot_interval_writes,
            strict_acid,
            concurrency,
            txn_isolation,
            tenant_memory_quota_bytes,
            tenant_memory_quota_config_error,
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

    pub fn validate(&self) -> Result<(), String> {
        if let Some(error) = &self.tenant_memory_quota_config_error {
            return Err(error.clone());
        }
        if self.storage_mode == StorageMode::Embedded
            && self.require_volume
            && !self.volume_available
        {
            return Err(
                "RUSTY_RED_REQUIRE_VOLUME=true requires RAILWAY_VOLUME_MOUNT_PATH or RUSTY_RED_VOLUME_MOUNTED=true"
                    .to_string(),
                );
        }
        if self.storage_mode == StorageMode::Redis && self.tenant_memory_quota_bytes > 0 {
            return Err(
                "RUSTY_RED_TENANT_MEMORY_QUOTA_BYTES is currently enforced only for RUSTY_RED_MODE=embedded or RUSTY_RED_MODE=memory; redis mode quota enforcement is a separate follow-up gate"
                    .to_string(),
            );
        }
        if !self.strict_acid {
            return Ok(());
        }
        if self.storage_mode != StorageMode::Embedded {
            return Err(format!(
                "RUSTY_RED_STRICT_ACID=true requires RUSTY_RED_MODE=embedded, got {}",
                self.storage_mode.as_str()
            ));
        }
        if self.durability != RedCoreDurability::AofAlways {
            return Err(format!(
                "RUSTY_RED_STRICT_ACID=true requires RUSTY_RED_DURABILITY=aof_always, got {}",
                self.durability.as_str()
            ));
        }
        if self.concurrency.trim() != "single_writer" {
            return Err(format!(
                "RUSTY_RED_STRICT_ACID=true requires RUSTY_RED_CONCURRENCY=single_writer, got {}",
                self.concurrency
            ));
        }
        if self.txn_isolation.trim() != "serializable" {
            return Err(format!(
                "RUSTY_RED_STRICT_ACID=true requires RUSTY_RED_TXN_ISOLATION=serializable, got {}",
                self.txn_isolation
            ));
        }
        Ok(())
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

fn env_usize(keys: &[&str], default: usize) -> (usize, Option<String>) {
    match env_first(keys) {
        Ok(value) => match value.parse::<usize>() {
            Ok(parsed) => (parsed, None),
            Err(_) => (
                default,
                Some(format!(
                    "{} must be an unsigned byte count, got {value}",
                    keys.join(" or "),
                )),
            ),
        },
        Err(_) => (default, None),
    }
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

#[cfg(test)]
mod tests {
    use super::{Config, RedCoreDurability, StorageMode};

    fn base_config() -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Embedded,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 1_000,
            strict_acid: true,
            concurrency: "single_writer".to_string(),
            txn_isolation: "serializable".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
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
        }
    }

    #[test]
    fn strict_acid_config_requires_aof_always() {
        let mut config = base_config();
        config.durability = RedCoreDurability::AofEverysec;

        assert!(config.validate().unwrap_err().contains("aof_always"));
    }

    #[test]
    fn strict_acid_config_accepts_single_writer_serializable_embedded() {
        assert_eq!(base_config().validate(), Ok(()));
    }

    #[test]
    fn embedded_config_rejects_missing_required_volume() {
        let mut config = base_config();
        config.require_volume = true;
        config.volume_available = false;

        assert!(config.validate().unwrap_err().contains("REQUIRE_VOLUME"));
    }

    #[test]
    fn embedded_config_accepts_required_available_volume() {
        let mut config = base_config();
        config.require_volume = true;
        config.volume_available = true;

        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn invalid_tenant_memory_quota_config_fails_validation() {
        let mut config = base_config();
        config.tenant_memory_quota_config_error =
            Some("RUSTY_RED_TENANT_MEMORY_QUOTA_BYTES must be an unsigned byte count".to_string());

        assert!(config
            .validate()
            .unwrap_err()
            .contains("unsigned byte count"));
    }

    #[test]
    fn redis_mode_rejects_tenant_memory_quota_until_supported() {
        let mut config = base_config();
        config.storage_mode = StorageMode::Redis;
        config.tenant_memory_quota_bytes = 1024;

        assert!(config.validate().unwrap_err().contains("redis mode quota"));
    }
}
