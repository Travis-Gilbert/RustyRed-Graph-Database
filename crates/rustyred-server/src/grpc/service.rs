// GraphDatabaseService — tonic-side implementation of rustyred.v1.GraphDatabase.
//
// This file mirrors the HTTP routes defined in router.rs but exposes them over
// gRPC. The implementation intentionally calls the same state, graph store,
// auth, query, and cache primitives as the HTTP surface so clients get symmetric
// behavior regardless of transport.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use http::{HeaderMap, HeaderValue, StatusCode};
use rustyred_core::{
    connected_components, label_propagation_communities, pagerank, personalized_pagerank,
    Direction, EdgeRecord, EpistemicType, GraphMutation, GraphMutationBatch, GraphStoreError,
    NeighborHit, NeighborQuery, NodeQuery, NodeRecord,
};
use serde_json::{json, Number, Value};
use tonic::{Request, Response, Status};

use super::proto;
use super::proto::graph_database_server::GraphDatabase;
use crate::auth::require_scope;
use crate::graph_cache::{GraphCacheInvalidateBody, GraphCacheLookupBody, GraphCachePutBody};
use crate::query_surface::{
    execute_cypher_query, explain_cypher_query, parse_tx_cypher_mutations, PublicCypherBody,
    QuerySurfaceError,
};
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

    fn require_request_scope<T>(
        &self,
        request: &Request<T>,
        required_scope: &str,
    ) -> Result<(), Status> {
        let mut headers = HeaderMap::new();
        if let Some(value) = request.metadata().get("authorization") {
            let value = value.to_str().map_err(|_| {
                Status::unauthenticated("authorization metadata must be valid ASCII")
            })?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| Status::unauthenticated("authorization metadata is invalid"))?;
            headers.insert(http::header::AUTHORIZATION, value);
        }
        require_scope(
            &headers,
            &self.state.config.api_tokens,
            required_scope,
            self.state.config.require_auth,
        )
        .map(|_| ())
        .map_err(auth_status_to_grpc)
    }
}

fn graph_store_status(operation: &str, error: GraphStoreError) -> Status {
    Status::internal(format!("{operation}: {}: {}", error.code, error.message))
}

fn auth_status_to_grpc(status: StatusCode) -> Status {
    match status {
        StatusCode::UNAUTHORIZED => Status::unauthenticated("missing or invalid bearer token"),
        StatusCode::FORBIDDEN => Status::permission_denied("token lacks the required scope"),
        other => Status::internal(format!("unexpected auth status: {other}")),
    }
}

fn state_status(operation: &str, error: crate::state::StoreAccessError) -> Status {
    Status::failed_precondition(format!("{operation}: {}: {}", error.code, error.message))
}

fn query_surface_status(operation: &str, error: QuerySurfaceError) -> Status {
    let payload = error.payload();
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("query surface error");
    match error.status() {
        StatusCode::BAD_REQUEST => Status::invalid_argument(format!("{operation}: {message}")),
        StatusCode::UNAUTHORIZED => Status::unauthenticated(format!("{operation}: {message}")),
        StatusCode::FORBIDDEN => Status::permission_denied(format!("{operation}: {message}")),
        StatusCode::NOT_FOUND => Status::not_found(format!("{operation}: {message}")),
        StatusCode::SERVICE_UNAVAILABLE => Status::unavailable(format!("{operation}: {message}")),
        _ => Status::internal(format!("{operation}: {message}")),
    }
}

fn json_result_count(value: &Value) -> u32 {
    value
        .get("row_count")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get("stats")
                .and_then(|stats| stats.get("returned"))
                .and_then(Value::as_u64)
        })
        .or_else(|| {
            value
                .get("rows")
                .and_then(Value::as_array)
                .map(|rows| rows.len() as u64)
        })
        .or_else(|| {
            value
                .get("nodes")
                .and_then(Value::as_array)
                .map(|nodes| nodes.len() as u64)
        })
        .unwrap_or_default() as u32
}

fn grpc_cache_lookup(cache_key: String) -> GraphCacheLookupBody {
    GraphCacheLookupBody {
        tenant_id: None,
        kind: "query_result".to_string(),
        key: Value::String(cache_key),
        index_manifest_hash: None,
        auth_scope_hash: None,
        retrieval_policy_hash: None,
        model_version: Some("grpc.v1".to_string()),
        source_hashes: Vec::new(),
    }
}

fn bytes_to_json(bytes: Vec<u8>) -> Value {
    Value::Array(
        bytes
            .into_iter()
            .map(|byte| Value::Number(u64::from(byte).into()))
            .collect(),
    )
}

fn json_to_bytes(value: Option<Value>) -> Result<Vec<u8>, Status> {
    let Some(Value::Array(bytes)) = value else {
        return Ok(Vec::new());
    };
    bytes
        .into_iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .ok_or_else(|| Status::internal("cached gRPC payload is not a byte array"))
        })
        .collect()
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
        .unwrap_or_default();
    proto::PropertyMap { properties }
}

fn node_from_proto(node: Option<proto::Node>) -> Result<NodeRecord, Status> {
    let node = node.ok_or_else(|| Status::invalid_argument("node is required"))?;
    if node.id.trim().is_empty() {
        return Err(Status::invalid_argument("node.id is required"));
    }
    Ok(NodeRecord::new(
        node.id,
        node.labels,
        properties_value_from_proto(node.properties),
    ))
}

