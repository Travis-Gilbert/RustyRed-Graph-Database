// GraphDatabaseService — tonic-side implementation of rustyred.v1.GraphDatabase.
//
// This file mirrors the HTTP routes defined in router.rs but exposes them
// over gRPC. The MAJORITY of methods in this first commit are stubbed with
// `Status::unimplemented` and a comment naming the HTTP route they should
// mirror; filling them in is mechanical (parse gRPC request → call the
// existing handler logic → map the response into a gRPC response message).
//
// The methods implemented in this first commit are the "vital signs" that
// prove the server is alive and the architecture works:
//
//   - Health        — does the server respond at all?
//   - Ready         — does the server pass its readiness check?
//   - GraphStats    — can we read the graph state through gRPC?
//
// Subsequent commits fill in the remaining 29 methods. Each todo points
// at the existing axum route handler so the mapping work is bounded.

use tonic::{Request, Response, Status};

use super::proto;
use super::proto::graph_database_server::GraphDatabase;
use crate::state::AppState;

#[derive(Clone)]
pub struct GraphDatabaseService {
    state: AppState,
}

impl GraphDatabaseService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

// One-stop helper so each unimplemented stub stays a single line below.
// Returns a tonic Status saying "this method is pending; see the HTTP
// route for the equivalent surface."
fn pending(method: &str, http_route: &str) -> Status {
    Status::unimplemented(format!(
        "rustyred.v1.GraphDatabase/{} — see HTTP route {} for the equivalent surface; gRPC mapping pending.",
        method, http_route
    ))
}

#[tonic::async_trait]
impl GraphDatabase for GraphDatabaseService {
    // ====================================================================
    // Lifecycle — IMPLEMENTED
    // ====================================================================

