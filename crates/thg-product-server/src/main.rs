#![recursion_limit = "512"]

mod auth;
mod config;
mod graph_cache;
mod metrics;
mod observability;
mod openapi;
mod query_surface;
mod router;
mod state;

use std::net::SocketAddr;

use config::Config;
use state::AppState;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let config = Config::from_env();
    config
        .validate()
        .map_err(|exc| std::io::Error::new(std::io::ErrorKind::InvalidInput, exc))?;
    let addr: SocketAddr = config
        .bind_addr()
        .parse()
        .map_err(|exc| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{exc}")))?;
    let state = AppState::new(config);
    let app = router::build_router(state);

    tracing::info!("THG_PRODUCT_READY {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
