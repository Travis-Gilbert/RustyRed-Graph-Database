// gRPC server module.
//
// Mounts a tonic service implementing rustyred.v1.GraphDatabase alongside
// the existing axum HTTP routes on the same port. Content-type sniffing
// routes incoming requests: `Content-Type: application/grpc*` go to
// tonic, everything else to axum.
//
// See theorem-protos/rustyred/v1/rustyred.proto for the canonical
// service contract.

pub mod service;

// Re-export the tonic-generated code at a stable path. The macro below
// is what `tonic-build` produces from rustyred.v1.proto at compile time.
pub mod proto {
    tonic::include_proto!("rustyred.v1");
}

pub use proto::graph_database_server::GraphDatabaseServer;
pub use service::GraphDatabaseService;

use crate::state::AppState;

/// Build a tonic service tree implementing rustyred.v1.GraphDatabase
/// against the shared AppState. Returns a Routes that can be merged
/// onto the axum router.
pub fn build_grpc_routes(state: AppState) -> tonic::service::Routes {
    let service = GraphDatabaseService::new(state);
    let server = GraphDatabaseServer::new(service);
    tonic::service::Routes::new(server)
}
