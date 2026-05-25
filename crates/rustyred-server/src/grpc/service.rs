// GraphDatabaseService — tonic-side implementation of rustyred.v1.GraphDatabase.
//
// This file mirrors the HTTP routes defined in router.rs but exposes them
// over gRPC. The MAJORITY of methods in this first commit are stubbed with
// `Status::unimplemented` and a comment naming the HTTP route they should
// mirror; filling them in is mechanical (parse gRPC request → call the
// existing handler logic → map the response into a gRPC response message).
//
// The methods implemented first are the "vital signs" plus the read-only
// graph primitives that make gRPC useful for symmetric agent/tool exposure:
//
//   - Health        — does the server respond at all?
//   - Ready         — does the server pass its readiness check?
//   - GraphStats    — can we read the graph state through gRPC?
//   - GetNode/GetEdge/QueryNodes/Neighbors — can clients inspect graph state?
//
// Subsequent commits fill in the remaining write/search/cache methods. Each
// todo points at the existing axum route handler so the mapping work is bounded.

use std::collections::{BTreeMap, HashMap};

use rustyred_core::{
    Direction, EdgeRecord, GraphStoreError, NeighborHit, NeighborQuery, NodeQuery, NodeRecord,
};
use serde_json::{Number, Value};
use tonic::{Request, Response, Status};

use super::proto;
use super::proto::graph_database_server::GraphDatabase;
use crate::state::{AppState, TenantGraphStore};

#[derive(Clone)]
pub struct GraphDatabaseService {
    state: AppState,
}

impl GraphDatabaseService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn tenant_store(&self, tenant_id: &str) -> Result<TenantGraphStore, Status> {
        self.state.tenant_graph_store(tenant_id).map_err(|err| {
            Status::failed_precondition(format!(
                "tenant store unavailable: {}: {}",
                err.code, err.message
            ))
        })
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

fn graph_store_status(operation: &str, error: GraphStoreError) -> Status {
    Status::internal(format!("{operation}: {}: {}", error.code, error.message))
}

fn node_to_proto(node: NodeRecord) -> proto::Node {
    proto::Node {
        id: node.id,
        labels: node.labels,
        properties: Some(properties_to_proto(&node.properties)),
    }
}

fn edge_to_proto(edge: EdgeRecord) -> proto::Edge {
    proto::Edge {
        id: edge.id,
        r#type: edge.edge_type,
        source_id: edge.from_id,
        target_id: edge.to_id,
        properties: Some(properties_to_proto(&edge.properties)),
    }
}

fn properties_to_proto(value: &Value) -> proto::PropertyMap {
    let properties = value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), property_to_proto(value)))
                .collect()
        })
        .unwrap_or_else(HashMap::new);
    proto::PropertyMap { properties }
}

fn property_to_proto(value: &Value) -> proto::Property {
    use proto::property::Value as ProtoValue;

    let value = match value {
        Value::String(value) => ProtoValue::StringVal(value.clone()),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                ProtoValue::IntVal(value)
            } else if let Some(value) = value.as_f64() {
                ProtoValue::DoubleVal(value)
            } else {
                ProtoValue::JsonVal(value.to_string())
            }
        }
        Value::Bool(value) => ProtoValue::BoolVal(*value),
        Value::Array(_) | Value::Object(_) | Value::Null => ProtoValue::JsonVal(value.to_string()),
    };
    proto::Property { value: Some(value) }
}

fn query_properties_from_proto(properties: Option<proto::PropertyMap>) -> BTreeMap<String, Value> {
    properties
        .map(|properties| {
            properties
                .properties
                .into_iter()
                .map(|(key, value)| (key, value_from_proto_property(value)))
                .collect()
        })
        .unwrap_or_default()
}

fn value_from_proto_property(property: proto::Property) -> Value {
    use proto::property::Value as ProtoValue;

    match property.value {
        Some(ProtoValue::StringVal(value)) => Value::String(value),
        Some(ProtoValue::IntVal(value)) => Value::Number(value.into()),
        Some(ProtoValue::DoubleVal(value)) => Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(ProtoValue::BoolVal(value)) => Value::Bool(value),
        Some(ProtoValue::BytesVal(value)) => Value::Array(
            value
                .into_iter()
                .map(|byte| Value::Number(u64::from(byte).into()))
                .collect(),
        ),
        Some(ProtoValue::JsonVal(value)) => {
            serde_json::from_str(&value).unwrap_or(Value::String(value))
        }
        None => Value::Null,
    }
}