fn edge_from_proto(edge: Option<proto::Edge>) -> Result<EdgeRecord, Status> {
    let edge = edge.ok_or_else(|| Status::invalid_argument("edge is required"))?;
    if edge.id.trim().is_empty() {
        return Err(Status::invalid_argument("edge.id is required"));
    }
    if edge.source_id.trim().is_empty() {
        return Err(Status::invalid_argument("edge.source_id is required"));
    }
    if edge.target_id.trim().is_empty() {
        return Err(Status::invalid_argument("edge.target_id is required"));
    }
    if edge.r#type.trim().is_empty() {
        return Err(Status::invalid_argument("edge.type is required"));
    }
    Ok(EdgeRecord::new(
        edge.id,
        edge.source_id,
        edge.r#type,
        edge.target_id,
        properties_value_from_proto(edge.properties),
    ))
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

fn properties_value_from_proto(properties: Option<proto::PropertyMap>) -> Value {
    Value::Object(
        properties
            .map(|properties| {
                properties
                    .properties
                    .into_iter()
                    .map(|(key, value)| (key, value_from_proto_property(value)))
                    .collect()
            })
            .unwrap_or_default(),
    )
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

fn vector_query_from_proto(values: Vec<f64>) -> Vec<f32> {
    values.into_iter().map(|value| value as f32).collect()
}

fn spatial_property_pair(property_name: &str) -> Result<(String, String), Status> {
    let separators = [',', ':', '/'];
    for separator in separators {
        if let Some((lat, lon)) = property_name.split_once(separator) {
            let lat = lat.trim();
            let lon = lon.trim();
            if !lat.is_empty() && !lon.is_empty() {
                return Ok((lat.to_string(), lon.to_string()));
            }
        }
    }
    Err(Status::invalid_argument(
        "spatial property_name must encode the latitude and longitude properties as \"lat,lon\"",
    ))
}

fn epistemic_types_from_proto(values: Vec<i32>) -> Result<Option<Vec<EpistemicType>>, Status> {
    let mut types = Vec::new();
    for value in values {
        let value = proto::EpistemicEdgeType::try_from(value)
            .map_err(|_| Status::invalid_argument("invalid epistemic edge type"))?;
        match value {
            proto::EpistemicEdgeType::EpistemicTypeUnspecified => {}
            proto::EpistemicEdgeType::EpistemicTypeSupports => types.push(EpistemicType::Supports),
            proto::EpistemicEdgeType::EpistemicTypeContradicts => {
                types.push(EpistemicType::Contradicts)
            }
            proto::EpistemicEdgeType::EpistemicTypeTension => types.push(EpistemicType::Tension),
            proto::EpistemicEdgeType::EpistemicTypeDerives => types.push(EpistemicType::Derives),
            proto::EpistemicEdgeType::EpistemicTypeCites => types.push(EpistemicType::Cites),
        }
    }
    Ok((!types.is_empty()).then_some(types))
}

fn epistemic_type_to_proto(value: Option<EpistemicType>) -> proto::EpistemicEdgeType {
    match value {
        Some(EpistemicType::Supports) => proto::EpistemicEdgeType::EpistemicTypeSupports,
        Some(EpistemicType::Contradicts) => proto::EpistemicEdgeType::EpistemicTypeContradicts,
        Some(EpistemicType::Tension) => proto::EpistemicEdgeType::EpistemicTypeTension,
        Some(EpistemicType::Derives) => proto::EpistemicEdgeType::EpistemicTypeDerives,
        Some(EpistemicType::Cites) => proto::EpistemicEdgeType::EpistemicTypeCites,
        None => proto::EpistemicEdgeType::EpistemicTypeUnspecified,
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
    // Native query surface
    // ====================================================================

    async fn query(
        &self,
        request: Request<proto::QueryRequest>,
    ) -> Result<Response<proto::QueryResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let graph_version = store
            .stats()
            .map_err(|err| graph_store_status("query.stats", err))?
            .version;
        let body: Value = serde_json::from_str(&request.body_json)
            .map_err(|err| Status::invalid_argument(format!("body_json must be JSON: {err}")))?;
        let payload = match proto::QueryKind::try_from(request.kind)
            .unwrap_or(proto::QueryKind::Unspecified)
        {
            proto::QueryKind::NodeMatch => {
                let query: NodeQuery = serde_json::from_value(body).map_err(|err| {
                    Status::invalid_argument(format!("node query body_json is invalid: {err}"))
                })?;
                let nodes = store
                    .query_nodes(query)
                    .map_err(|err| graph_store_status("query.node_match", err))?;
                serde_json::to_string(&nodes)
                    .map_err(|err| Status::internal(format!("serialize query nodes: {err}")))?
            }
            proto::QueryKind::Neighbors => {
                let query: NeighborQuery = serde_json::from_value(body).map_err(|err| {
                    Status::invalid_argument(format!("neighbor query body_json is invalid: {err}"))
                })?;
                let neighbors = store
                    .neighbors(query)
                    .map_err(|err| graph_store_status("query.neighbors", err))?;
                serde_json::to_string(&neighbors)
                    .map_err(|err| Status::internal(format!("serialize neighbors: {err}")))?
            }
            proto::QueryKind::Unspecified => {
                return Err(Status::invalid_argument("query kind is required"));
            }
        };
        let result_count = serde_json::from_str::<Value>(&payload)
            .ok()
            .and_then(|value| value.as_array().map(|array| array.len() as u32))
            .unwrap_or_default();
        Ok(Response::new(proto::QueryResponse {
            result_json: payload,
            result_count,
            graph_version,
        }))
    }
    async fn cypher(
        &self,
        request: Request<proto::CypherRequest>,
    ) -> Result<Response<proto::CypherResponse>, Status> {
        let needs_write_scope = !request.get_ref().transaction_id.trim().is_empty();
        self.require_request_scope(
            &request,
            if needs_write_scope {
                "graph:write"
            } else {
                "graph:read"
            },
        )?;
        let request = request.into_inner();
        let tenant_id = if request.tenant_id.trim().is_empty() {
            self.state.config.mcp_default_tenant.clone()
        } else {
            request.tenant_id
        };
        let params = if request.parameters_json.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str::<BTreeMap<String, Value>>(&request.parameters_json).map_err(
                |err| Status::invalid_argument(format!("parameters_json must be an object: {err}")),
            )?
        };
        let body = PublicCypherBody {
            tenant_id: Some(tenant_id.clone()),
            query: request.cypher,
            params,
            tx_id: (!request.transaction_id.trim().is_empty())
                .then_some(request.transaction_id.clone()),
        };
        let store = self.tenant_store(&tenant_id)?;
        let graph_version = store
            .stats()
            .map_err(|err| graph_store_status("cypher.stats", err))?
            .version;

        let payload = if let Some(tx_id) = body.tx_id.as_deref() {
            let mutations = parse_tx_cypher_mutations(&body.query, &body.params)
                .map_err(|err| query_surface_status("cypher.parse_tx", err))?;
            let staged_mutations = self
                .state
                .append_graph_transaction_mutations(&tenant_id, tx_id, mutations)
                .map_err(|err| state_status("cypher.stage_tx", err))?;
            json!({
                "ok": true,
                "tenant": tenant_id,
                "query": body.query,
                "tx_id": tx_id,
                "subset": "opencypher_v0_1_write_tx",
                "staged_mutations": staged_mutations,
            })
        } else {
            let mut store = store;
            self.state.observability.record_cypher();
            let start = Instant::now();
            let payload = execute_cypher_query(&mut store, &tenant_id, &body)
                .map_err(|err| query_surface_status("cypher", err))?;
            let nanos = start.elapsed().as_nanos() as u64;
            let detail = body.query.chars().take(120).collect::<String>();
            self.state
                .observability
                .record_query_timing("cypher", &detail, nanos, 0, 0);
            payload
        };

        let result_count = json_result_count(&payload);
        let result_json = serde_json::to_string(&payload)
            .map_err(|err| Status::internal(format!("serialize cypher result: {err}")))?;
        Ok(Response::new(proto::CypherResponse {
            result_json,
            result_count,
            graph_version,
        }))
    }
    async fn cypher_explain(
        &self,
        request: Request<proto::CypherRequest>,
    ) -> Result<Response<proto::CypherExplainResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let tenant_id = if request.tenant_id.trim().is_empty() {
            self.state.config.mcp_default_tenant.clone()
        } else {
            request.tenant_id
        };
        let params = if request.parameters_json.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str::<BTreeMap<String, Value>>(&request.parameters_json).map_err(
                |err| Status::invalid_argument(format!("parameters_json must be an object: {err}")),
            )?
        };
        let body = PublicCypherBody {
            tenant_id: Some(tenant_id.clone()),
            query: request.cypher,
            params,
            tx_id: None,
        };
        let plan = explain_cypher_query(&tenant_id, &body)
            .map_err(|err| query_surface_status("cypher_explain", err))?;
        let plan_json = serde_json::to_string(&plan)
            .map_err(|err| Status::internal(format!("serialize cypher plan: {err}")))?;
        Ok(Response::new(proto::CypherExplainResponse {
            plan_json,
            narrative: "OpenCypher compatibility plan generated by RustyRed.".to_string(),
        }))
    }

    // ====================================================================
    // Transactions
    // ====================================================================

    async fn begin_transaction(
        &self,
        request: Request<proto::BeginTxnRequest>,
    ) -> Result<Response<proto::BeginTxnResponse>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let transaction_id = self
            .state
            .begin_graph_transaction(&request.tenant_id)
            .map_err(|err| state_status("begin_transaction", err))?;
        let snapshot_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("begin_transaction.stats", err))?
            .version;
        self.state.observability.record_transaction_begin();
        Ok(Response::new(proto::BeginTxnResponse {
            transaction_id,
            snapshot_graph_version,
        }))
    }
    async fn commit_transaction(
        &self,
        request: Request<proto::CommitTxnRequest>,
    ) -> Result<Response<proto::CommitTxnResponse>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let transaction = self
            .state
            .commit_graph_transaction(&request.tenant_id, &request.transaction_id)
            .map_err(|err| state_status("commit_transaction", err))?;
        let store = self.tenant_store(&request.tenant_id)?;
        let mut nodes_written = 0u32;
        let mut edges_written = 0u32;
        for write in &transaction.writes {
            if store
                .get_node(&write.id)
                .map_err(|err| graph_store_status("commit_transaction.get_node", err))?
                .is_some()
            {
                nodes_written += 1;
            } else if store
                .get_edge(&write.id)
                .map_err(|err| graph_store_status("commit_transaction.get_edge", err))?
                .is_some()
            {
                edges_written += 1;
            }
        }
        self.state.observability.record_transaction_commit();
        Ok(Response::new(proto::CommitTxnResponse {
            new_graph_version: transaction.graph_version,
            nodes_written,
            edges_written,
        }))
    }
    async fn rollback_transaction(
        &self,
        request: Request<proto::RollbackTxnRequest>,
    ) -> Result<Response<proto::RollbackTxnResponse>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        self.state
            .rollback_graph_transaction(&request.tenant_id, &request.transaction_id)
            .map_err(|err| state_status("rollback_transaction", err))?;
        self.state.observability.record_transaction_rollback();
        Ok(Response::new(proto::RollbackTxnResponse {
            rolled_back: true,
        }))
    }

    // ====================================================================
    // Node + edge primitives
    // ====================================================================

    async fn upsert_node(
        &self,
        request: Request<proto::UpsertNodeRequest>,
    ) -> Result<Response<proto::Node>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let node = node_from_proto(request.node)?;
        if request.transaction_id.trim().is_empty() {
            let mut store = self.tenant_store(&request.tenant_id)?;
            store
                .upsert_node(node.clone())
                .map_err(|err| graph_store_status("upsert_node", err))?;
            self.state.observability.record_mutation();
            self.state
                .maybe_index_node_fulltext(&request.tenant_id, &node);
            self.state
                .maybe_index_node_spatially(&request.tenant_id, &node);
        } else {
            self.state
                .append_graph_transaction_mutations(
                    &request.tenant_id,
                    &request.transaction_id,
                    GraphMutationBatch::new([GraphMutation::NodeUpsert(node.clone())]),
                )
                .map_err(|err| state_status("upsert_node.stage", err))?;
        }
        Ok(Response::new(node_to_proto(node)))
    }
    async fn upsert_edge(
        &self,
        request: Request<proto::UpsertEdgeRequest>,
    ) -> Result<Response<proto::Edge>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let edge = edge_from_proto(request.edge)?;
        if request.transaction_id.trim().is_empty() {
            let mut store = self.tenant_store(&request.tenant_id)?;
            store
                .upsert_edge(edge.clone())
                .map_err(|err| graph_store_status("upsert_edge", err))?;
            self.state.observability.record_mutation();
        } else {
            self.state
                .append_graph_transaction_mutations(
                    &request.tenant_id,
                    &request.transaction_id,
                    GraphMutationBatch::new([GraphMutation::EdgeUpsert(edge.clone())]),
                )
                .map_err(|err| state_status("upsert_edge.stage", err))?;
        }
        Ok(Response::new(edge_to_proto(edge)))
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
    // Bulk ingest
    // ====================================================================

    async fn bulk_insert_nodes(
        &self,
        request: Request<proto::BulkNodesRequest>,
    ) -> Result<Response<proto::BulkInsertResponse>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let nodes = request
            .nodes
            .into_iter()
            .map(|node| node_from_proto(Some(node)))
            .collect::<Result<Vec<_>, _>>()?;
        let inserted_count = nodes.len() as u32;
        let graph_version = if request.transaction_id.trim().is_empty() {
            if nodes.is_empty() {
                self.tenant_store(&request.tenant_id)?
                    .stats()
                    .map_err(|err| graph_store_status("bulk_insert_nodes.stats", err))?
                    .version
            } else {
                let mut store = self.tenant_store(&request.tenant_id)?;
                let transaction = store
                    .commit_batch(GraphMutationBatch::new(
                        nodes.iter().cloned().map(GraphMutation::NodeUpsert),
                    ))
                    .map_err(|err| graph_store_status("bulk_insert_nodes", err))?;
                for node in &nodes {
                    self.state.observability.record_mutation();
                    self.state
                        .maybe_index_node_fulltext(&request.tenant_id, node);
                    self.state
                        .maybe_index_node_spatially(&request.tenant_id, node);
                }
                transaction.graph_version
            }
        } else if nodes.is_empty() {
            self.tenant_store(&request.tenant_id)?
                .stats()
                .map_err(|err| graph_store_status("bulk_insert_nodes.stats", err))?
                .version
        } else {
            self.state
                .append_graph_transaction_mutations(
                    &request.tenant_id,
                    &request.transaction_id,
                    GraphMutationBatch::new(nodes.into_iter().map(GraphMutation::NodeUpsert)),
                )
                .map_err(|err| state_status("bulk_insert_nodes.stage", err))?;
            self.tenant_store(&request.tenant_id)?
                .stats()
                .map_err(|err| graph_store_status("bulk_insert_nodes.stats", err))?
                .version
        };
        Ok(Response::new(proto::BulkInsertResponse {
            inserted_count,
            skipped_count: 0,
            skipped_ids: Vec::new(),
            new_graph_version: graph_version,
        }))
    }
    async fn bulk_insert_edges(
        &self,
        request: Request<proto::BulkEdgesRequest>,
    ) -> Result<Response<proto::BulkInsertResponse>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let edges = request
            .edges
            .into_iter()
            .map(|edge| edge_from_proto(Some(edge)))
            .collect::<Result<Vec<_>, _>>()?;
        let inserted_count = edges.len() as u32;
        let graph_version = if request.transaction_id.trim().is_empty() {
            if edges.is_empty() {
                self.tenant_store(&request.tenant_id)?
                    .stats()
                    .map_err(|err| graph_store_status("bulk_insert_edges.stats", err))?
                    .version
            } else {
                let mut store = self.tenant_store(&request.tenant_id)?;
                let transaction = store
                    .commit_batch(GraphMutationBatch::new(
                        edges.iter().cloned().map(GraphMutation::EdgeUpsert),
                    ))
                    .map_err(|err| graph_store_status("bulk_insert_edges", err))?;
                for _ in &edges {
                    self.state.observability.record_mutation();
                }
                transaction.graph_version
            }
        } else if edges.is_empty() {
            self.tenant_store(&request.tenant_id)?
                .stats()
                .map_err(|err| graph_store_status("bulk_insert_edges.stats", err))?
                .version
        } else {
            self.state
                .append_graph_transaction_mutations(
                    &request.tenant_id,
                    &request.transaction_id,
                    GraphMutationBatch::new(edges.into_iter().map(GraphMutation::EdgeUpsert)),
                )
                .map_err(|err| state_status("bulk_insert_edges.stage", err))?;
            self.tenant_store(&request.tenant_id)?
                .stats()
                .map_err(|err| graph_store_status("bulk_insert_edges.stats", err))?
                .version
        };
        Ok(Response::new(proto::BulkInsertResponse {
            inserted_count,
            skipped_count: 0,
            skipped_ids: Vec::new(),
            new_graph_version: graph_version,
        }))
    }

    // ====================================================================
    // Vector search
    // ====================================================================

    async fn vector_search(
        &self,
        request: Request<proto::VectorSearchRequest>,
    ) -> Result<Response<proto::VectorSearchResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let query = vector_query_from_proto(request.query_vector);
        let k = if request.k == 0 {
            10
        } else {
            request.k as usize
        };
        let store = self.tenant_store(&request.tenant_id)?;
        let hits = store
            .vector_search(Some(&request.label), &request.property_name, &query, k)
            .map_err(|err| graph_store_status("vector_search", err))?;
        let hits = hits
            .into_iter()
            .filter_map(|(node_id, distance)| {
                store
                    .get_node(&node_id)
                    .ok()
                    .flatten()
                    .map(|node| proto::VectorHit {
                        node: Some(node_to_proto(node)),
                        distance: distance as f64,
                        score: f64::from(1.0 - distance),
                    })
            })
            .collect();
        Ok(Response::new(proto::VectorSearchResponse { hits }))
    }
    async fn vector_hybrid_search(
        &self,
        request: Request<proto::VectorHybridRequest>,
    ) -> Result<Response<proto::VectorSearchResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let query = vector_query_from_proto(request.query_vector);
        let k = if request.k == 0 {
            10
        } else {
            request.k as usize
        };
        let graph_seeds = if request.seed_node_id.trim().is_empty() {
            Vec::new()
        } else {
            vec![request.seed_node_id.clone()]
        };
        let alpha = request.graph_proximity_weight.clamp(0.0, 1.0) as f32;
        let store = self.tenant_store(&request.tenant_id)?;
        let hits = store
            .hybrid_search(
                Some(&request.label),
                &request.property_name,
                &query,
                k,
                &graph_seeds,
                3,
                alpha,
            )
            .map_err(|err| graph_store_status("vector_hybrid_search", err))?;
        let hits = hits
            .into_iter()
            .filter_map(|(node_id, score)| {
                store
                    .get_node(&node_id)
                    .ok()
                    .flatten()
                    .map(|node| proto::VectorHit {
                        node: Some(node_to_proto(node)),
                        distance: f64::from(1.0 - score),
                        score: score as f64,
                    })
            })
            .collect();
        Ok(Response::new(proto::VectorSearchResponse { hits }))
    }
    async fn designate_vector_property(
        &self,
        request: Request<proto::DesignateVectorRequest>,
    ) -> Result<Response<proto::DesignateAck>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        store
            .designate_vector_property(
                &request.label,
                &request.property_name,
                request.dimension as usize,
            )
            .map_err(|err| graph_store_status("designate_vector_property", err))?;
        Ok(Response::new(proto::DesignateAck {
            designated: true,
            reason: String::new(),
        }))
    }

    // ====================================================================
    // Full-text search
    // ====================================================================

    async fn fulltext_search(
        &self,
        request: Request<proto::FulltextSearchRequest>,
    ) -> Result<Response<proto::FulltextSearchResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let limit = if request.limit == 0 {
            10
        } else {
            request.limit as usize
        };
        let label = (!request.label.trim().is_empty()).then_some(request.label.as_str());
        let results = self
            .state
            .fulltext_search(
                &request.tenant_id,
                label,
                &request.property_name,
                &request.query,
                limit,
            )
            .map_err(|err| state_status("fulltext_search", err))?;
        let store = self.tenant_store(&request.tenant_id)?;
        let hits = results
            .into_iter()
            .filter_map(|(node_id, score)| {
                store
                    .get_node(&node_id)
                    .ok()
                    .flatten()
                    .map(|node| proto::FulltextHit {
                        node: Some(node_to_proto(node)),
                        bm25_score: score as f64,
                        highlights: Vec::new(),
                    })
            })
            .collect();
        Ok(Response::new(proto::FulltextSearchResponse { hits }))
    }
    async fn designate_fulltext_property(
        &self,
        request: Request<proto::DesignateFulltextRequest>,
    ) -> Result<Response<proto::DesignateAck>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        self.state
            .designate_fulltext_property(&request.tenant_id, &request.label, &request.property_name)
            .map_err(|err| state_status("designate_fulltext_property", err))?;
        Ok(Response::new(proto::DesignateAck {
            designated: true,
            reason: String::new(),
        }))
    }

    // ====================================================================
    // Spatial
    // ====================================================================

    async fn spatial_radius(
        &self,
        request: Request<proto::SpatialRadiusRequest>,
    ) -> Result<Response<proto::SpatialResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let (lat_property, lon_property) = spatial_property_pair(&request.property_name)?;
        let limit = if request.limit == 0 {
            usize::MAX
        } else {
            request.limit as usize
        };
        let node_ids = self
            .state
            .spatial_radius_search(
                &request.tenant_id,
                &request.label,
                &lat_property,
                &lon_property,
                request.center_lat,
                request.center_lon,
                request.radius_meters / 1000.0,
            )
            .map_err(|err| state_status("spatial_radius", err))?;
        let store = self.tenant_store(&request.tenant_id)?;
        let hits = node_ids
            .into_iter()
            .take(limit)
            .filter_map(|node_id| {
                store
                    .get_node(&node_id)
                    .ok()
                    .flatten()
                    .map(|node| proto::SpatialHit {
                        node: Some(node_to_proto(node)),
                        distance_meters: 0.0,
                    })
            })
            .collect();
        Ok(Response::new(proto::SpatialResponse { hits }))
    }
    async fn spatial_bounding_box(
        &self,
        request: Request<proto::SpatialBboxRequest>,
    ) -> Result<Response<proto::SpatialResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let (lat_property, lon_property) = spatial_property_pair(&request.property_name)?;
        let limit = if request.limit == 0 {
            usize::MAX
        } else {
            request.limit as usize
        };
        let node_ids = self
            .state
            .spatial_bbox_search(
                &request.tenant_id,
                &request.label,
                &lat_property,
                &lon_property,
                request.min_lat,
                request.min_lon,
                request.max_lat,
                request.max_lon,
            )
            .map_err(|err| state_status("spatial_bounding_box", err))?;
        let store = self.tenant_store(&request.tenant_id)?;
        let hits = node_ids
            .into_iter()
            .take(limit)
            .filter_map(|node_id| {
                store
                    .get_node(&node_id)
                    .ok()
                    .flatten()
                    .map(|node| proto::SpatialHit {
                        node: Some(node_to_proto(node)),
                        distance_meters: 0.0,
                    })
            })
            .collect();
        Ok(Response::new(proto::SpatialResponse { hits }))
    }
    async fn designate_spatial_property(
        &self,
        request: Request<proto::DesignateSpatialRequest>,
    ) -> Result<Response<proto::DesignateAck>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let (lat_property, lon_property) = spatial_property_pair(&request.property_name)?;
        self.state
            .designate_spatial_property(
                &request.tenant_id,
                &request.label,
                &lat_property,
                &lon_property,
                9,
            )
            .map_err(|err| state_status("designate_spatial_property", err))?;
        Ok(Response::new(proto::DesignateAck {
            designated: true,
            reason: String::new(),
        }))
    }

    // ====================================================================
    // Epistemic traversal
    // ====================================================================

    async fn epistemic_neighbors(
        &self,
        request: Request<proto::EpistemicNeighborsRequest>,
    ) -> Result<Response<proto::EpistemicNeighborsResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let edge_types = epistemic_types_from_proto(request.edge_types)?;
        let max_depth = (request.max_hops > 0).then_some(request.max_hops as usize);
        let min_confidence = (request.min_confidence > 0.0).then_some(request.min_confidence);
        let store = self.tenant_store(&request.tenant_id)?;
        let hits = store
            .epistemic_neighbors(
                &request.seed_node_id,
                edge_types.as_deref(),
                min_confidence,
                max_depth,
            )
            .map_err(|err| graph_store_status("epistemic_neighbors", err))?
            .into_iter()
            .map(|(edge, node)| proto::EpistemicHit {
                node: Some(node_to_proto(node)),
                edge_type: epistemic_type_to_proto(edge.epistemic_type.clone()).into(),
                confidence: edge.effective_confidence(),
                hop_count: 1,
            })
            .collect();
        Ok(Response::new(proto::EpistemicNeighborsResponse { hits }))
    }

    // ====================================================================
    // Graph algorithms
    // ====================================================================

    async fn personalized_page_rank(
        &self,
        request: Request<proto::PprRequest>,
    ) -> Result<Response<proto::PprResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let edges = store
            .list_edges()
            .map_err(|err| graph_store_status("personalized_page_rank.list_edges", err))?;
        let mut adjacency: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for edge in edges.iter().filter(|edge| !edge.tombstone) {
            adjacency
                .entry(edge.from_id.clone())
                .or_default()
                .push((edge.to_id.clone(), edge.effective_confidence()));
        }
        let max_pushes = if request.max_pushes == 0 {
            10_000
        } else {
            request.max_pushes as usize
        };
        let scores = personalized_pagerank(
            &adjacency,
            &request.seeds,
            request.alpha,
            request.epsilon,
            max_pushes,
        );
        Ok(Response::new(proto::PprResponse {
            scores,
            total_pushes: 0,
        }))
    }
    async fn page_rank(
        &self,
        request: Request<proto::PageRankRequest>,
    ) -> Result<Response<proto::PageRankResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let edges = store
            .list_edges()
            .map_err(|err| graph_store_status("page_rank.list_edges", err))?;
        let damping = if request.damping == 0.0 {
            0.85
        } else {
            request.damping
        };
        let max_iter = if request.max_iter == 0 {
            50
        } else {
            request.max_iter as usize
        };
        let tolerance = if request.tolerance == 0.0 {
            1e-6
        } else {
            request.tolerance
        };
        let scores = pagerank(&edges, damping, max_iter, tolerance);
        Ok(Response::new(proto::PageRankResponse {
            scores,
            iterations_used: max_iter as u32,
        }))
    }
    async fn connected_components(
        &self,
        request: Request<proto::ComponentsRequest>,
    ) -> Result<Response<proto::ComponentsResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let edges = store
            .list_edges()
            .map_err(|err| graph_store_status("connected_components.list_edges", err))?;
        let components = connected_components(&edges, request.directed);
        let mut node_to_component = HashMap::new();
        for (idx, component) in components.iter().enumerate() {
            for node_id in component {
                node_to_component.insert(node_id.clone(), idx as u32);
            }
        }
        Ok(Response::new(proto::ComponentsResponse {
            node_to_component,
            component_count: components.len() as u32,
        }))
    }
    async fn communities(
        &self,
        request: Request<proto::CommunitiesRequest>,
    ) -> Result<Response<proto::CommunitiesResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let edges = store
            .list_edges()
            .map_err(|err| graph_store_status("communities.list_edges", err))?;
        let (communities, _modularity) = label_propagation_communities(&edges);
        let community_count = communities.values().copied().collect::<HashSet<_>>().len() as u32;
        Ok(Response::new(proto::CommunitiesResponse {
            node_to_community: communities
                .into_iter()
                .map(|(node_id, community)| (node_id, community as u32))
                .collect(),
            community_count,
        }))
    }

    // ====================================================================
    // Stats + diagnostics
    // ====================================================================

    async fn graph_verify(
        &self,
        request: Request<proto::GraphVerifyRequest>,
    ) -> Result<Response<proto::VerifyReport>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let store = self.tenant_store(&request.tenant_id)?;
        let report = store
            .verify()
            .map_err(|err| graph_store_status("graph_verify", err))?;
        let issues = report
            .problems
            .into_iter()
            .map(|problem| format!("{}:{}: {}", problem.kind, problem.id, problem.detail))
            .collect();
        Ok(Response::new(proto::VerifyReport {
            ok: report.ok,
            issues,
            graph_version_at_verify: report.stats.version,
        }))
    }
    async fn rebuild_indexes(
        &self,
        request: Request<proto::RebuildIndexesRequest>,
    ) -> Result<Response<proto::RebuildIndexesResponse>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let mut store = self.tenant_store(&request.tenant_id)?;
        let start = Instant::now();
        let report = store
            .rebuild_indexes()
            .map_err(|err| graph_store_status("rebuild_indexes", err))?;
        Ok(Response::new(proto::RebuildIndexesResponse {
            indexes_rebuilt: u32::from(report.repaired),
            duration_micros: start.elapsed().as_micros() as u64,
        }))
    }

    // ====================================================================
    // Cache
    // ====================================================================

    async fn cache_put(
        &self,
        request: Request<proto::CachePutRequest>,
    ) -> Result<Response<proto::CacheAck>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let current_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("cache_put.stats", err))?
            .version;
        if request.graph_version != 0 && request.graph_version != current_graph_version {
            return Err(Status::failed_precondition(format!(
                "cache_put graph_version {} does not match current graph version {}",
                request.graph_version, current_graph_version
            )));
        }
        let cache = self
            .state
            .tenant_graph_cache(&request.tenant_id)
            .map_err(|err| state_status("cache_put.cache", err))?;
        cache
            .put(
                GraphCachePutBody {
                    tenant_id: None,
                    kind: "query_result".to_string(),
                    key: Value::String(request.cache_key.clone()),
                    value: bytes_to_json(request.payload),
                    metadata: json!({
                        "grpc_cache_key": request.cache_key,
                        "requested_graph_version": request.graph_version,
                    }),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: Some("grpc.v1".to_string()),
                    source_hashes: Vec::new(),
                },
                current_graph_version,
            )
            .map_err(|err| graph_store_status("cache_put", err))?;
        Ok(Response::new(proto::CacheAck {
            ok: true,
            reason: "stored".to_string(),
        }))
    }
    async fn cache_get(
        &self,
        request: Request<proto::CacheGetRequest>,
    ) -> Result<Response<proto::CacheGetResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let current_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("cache_get.stats", err))?
            .version;
        let cache = self
            .state
            .tenant_graph_cache(&request.tenant_id)
            .map_err(|err| state_status("cache_get.cache", err))?;
        let result = cache
            .get(grpc_cache_lookup(request.cache_key), current_graph_version)
            .map_err(|err| graph_store_status("cache_get", err))?;
        Ok(Response::new(proto::CacheGetResponse {
            found: result.hit,
            payload: json_to_bytes(result.value)?,
            stored_at_graph_version: result.entry_graph_version.unwrap_or_default(),
            stale: result.stale,
        }))
    }
    async fn cache_check(
        &self,
        request: Request<proto::CacheCheckRequest>,
    ) -> Result<Response<proto::CacheCheckResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let current_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("cache_check.stats", err))?
            .version;
        let cache = self
            .state
            .tenant_graph_cache(&request.tenant_id)
            .map_err(|err| state_status("cache_check.cache", err))?;
        let result = cache
            .check(grpc_cache_lookup(request.cache_key), current_graph_version)
            .map_err(|err| graph_store_status("cache_check", err))?;
        Ok(Response::new(proto::CacheCheckResponse {
            present: result.hit,
            stale: result.stale,
            stored_at_graph_version: result.entry_graph_version.unwrap_or_default(),
        }))
    }
    async fn cache_explain(
        &self,
        request: Request<proto::CacheCheckRequest>,
    ) -> Result<Response<proto::CacheExplainResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let current_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("cache_explain.stats", err))?
            .version;
        let cache = self
            .state
            .tenant_graph_cache(&request.tenant_id)
            .map_err(|err| state_status("cache_explain.cache", err))?;
        let result = cache
            .explain(grpc_cache_lookup(request.cache_key), current_graph_version)
            .map_err(|err| graph_store_status("cache_explain", err))?;
        let staleness_reasons = if result.stale {
            vec![result.reason]
        } else {
            Vec::new()
        };
        Ok(Response::new(proto::CacheExplainResponse {
            present: result.hit,
            stale: result.stale,
            stored_at_graph_version: result.entry_graph_version.unwrap_or_default(),
            current_graph_version,
            staleness_reasons,
        }))
    }
    async fn cache_invalidate(
        &self,
        request: Request<proto::CacheInvalidateRequest>,
    ) -> Result<Response<proto::CacheAck>, Status> {
        self.require_request_scope(&request, "graph:write")?;
        let request = request.into_inner();
        let current_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("cache_invalidate.stats", err))?
            .version;
        let cache = self
            .state
            .tenant_graph_cache(&request.tenant_id)
            .map_err(|err| state_status("cache_invalidate.cache", err))?;
        let result = cache
            .invalidate(
                GraphCacheInvalidateBody {
                    tenant_id: None,
                    all: false,
                    stale_only: false,
                    kind: Some("query_result".to_string()),
                    key: Some(Value::String(request.cache_key)),
                    index_manifest_hash: None,
                    auth_scope_hash: None,
                    retrieval_policy_hash: None,
                    model_version: Some("grpc.v1".to_string()),
                    source_hashes: Vec::new(),
                },
                current_graph_version,
            )
            .map_err(|err| graph_store_status("cache_invalidate", err))?;
        Ok(Response::new(proto::CacheAck {
            ok: result.removed > 0,
            reason: format!("removed {}", result.removed),
        }))
    }
    async fn cache_stats(
        &self,
        request: Request<proto::CacheStatsRequest>,
    ) -> Result<Response<proto::CacheStatsResponse>, Status> {
        self.require_request_scope(&request, "graph:read")?;
        let request = request.into_inner();
        let current_graph_version = self
            .tenant_store(&request.tenant_id)?
            .stats()
            .map_err(|err| graph_store_status("cache_stats.stats", err))?
            .version;
        let cache = self
            .state
            .tenant_graph_cache(&request.tenant_id)
            .map_err(|err| state_status("cache_stats.cache", err))?;
        let stats = cache
            .stats(current_graph_version)
            .map_err(|err| graph_store_status("cache_stats", err))?;
        Ok(Response::new(proto::CacheStatsResponse {
            entries: stats.entries_total as u64,
            hits: stats.hits,
            misses: stats.misses,
            stale_evictions: stats.stale_hits,
            explicit_invalidations: stats.invalidations,
        }))
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
            federate: true,
            federate_hub_url: None,
            federate_token: None,
            federate_peer_id: None,
            federate_private_key: None,
            federate_provenance: false,
            federate_snapshot_text_bytes: rustyred_search::DEFAULT_WEB_COMMONS_SNAPSHOT_TEXT_BYTES,
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
    async fn grpc_write_query_diagnostics_and_cache_surfaces_work() {
        let service = service_with_graph();

        let count = service
            .cypher(Request::new(proto::CypherRequest {
                tenant_id: "smoke".to_string(),
                cypher: "MATCH (n:Person) RETURN COUNT(*)".to_string(),
                parameters_json: String::new(),
                transaction_id: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(count.result_count, 1);
        let count_payload: Value = serde_json::from_str(&count.result_json).unwrap();
        assert_eq!(count_payload["row_count"], 1);

        let created = service
            .upsert_node(Request::new(proto::UpsertNodeRequest {
                tenant_id: "smoke".to_string(),
                node: Some(proto::Node {
                    id: "node:c".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: Some(properties_to_proto(&json!({"name": "Katherine"}))),
                }),
                transaction_id: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(created.id, "node:c");

        let verify = service
            .graph_verify(Request::new(proto::GraphVerifyRequest {
                tenant_id: "smoke".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(verify.ok);
        assert!(verify.issues.is_empty());

        let payload = b"cached bytes".to_vec();
        let ack = service
            .cache_put(Request::new(proto::CachePutRequest {
                tenant_id: "smoke".to_string(),
                cache_key: "cypher:person-count".to_string(),
                payload: payload.clone(),
                graph_version: 0,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(ack.ok);

        let cached = service
            .cache_get(Request::new(proto::CacheGetRequest {
                tenant_id: "smoke".to_string(),
                cache_key: "cypher:person-count".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(cached.found);
        assert_eq!(cached.payload, payload);

        let stats = service
            .cache_stats(Request::new(proto::CacheStatsRequest {
                tenant_id: "smoke".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.hits, 1);
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
