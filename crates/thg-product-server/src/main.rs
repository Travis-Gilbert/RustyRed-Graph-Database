mod auth;
mod config;
mod state;

use std::net::SocketAddr;

use axum::{routing::get, Router};
use config::Config;
use state::AppState;

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let config = Config::from_env();
    let addr: SocketAddr = config.bind_addr().parse().map_err(|exc| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{exc}"))
    })?;
    let state = AppState::new(config);
    let app = Router::new()
        .route("/health", get(health))
        .with_state(state);

    tracing::info!("THG_PRODUCT_READY {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