fn neighbor_to_proto(
    store: &TenantGraphStore,
    hit: NeighborHit,
) -> Result<Option<proto::NeighborHit>, Status> {
    let node = store
        .get_node(&hit.node_id)
        .map_err(|err| graph_store_status("neighbors.get_node", err))?;
    let edge = store
        .get_edge(&hit.edge_id)
        .map_err(|err| graph_store_status("neighbors.get_edge", err))?;
    let (Some(node), Some(edge)) = (node, edge) else {
        return Ok(None);
    };

    Ok(Some(proto::NeighborHit {
        node: Some(node_to_proto(node)),
        edge: Some(edge_to_proto(edge)),
        score: hit.confidence.unwrap_or(1.0),
    }))
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
        let store = self.tenant_store(&tenant_id)?;
        let stats = store
            .stats()
            .map_err(|err| graph_store_status("graph_stats", err))?;
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

    async fn query(
        &self,
        _r: Request<proto::QueryRequest>,
    ) -> Result<Response<proto::QueryResponse>, Status> {
        Err(pending("query", "POST /v1/query"))
    }
    async fn cypher(
        &self,
        _r: Request<proto::CypherRequest>,
    ) -> Result<Response<proto::CypherResponse>, Status> {
        Err(pending("cypher", "POST /v1/cypher"))
    }
    async fn cypher_explain(
        &self,
        _r: Request<proto::CypherRequest>,
    ) -> Result<Response<proto::CypherExplainResponse>, Status> {
        Err(pending("cypher_explain", "POST /v1/cypher/explain"))
    }

    // ====================================================================
    // Transactions — PENDING
    // ====================================================================

    async fn begin_transaction(
        &self,
        _r: Request<proto::BeginTxnRequest>,
    ) -> Result<Response<proto::BeginTxnResponse>, Status> {
        Err(pending("begin_transaction", "POST /v1/transactions/begin"))
    }
    async fn commit_transaction(
        &self,
        _r: Request<proto::CommitTxnRequest>,
    ) -> Result<Response<proto::CommitTxnResponse>, Status> {
        Err(pending(
            "commit_transaction",
            "POST /v1/transactions/commit",
        ))
    }
    async fn rollback_transaction(
        &self,
        _r: Request<proto::RollbackTxnRequest>,
    ) -> Result<Response<proto::RollbackTxnResponse>, Status> {
        Err(pending(
            "rollback_transaction",
            "POST /v1/transactions/rollback",
        ))
    }

    // ====================================================================
    // Node + edge primitives — PENDING
    // ====================================================================

    async fn upsert_node(
        &self,
        _r: Request<proto::UpsertNodeRequest>,
    ) -> Result<Response<proto::Node>, Status> {
        Err(pending(
            "upsert_node",
            "POST /v1/tenants/{tenant_id}/graph/nodes",
        ))
    }
    async fn upsert_edge(
        &self,
        _r: Request<proto::UpsertEdgeRequest>,
    ) -> Result<Response<proto::Edge>, Status> {
        Err(pending(
            "upsert_edge",
            "POST /v1/tenants/{tenant_id}/graph/edges",
        ))
    }
    async fn get_node(
        &self,
        request: Request<proto::GetNodeRequest>,
    ) -> Result<Response<proto::Node>, Status> {
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let node = store
            .get_node(&request.node_id)
            .map_err(|err| graph_store_status("get_node", err))?
            .ok_or_else(|| Status::not_found(format!("node not found: {}", request.node_id)))?;
        Ok(Response::new(node_to_proto(node)))
    }
    async fn get_edge(
        &self,
        request: Request<proto::GetEdgeRequest>,
    ) -> Result<Response<proto::Edge>, Status> {
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let edge = store
            .get_edge(&request.edge_id)
            .map_err(|err| graph_store_status("get_edge", err))?
            .ok_or_else(|| Status::not_found(format!("edge not found: {}", request.edge_id)))?;
        Ok(Response::new(edge_to_proto(edge)))
    }
    async fn query_nodes(
        &self,
        request: Request<proto::QueryNodesRequest>,
    ) -> Result<Response<proto::NodeList>, Status> {
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let limit = (request.limit > 0).then_some(request.limit as usize);
        let properties = query_properties_from_proto(request.property_filter);
        let mut nodes_by_id = BTreeMap::new();

        if request.labels.is_empty() {
            let nodes = store
                .query_nodes(NodeQuery {
                    label: None,
                    properties,
                    limit,
                })
                .map_err(|err| graph_store_status("query_nodes", err))?;
            for node in nodes {
                nodes_by_id.insert(node.id.clone(), node);
            }
        } else {
            for label in request.labels {
                let nodes = store
                    .query_nodes(NodeQuery {
                        label: Some(label),
                        properties: properties.clone(),
                        limit,
                    })
                    .map_err(|err| graph_store_status("query_nodes", err))?;
                for node in nodes {
                    nodes_by_id.entry(node.id.clone()).or_insert(node);
                    if limit.is_some_and(|limit| nodes_by_id.len() >= limit) {
                        break;
                    }
                }
                if limit.is_some_and(|limit| nodes_by_id.len() >= limit) {
                    break;
                }
            }
        }

        let nodes = nodes_by_id
            .into_values()
            .take(limit.unwrap_or(usize::MAX))
            .map(node_to_proto)
            .collect();
        Ok(Response::new(proto::NodeList { nodes }))
    }
    async fn neighbors(
        &self,
        request: Request<proto::NeighborsRequest>,
    ) -> Result<Response<proto::NeighborList>, Status> {
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let limit = (request.limit > 0).then_some(request.limit as usize);
        let directions = match proto::neighbors_request::Direction::try_from(request.direction)
            .unwrap_or(proto::neighbors_request::Direction::Unspecified)
        {
            proto::neighbors_request::Direction::In => vec![Direction::In],
            proto::neighbors_request::Direction::Out
            | proto::neighbors_request::Direction::Unspecified => vec![Direction::Out],
            proto::neighbors_request::Direction::Both => vec![Direction::Out, Direction::In],
        };
        let edge_types: Vec<Option<String>> = if request.edge_types.is_empty() {
            vec![None]
        } else {
            request.edge_types.into_iter().map(Some).collect()
        };
        let mut neighbors_by_key = BTreeMap::new();

        for direction in directions {
            for edge_type in &edge_types {
                let hits = store
                    .neighbors(NeighborQuery {
                        node_id: request.node_id.clone(),
                        direction: direction.clone(),
                        edge_type: edge_type.clone(),
                    })
                    .map_err(|err| graph_store_status("neighbors", err))?;
                for hit in hits {
                    let key = format!("{}:{}", hit.edge_id, hit.node_id);
                    if let Some(neighbor) = neighbor_to_proto(&store, hit)? {
                        neighbors_by_key.entry(key).or_insert(neighbor);
                    }
                    if limit.is_some_and(|limit| neighbors_by_key.len() >= limit) {
                        break;
                    }
                }
                if limit.is_some_and(|limit| neighbors_by_key.len() >= limit) {
                    break;
                }
            }
            if limit.is_some_and(|limit| neighbors_by_key.len() >= limit) {
                break;
            }
        }

        let neighbors = neighbors_by_key
            .into_values()
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        Ok(Response::new(proto::NeighborList { neighbors }))
    }

    // ====================================================================
    // Bulk ingest — PENDING
    // ====================================================================

    async fn bulk_insert_nodes(
        &self,
        _r: Request<proto::BulkNodesRequest>,
    ) -> Result<Response<proto::BulkInsertResponse>, Status> {
        Err(pending(
            "bulk_insert_nodes",
            "POST /v1/tenants/{tenant_id}/graph/bulk/nodes",
        ))
    }
    async fn bulk_insert_edges(
        &self,
        _r: Request<proto::BulkEdgesRequest>,
    ) -> Result<Response<proto::BulkInsertResponse>, Status> {
        Err(pending(
            "bulk_insert_edges",
            "POST /v1/tenants/{tenant_id}/graph/bulk/edges",
        ))
    }

    // ====================================================================
    // Vector search — PENDING
    // ====================================================================

    async fn vector_search(
        &self,
        _r: Request<proto::VectorSearchRequest>,
    ) -> Result<Response<proto::VectorSearchResponse>, Status> {
        Err(pending(
            "vector_search",
            "POST /v1/tenants/{tenant_id}/graph/vector/search",
        ))
    }
    async fn vector_hybrid_search(
        &self,
        _r: Request<proto::VectorHybridRequest>,
    ) -> Result<Response<proto::VectorSearchResponse>, Status> {
        Err(pending(
            "vector_hybrid_search",
            "POST /v1/tenants/{tenant_id}/graph/vector/hybrid",
        ))
    }
    async fn designate_vector_property(
        &self,
        _r: Request<proto::DesignateVectorRequest>,
    ) -> Result<Response<proto::DesignateAck>, Status> {
        Err(pending(
            "designate_vector_property",
            "POST /v1/tenants/{tenant_id}/graph/vector/designate",
        ))
    }

    // ====================================================================
    // Full-text search — PENDING
    // ====================================================================

    async fn fulltext_search(
        &self,
        _r: Request<proto::FulltextSearchRequest>,
    ) -> Result<Response<proto::FulltextSearchResponse>, Status> {
        Err(pending(
            "fulltext_search",
            "POST /v1/tenants/{tenant_id}/graph/fulltext/search",
        ))
    }
    async fn designate_fulltext_property(
        &self,
        _r: Request<proto::DesignateFulltextRequest>,
    ) -> Result<Response<proto::DesignateAck>, Status> {
        Err(pending(
            "designate_fulltext_property",
            "POST /v1/tenants/{tenant_id}/graph/fulltext/designate",
        ))
    }

    // ====================================================================
    // Spatial — PENDING
    // ====================================================================

    async fn spatial_radius(
        &self,
        _r: Request<proto::SpatialRadiusRequest>,
    ) -> Result<Response<proto::SpatialResponse>, Status> {
        Err(pending(
            "spatial_radius",
            "POST /v1/tenants/{tenant_id}/graph/spatial/radius",
        ))
    }
    async fn spatial_bounding_box(
        &self,
        _r: Request<proto::SpatialBboxRequest>,
    ) -> Result<Response<proto::SpatialResponse>, Status> {
        Err(pending(
            "spatial_bounding_box",
            "POST /v1/tenants/{tenant_id}/graph/spatial/bbox",
        ))
    }
    async fn designate_spatial_property(
        &self,
        _r: Request<proto::DesignateSpatialRequest>,
    ) -> Result<Response<proto::DesignateAck>, Status> {
        Err(pending(
            "designate_spatial_property",
            "POST /v1/tenants/{tenant_id}/graph/spatial/designate",
        ))
    }

    // ====================================================================
    // Epistemic traversal — PENDING
    // ====================================================================

    async fn epistemic_neighbors(
        &self,
        _r: Request<proto::EpistemicNeighborsRequest>,
    ) -> Result<Response<proto::EpistemicNeighborsResponse>, Status> {
        Err(pending(
            "epistemic_neighbors",
            "POST /v1/tenants/{tenant_id}/graph/epistemic-neighbors",
        ))
    }

    // ====================================================================
    // Graph algorithms — PENDING
    // ====================================================================

    async fn personalized_page_rank(
        &self,
        _r: Request<proto::PprRequest>,
    ) -> Result<Response<proto::PprResponse>, Status> {
        Err(pending(
            "personalized_page_rank",
            "POST /v1/tenants/{tenant_id}/graph/algorithms/ppr",
        ))
    }
    async fn page_rank(
        &self,
        _r: Request<proto::PageRankRequest>,
    ) -> Result<Response<proto::PageRankResponse>, Status> {
        Err(pending(
            "page_rank",
            "POST /v1/tenants/{tenant_id}/graph/algorithms/pagerank",
        ))
    }
    async fn connected_components(
        &self,
        _r: Request<proto::ComponentsRequest>,
    ) -> Result<Response<proto::ComponentsResponse>, Status> {
        Err(pending(
            "connected_components",
            "POST /v1/tenants/{tenant_id}/graph/algorithms/components",
        ))
    }
    async fn communities(
        &self,
        _r: Request<proto::CommunitiesRequest>,
    ) -> Result<Response<proto::CommunitiesResponse>, Status> {
        Err(pending(
            "communities",
            "POST /v1/tenants/{tenant_id}/graph/algorithms/communities",
        ))
    }

    // ====================================================================
    // Stats + diagnostics — graph_verify + rebuild_indexes PENDING
    // ====================================================================

    async fn graph_verify(
        &self,
        _r: Request<proto::GraphVerifyRequest>,
    ) -> Result<Response<proto::VerifyReport>, Status> {
        Err(pending(
            "graph_verify",
            "GET /v1/tenants/{tenant_id}/graph/verify",
        ))
    }
    async fn rebuild_indexes(
        &self,
        _r: Request<proto::RebuildIndexesRequest>,
    ) -> Result<Response<proto::RebuildIndexesResponse>, Status> {
        Err(pending(
            "rebuild_indexes",
            "POST /v1/tenants/{tenant_id}/graph/rebuild-indexes",
        ))
    }

    // ====================================================================
    // Cache — PENDING
    // ====================================================================

    async fn cache_put(
        &self,
        _r: Request<proto::CachePutRequest>,
    ) -> Result<Response<proto::CacheAck>, Status> {
        Err(pending("cache_put", "POST /v1/cache/put"))
    }
    async fn cache_get(
        &self,
        _r: Request<proto::CacheGetRequest>,
    ) -> Result<Response<proto::CacheGetResponse>, Status> {
        Err(pending("cache_get", "POST /v1/cache/get"))
    }
    async fn cache_check(
        &self,
        _r: Request<proto::CacheCheckRequest>,
    ) -> Result<Response<proto::CacheCheckResponse>, Status> {
        Err(pending("cache_check", "POST /v1/cache/check"))
    }
    async fn cache_explain(
        &self,
        _r: Request<proto::CacheCheckRequest>,
    ) -> Result<Response<proto::CacheExplainResponse>, Status> {
        Err(pending("cache_explain", "POST /v1/cache/explain"))
    }
    async fn cache_invalidate(
        &self,
        _r: Request<proto::CacheInvalidateRequest>,
    ) -> Result<Response<proto::CacheAck>, Status> {
        Err(pending("cache_invalidate", "POST /v1/cache/invalidate"))
    }
    async fn cache_stats(
        &self,
        _r: Request<proto::CacheStatsRequest>,
    ) -> Result<Response<proto::CacheStatsResponse>, Status> {
        Err(pending("cache_stats", "POST /v1/cache/stats"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rustyred_core::{EdgeRecord, HybridScoringConfig, NodeRecord, RedCoreDurability};
    use serde_json::json;

    use super::proto::graph_database_server::GraphDatabase;
    use super::*;
    use crate::config::{Config, StorageMode};

    fn test_config() -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::AofEverysec,
            snapshot_interval_writes: 1_000,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "read_committed".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: BTreeMap::new(),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: HybridScoringConfig::default(),
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

    fn service_with_graph() -> GraphDatabaseService {
        let state = AppState::new(test_config());
        let mut store = state.tenant_graph_store("smoke").unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["Person"],
                json!({"name": "Ada"}),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:b",
                ["Person", "Engineer"],
                json!({"name": "Grace"}),
            ))
            .unwrap();
        store
            .upsert_edge(
                EdgeRecord::new(
                    "edge:ab",
                    "node:a",
                    "KNOWS",
                    "node:b",
                    json!({"since": 1952}),
                )
                .with_confidence(0.82),
            )
            .unwrap();
        GraphDatabaseService::new(state)
    }

    #[tokio::test]
    async fn grpc_read_primitives_return_graph_state() {
        let service = service_with_graph();

        let node = service
            .get_node(Request::new(proto::GetNodeRequest {
                tenant_id: "smoke".to_string(),
                node_id: "node:a".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(node.id, "node:a");
        assert_eq!(
            node.properties.unwrap().properties["name"].value,
            Some(proto::property::Value::StringVal("Ada".to_string()))
        );

        let edge = service
            .get_edge(Request::new(proto::GetEdgeRequest {
                tenant_id: "smoke".to_string(),
                edge_id: "edge:ab".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(edge.source_id, "node:a");
        assert_eq!(edge.target_id, "node:b");

        let nodes = service
            .query_nodes(Request::new(proto::QueryNodesRequest {
                tenant_id: "smoke".to_string(),
                labels: vec!["Engineer".to_string()],
                property_filter: None,
                limit: 10,
                cursor: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(nodes.nodes.len(), 1);
        assert_eq!(nodes.nodes[0].id, "node:b");

        let neighbors = service
            .neighbors(Request::new(proto::NeighborsRequest {
                tenant_id: "smoke".to_string(),
                node_id: "node:a".to_string(),
                direction: proto::neighbors_request::Direction::Out as i32,
                edge_types: vec!["KNOWS".to_string()],
                limit: 10,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(neighbors.neighbors.len(), 1);
        assert_eq!(neighbors.neighbors[0].node.as_ref().unwrap().id, "node:b");
        assert_eq!(neighbors.neighbors[0].edge.as_ref().unwrap().id, "edge:ab");
        assert_eq!(neighbors.neighbors[0].score, 0.82);
    }

    #[tokio::test]
    async fn grpc_get_node_reports_not_found() {
        let service = service_with_graph();

        let error = service
            .get_node(Request::new(proto::GetNodeRequest {
                tenant_id: "smoke".to_string(),
                node_id: "node:missing".to_string(),
            }))
            .await
            .unwrap_err();

        assert_eq!(error.code(), tonic::Code::NotFound);
    }
}
