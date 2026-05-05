use std::env;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use thg_core::InMemoryThgExecutor;
use thg_server::{serve, SharedExecutor};

fn main() -> std::io::Result<()> {
    let config = Config::from_env_and_args();
    let listener = TcpListener::bind((config.host.as_str(), config.port))?;
    let local_addr = listener.local_addr()?;
    eprintln!("THG_SERVER_READY {}", local_addr);

    let executor: SharedExecutor = Arc::new(Mutex::new(InMemoryThgExecutor::new()));
    serve(listener, executor)
}

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
}

impl Config {
    fn from_env_and_args() -> Self {
        let mut host = env::var("THG_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let mut port = env::var("THG_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(7379);

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

        Self { host, port }
    }
}
