#![recursion_limit = "512"]

mod auth;
mod bulk;
mod config;
mod cypher;
mod graph_cache;
mod grpc;
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

    // Build the HTTP router (existing) and the gRPC routes (new). Both
    // serve from the same TCP listener via tonic 0.12's axum-router
    // bridge: Routes::into_axum_router() returns an axum Router that
    // routes `Content-Type: application/grpc*` requests to the gRPC
    // services, which we merge with the HTTP router so non-gRPC traffic
    // continues to flow through the existing handlers unchanged.
    let http_router = router::build_router(state.clone());
    let grpc_router = grpc::build_grpc_routes(state).into_axum_router();
    let app = http_router.merge(grpc_router);

    tracing::info!("RUSTYRED_PRODUCT_READY {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
