use std::env;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use thg_core::executor::StoreBackedThgExecutor;
use thg_core::store::RedisThgStore;
use thg_core::InMemoryThgExecutor;
use thg_server::{serve, SharedExecutor};

fn main() -> std::io::Result<()> {
    let config = Config::from_env_and_args();
    let listener = TcpListener::bind((config.host.as_str(), config.port))?;
    let local_addr = listener.local_addr()?;
    eprintln!("THG_SERVER_READY {}", local_addr);

    let executor: SharedExecutor = if config.store == "redis" {
        let redis_url = env::var("THG_REDIS_URL").unwrap_or_else(|_| {
            env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
        });
        let store = RedisThgStore::new(&redis_url, config.redis_key.clone())
            .map_err(|exc| std::io::Error::new(std::io::ErrorKind::Other, exc.to_string()))?;
        Arc::new(Mutex::new(Box::new(StoreBackedThgExecutor::new(store))))
    } else {
        Arc::new(Mutex::new(Box::new(InMemoryThgExecutor::new())))
    };
    serve(listener, executor)
}

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
    store: String,
    redis_key: String,
}

impl Config {
    fn from_env_and_args() -> Self {
        let railway_port = env::var("PORT").ok();
        let mut host = env::var("THG_HOST").unwrap_or_else(|_| {
            if railway_port.is_some() {
                "0.0.0.0".to_string()
            } else {
                "127.0.0.1".to_string()
            }
        });
        let mut port = env::var("THG_PORT")
            .ok()
            .or(railway_port)
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(7379);
        let store = env::var("THG_STORE").unwrap_or_else(|_| "memory".to_string());
        let redis_key =
            env::var("THG_REDIS_KEY").unwrap_or_else(|_| "theseus:thg:state:v1".to_string());

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--host" => {
                    if let Some(value) = args.next() {
                        host = value;
                    }
                }
                "--port" => {
                    if let Some(value) = args.next().and_then(|item| item.parse::<u16>().ok()) {
                        port = value;
                    }
                }
                _ => {}
            }
        }

        Self {
            host,
            port,
            store,
            redis_key,
        }
    }
}
