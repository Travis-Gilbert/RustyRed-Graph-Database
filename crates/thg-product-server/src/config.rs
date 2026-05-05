use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub redis_url: String,
    pub redis_key_prefix: String,
    pub require_auth: bool,
    pub allowed_origins: Vec<String>,
    pub api_tokens: Vec<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let railway_port = env::var("PORT").ok();
        let host = env::var("THG_PRODUCT_HOST").unwrap_or_else(|_| {
            if railway_port.is_some() {
                "0.0.0.0".to_string()
            } else {
                "127.0.0.1".to_string()
            }
        });
        let port = env::var("THG_PRODUCT_PORT")
            .ok()
            .or(railway_port)
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(8380);
        let redis_url = env::var("THG_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let redis_key_prefix = env::var("THG_REDIS_KEY_PREFIX")
            .unwrap_or_else(|_| "theseus:thg:tenant".to_string());
        let require_auth = env::var("THG_REQUIRE_AUTH")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let allowed_origins = env::var("THG_ALLOWED_ORIGINS")
            .unwrap_or_else(|_| "http://localhost:3000".to_string())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
        let api_tokens = env::var("THG_API_TOKENS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();

        Self {
            host,
            port,
            redis_url,
            redis_key_prefix,
            require_auth,
            allowed_origins,
            api_tokens,
        }
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