    async fn health(
        &self,
        _request: Request<proto::HealthRequest>,
    ) -> Result<Response<proto::HealthResponse>, Status> {
        Ok(Response::new(proto::HealthResponse {
            status: "ok".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    async fn ready(
        &self,
        _request: Request<proto::ReadyRequest>,
    ) -> Result<Response<proto::ReadyResponse>, Status> {
        // Mirrors the axum /ready handler (router.rs ~line 377).
        match self.state.store_ready() {
            Ok(_report) => Ok(Response::new(proto::ReadyResponse {
                ready: true,
                reason: String::new(),
            })),
            Err(error) => Ok(Response::new(proto::ReadyResponse {
                ready: false,
                reason: format!("{}: {}", error.code, error.message),
            })),
        }
    }

    // ====================================================================
    // Graph stats — IMPLEMENTED
    // ====================================================================

    async fn graph_stats(
        &self,
        request: Request<proto::GraphStatsRequest>,
    ) -> Result<Response<proto::GraphStatsResponse>, Status> {
        let tenant_id = request.into_inner().tenant_id;
        // Mirrors the axum /v1/tenants/{tenant_id}/graph/stats handler
        // (router.rs ~line 1313). The per-label / per-edge-type maps
        // are not yet exposed by rustyred_core::GraphStats — they are
        // left empty in the proto response and TODO'd for follow-on
        // when the core surface adds those breakdowns.
        let store = self
            .state
            .tenant_graph_store(&tenant_id)
            .map_err(|err| Status::failed_precondition(format!("tenant store unavailable: {}: {}", err.code, err.message)))?;
        let stats = store
            .stats()
            .map_err(|err| Status::internal(format!("graph_stats: {:?}", err)))?;
        Ok(Response::new(proto::GraphStatsResponse {
            node_count: stats.nodes_total as u64,
            edge_count: stats.edges_total as u64,
            graph_version: stats.version,
            nodes_by_label: std::collections::HashMap::new(),
            edges_by_type: std::collections::HashMap::new(),
        }))
    }

    // ====================================================================
    // Native query surface — PENDING
    // ====================================================================

    async fn query(&self, _r: Request<proto::QueryRequest>) -> Result<Response<proto::QueryResponse>, Status> {
        Err(pending("query", "POST /v1/query"))
    }
    async fn cypher(&self, _r: Request<proto::CypherRequest>) -> Result<Response<proto::CypherResponse>, Status> {
        Err(pending("cypher", "POST /v1/cypher"))
    }
    async fn cypher_explain(&self, _r: Request<proto::CypherRequest>) -> Result<Response<proto::CypherExplainResponse>, Status> {
        Err(pending("cypher_explain", "POST /v1/cypher/explain"))
    }

    // ====================================================================
    // Transactions — PENDING
    // ====================================================================

    async fn begin_transaction(&self, _r: Request<proto::BeginTxnRequest>) -> Result<Response<proto::BeginTxnResponse>, Status> {
        Err(pending("begin_transaction", "POST /v1/transactions/begin"))
    }
    async fn commit_transaction(&self, _r: Request<proto::CommitTxnRequest>) -> Result<Response<proto::CommitTxnResponse>, Status> {
        Err(pending("commit_transaction", "POST /v1/transactions/commit"))
    }
    async fn rollback_transaction(&self, _r: Request<proto::RollbackTxnRequest>) -> Result<Response<proto::RollbackTxnResponse>, Status> {
        Err(pending("rollback_transaction", "POST /v1/transactions/rollback"))
    }

    // ====================================================================
    // Node + edge primitives — PENDING
    // ====================================================================

    async fn upsert_node(&self, _r: Request<proto::UpsertNodeRequest>) -> Result<Response<proto::Node>, Status> {
        Err(pending("upsert_node", "POST /v1/tenants/{tenant_id}/graph/nodes"))
    }
    async fn upsert_edge(&self, _r: Request<proto::UpsertEdgeRequest>) -> Result<Response<proto::Edge>, Status> {
        Err(pending("upsert_edge", "POST /v1/tenants/{tenant_id}/graph/edges"))
    }
    async fn get_node(&self, _r: Request<proto::GetNodeRequest>) -> Result<Response<proto::Node>, Status> {
        Err(pending("get_node", "GET /v1/tenants/{tenant_id}/graph/nodes/{node_id}"))
    }
    async fn get_edge(&self, _r: Request<proto::GetEdgeRequest>) -> Result<Response<proto::Edge>, Status> {
        Err(pending("get_edge", "GET /v1/tenants/{tenant_id}/graph/edges/{edge_id}"))
    }
    async fn query_nodes(&self, _r: Request<proto::QueryNodesRequest>) -> Result<Response<proto::NodeList>, Status> {
        Err(pending("query_nodes", "POST /v1/tenants/{tenant_id}/graph/nodes/query"))
    }
    async fn neighbors(&self, _r: Request<proto::NeighborsRequest>) -> Result<Response<proto::NeighborList>, Status> {
        Err(pending("neighbors", "POST /v1/tenants/{tenant_id}/graph/neighbors"))
    }

    // ====================================================================
    // Bulk ingest — PENDING
    // ====================================================================

    async fn bulk_insert_nodes(&self, _r: Request<proto::BulkNodesRequest>) -> Result<Response<proto::BulkInsertResponse>, Status> {
        Err(pending("bulk_insert_nodes", "POST /v1/tenants/{tenant_id}/graph/bulk/nodes"))
    }
    async fn bulk_insert_edges(&self, _r: Request<proto::BulkEdgesRequest>) -> Result<Response<proto::BulkInsertResponse>, Status> {
        Err(pending("bulk_insert_edges", "POST /v1/tenants/{tenant_id}/graph/bulk/edges"))
    }

    // ====================================================================
    // Vector search — PENDING
    // ====================================================================

    async fn vector_search(&self, _r: Request<proto::VectorSearchRequest>) -> Result<Response<proto::VectorSearchResponse>, Status> {
        Err(pending("vector_search", "POST /v1/tenants/{tenant_id}/graph/vector/search"))
    }
    async fn vector_hybrid_search(&self, _r: Request<proto::VectorHybridRequest>) -> Result<Response<proto::VectorSearchResponse>, Status> {
        Err(pending("vector_hybrid_search", "POST /v1/tenants/{tenant_id}/graph/vector/hybrid"))
    }
    async fn designate_vector_property(&self, _r: Request<proto::DesignateVectorRequest>) -> Result<Response<proto::DesignateAck>, Status> {
        Err(pending("designate_vector_property", "POST /v1/tenants/{tenant_id}/graph/vector/designate"))
    }

    // ====================================================================
    // Full-text search — PENDING
    // ====================================================================

    async fn fulltext_search(&self, _r: Request<proto::FulltextSearchRequest>) -> Result<Response<proto::FulltextSearchResponse>, Status> {
        Err(pending("fulltext_search", "POST /v1/tenants/{tenant_id}/graph/fulltext/search"))
    }
    async fn designate_fulltext_property(&self, _r: Request<proto::DesignateFulltextRequest>) -> Result<Response<proto::DesignateAck>, Status> {
        Err(pending("designate_fulltext_property", "POST /v1/tenants/{tenant_id}/graph/fulltext/designate"))
    }

    // ====================================================================
    // Spatial — PENDING
    // ====================================================================

    async fn spatial_radius(&self, _r: Request<proto::SpatialRadiusRequest>) -> Result<Response<proto::SpatialResponse>, Status> {
        Err(pending("spatial_radius", "POST /v1/tenants/{tenant_id}/graph/spatial/radius"))
    }
    async fn spatial_bounding_box(&self, _r: Request<proto::SpatialBboxRequest>) -> Result<Response<proto::SpatialResponse>, Status> {
        Err(pending("spatial_bounding_box", "POST /v1/tenants/{tenant_id}/graph/spatial/bbox"))
    }
    async fn designate_spatial_property(&self, _r: Request<proto::DesignateSpatialRequest>) -> Result<Response<proto::DesignateAck>, Status> {
        Err(pending("designate_spatial_property", "POST /v1/tenants/{tenant_id}/graph/spatial/designate"))
    }

    // ====================================================================
    // Epistemic traversal — PENDING
    // ====================================================================

    async fn epistemic_neighbors(&self, _r: Request<proto::EpistemicNeighborsRequest>) -> Result<Response<proto::EpistemicNeighborsResponse>, Status> {
        Err(pending("epistemic_neighbors", "POST /v1/tenants/{tenant_id}/graph/epistemic-neighbors"))
    }

    // ====================================================================
    // Graph algorithms — PENDING
    // ====================================================================

    async fn personalized_page_rank(&self, _r: Request<proto::PprRequest>) -> Result<Response<proto::PprResponse>, Status> {
        Err(pending("personalized_page_rank", "POST /v1/tenants/{tenant_id}/graph/algorithms/ppr"))
    }
    async fn page_rank(&self, _r: Request<proto::PageRankRequest>) -> Result<Response<proto::PageRankResponse>, Status> {
        Err(pending("page_rank", "POST /v1/tenants/{tenant_id}/graph/algorithms/pagerank"))
    }
    async fn connected_components(&self, _r: Request<proto::ComponentsRequest>) -> Result<Response<proto::ComponentsResponse>, Status> {
        Err(pending("connected_components", "POST /v1/tenants/{tenant_id}/graph/algorithms/components"))
    }
    async fn communities(&self, _r: Request<proto::CommunitiesRequest>) -> Result<Response<proto::CommunitiesResponse>, Status> {
        Err(pending("communities", "POST /v1/tenants/{tenant_id}/graph/algorithms/communities"))
    }

    // ====================================================================
    // Stats + diagnostics — graph_verify + rebuild_indexes PENDING
    // ====================================================================

    async fn graph_verify(&self, _r: Request<proto::GraphVerifyRequest>) -> Result<Response<proto::VerifyReport>, Status> {
        Err(pending("graph_verify", "GET /v1/tenants/{tenant_id}/graph/verify"))
    }
    async fn rebuild_indexes(&self, _r: Request<proto::RebuildIndexesRequest>) -> Result<Response<proto::RebuildIndexesResponse>, Status> {
        Err(pending("rebuild_indexes", "POST /v1/tenants/{tenant_id}/graph/rebuild-indexes"))
    }

    // ====================================================================
    // Cache — PENDING
    // ====================================================================

    async fn cache_put(&self, _r: Request<proto::CachePutRequest>) -> Result<Response<proto::CacheAck>, Status> {
        Err(pending("cache_put", "POST /v1/cache/put"))
    }
    async fn cache_get(&self, _r: Request<proto::CacheGetRequest>) -> Result<Response<proto::CacheGetResponse>, Status> {
        Err(pending("cache_get", "POST /v1/cache/get"))
    }
    async fn cache_check(&self, _r: Request<proto::CacheCheckRequest>) -> Result<Response<proto::CacheCheckResponse>, Status> {
        Err(pending("cache_check", "POST /v1/cache/check"))
    }
    async fn cache_explain(&self, _r: Request<proto::CacheCheckRequest>) -> Result<Response<proto::CacheExplainResponse>, Status> {
        Err(pending("cache_explain", "POST /v1/cache/explain"))
    }
    async fn cache_invalidate(&self, _r: Request<proto::CacheInvalidateRequest>) -> Result<Response<proto::CacheAck>, Status> {
        Err(pending("cache_invalidate", "POST /v1/cache/invalidate"))
    }
    async fn cache_stats(&self, _r: Request<proto::CacheStatsRequest>) -> Result<Response<proto::CacheStatsResponse>, Status> {
        Err(pending("cache_stats", "POST /v1/cache/stats"))
    }
}
