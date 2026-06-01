use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rustyred_core::commands::{RustyredCommand, RustyredRequest, RustyredResponse};
use rustyred_core::errors::RustyredError;
use rustyred_core::executor::{RustyredExecutor, StoreBackedRustyredExecutor};
use rustyred_core::{
    checkout_graph_version, compile_graph_pack, diff_graph_snapshots, graph_version_log,
    merge_graph_snapshots, stable_hash, update_graph_ref, CodeKgManifest, Direction, EdgeRecord,
    EpistemicType, GraphCompileOptions, GraphMergeOptions, GraphMergeStrategy, GraphSnapshot,
    GraphStats, GraphStoreError, GraphVersionRepository, HarnessInstantKg, InMemoryGraphStore,
    NeighborQuery, NodeQuery, NodeRecord, SessionDelta,
};
use rustyred_mcp::{
    agent_manifest, handle_mcp_request_with_context, mcp_manifest, McpRequestContext,
};
use rustyred_search::{
    apply_batch_to_store, build_web_commons_fragment, build_web_commons_ingest_plan,
    render_serp_html, run_live_crawl, search_substrate, CrawlBudget, CrawlReceipt, CrawlRequest,
    CrawlScope, PageRecord, RustyWebError, SearchOptions, WebCommonsFragment,
    WebCommonsFragmentOptions, WebCommonsPageDisposition, WebCommonsReceipt, EDGE_LINKS_TO,
    LABEL_PAGE, LABEL_WEB_COMMONS_ATTESTATION, LABEL_WEB_COMMONS_PEER,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};

use crate::auth::require_scope;
use crate::graph_cache::{
    GraphCacheInvalidateBody, GraphCacheLookupBody, GraphCachePutBody, GraphCacheStatsBody,
};
use crate::observability::{
    KIND_ALGO_COMMUNITIES, KIND_ALGO_COMPONENTS, KIND_ALGO_PAGERANK, KIND_ALGO_PPR, KIND_CYPHER,
    KIND_FULLTEXT_SEARCH, KIND_VECTOR_SEARCH,
};
use crate::query_surface::{
    execute_cypher_query, execute_public_query, explain_cypher_query, parse_tx_cypher_mutations,
    resolve_tenant_id, PublicCypherBody, QuerySurfaceError,
};
use crate::state::{AppState, StoreAccessError, TenantGraphStore};

#[derive(Debug, Deserialize)]
pub struct CommandBody {
    pub command: String,
    #[serde(default, alias = "payload")]
    pub args: Value,
}

#[derive(Debug, Deserialize)]
pub struct BatchBody {
    #[serde(default)]
    pub commands: Vec<CommandBody>,
}

#[derive(Debug, Deserialize)]
pub struct RootCommandBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    pub command: String,
    #[serde(default, alias = "payload")]
    pub args: Value,
}

#[derive(Debug, Deserialize)]
pub struct RootBatchBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub commands: Vec<CommandBody>,
}

#[derive(Debug, Deserialize)]
pub struct GraphQueryBody {
    pub query: String,
    #[serde(default)]
    pub graph: Value,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default, alias = "tenant_id")]
    pub tenant: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CrawlRouteBody {
    #[serde(default, alias = "tenant_id")]
    pub tenant: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub seeds: Vec<String>,
    #[serde(default)]
    pub budget: Option<CrawlBudget>,
    #[serde(default)]
    pub scope: Option<CrawlScope>,
}

#[derive(Debug, Deserialize)]
pub struct FederateSubmitBody {
    #[serde(default, alias = "tenant_id")]
    pub tenant: Option<String>,
    #[serde(default)]
    pub federable: Option<bool>,
    #[serde(default)]
    pub graph_delta_hash: Option<String>,
    #[serde(default)]
    pub receipt: Option<CrawlReceipt>,
    #[serde(default)]
    pub fragment: Option<WebCommonsFragment>,
}

#[derive(Debug, Deserialize)]
pub struct NodeWriteBody {
    pub id: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: Value,
    #[serde(default)]
    pub tombstone: bool,
}

impl NodeWriteBody {
    fn into_record(self) -> NodeRecord {
        let mut node = NodeRecord::new(self.id, self.labels, self.properties);
        node.tombstone = self.tombstone;
        node
    }
}

#[derive(Debug, Deserialize)]
pub struct EdgeWriteBody {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default)]
    pub properties: Value,
    #[serde(default)]
    pub tombstone: bool,
}

impl EdgeWriteBody {
    fn into_record(self) -> EdgeRecord {
        let mut edge = EdgeRecord::new(
            self.id,
            self.from_id,
            self.edge_type,
            self.to_id,
            self.properties,
        );
        edge.tombstone = self.tombstone;
        edge
    }
}

#[derive(Debug, Serialize)]
pub struct HealthBody {
    pub status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct VectorDesignateBody {
    pub label: String,
    pub property: String,
    pub dimension: usize,
}

#[derive(Debug, Deserialize)]
pub struct VectorSearchBody {
    pub query: Vec<f32>,
    #[serde(default = "default_k")]
    pub k: usize,
    pub label: Option<String>,
    pub property: String,
}

#[derive(Debug, Deserialize)]
pub struct HybridSearchBody {
    pub query: Vec<f32>,
    #[serde(default = "default_k")]
    pub k: usize,
    pub label: Option<String>,
    pub property: String,
    pub graph_seeds: Vec<String>,
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    #[serde(default)]
    pub alpha: Option<f32>,
    #[serde(default)]
    pub confidence_weighted_graph_distance: Option<bool>,
    #[serde(default)]
    pub edge_type_weights: Option<std::collections::BTreeMap<String, f32>>,
}

#[derive(Debug, Deserialize)]
pub struct EpistemicNeighborsBody {
    pub node_id: String,
    #[serde(default)]
    pub epistemic_types: Option<Vec<EpistemicType>>,
    #[serde(default)]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionBeginBody {
    #[serde(default)]
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionMutationBody {
    pub tx_id: String,
    #[serde(default)]
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphVersionDiffBody {
    pub base: GraphSnapshot,
    #[serde(default)]
    pub target: Option<GraphSnapshot>,
}

#[derive(Debug, Deserialize)]
pub struct GraphVersionRefBody {
    #[serde(default)]
    pub repository: Option<GraphVersionRepository>,
    #[serde(default)]
    pub updated_at_unix_ms: Option<u128>,
    #[serde(flatten)]
    pub options: GraphCompileOptions,
}

#[derive(Debug, Deserialize)]
pub struct GraphVersionLogBody {
    pub repository: GraphVersionRepository,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphVersionCheckoutBody {
    pub repository: GraphVersionRepository,
    pub target: String,
}

#[derive(Debug, Deserialize)]
pub struct GraphVersionMergeBody {
    pub base: GraphSnapshot,
    #[serde(default)]
    pub ours: Option<GraphSnapshot>,
    pub theirs: GraphSnapshot,
    #[serde(flatten)]
    pub options: GraphMergeOptions,
}

fn default_k() -> usize {
    10
}
fn default_max_hops() -> usize {
    3
}
pub fn build_router(state: AppState) -> Router {
    let cors = cors_layer(&state);
    Router::new()
        .route("/", get(search_home))
        .route("/search", get(search_html))
        .route("/search.json", get(search_json))
        .route("/crawl", post(crawl_submit))
        .route("/federate/submit", post(federate_submit))
        .route("/health", get(health))
        .route("/health/", get(health))
        .route("/ready", get(ready))
        .route("/ready/", get(ready))
        .route("/openapi.json", get(crate::openapi::openapi))
        .route("/.well-known/mcp/rustyred.json", get(mcp_well_known))
        .route("/.well-known/agent.json", get(agent_well_known))
        .route("/mcp", post(mcp_post))
        .route("/metrics", get(crate::metrics::metrics))
        .route(
            "/v1/diagnostics/slow_queries",
            get(crate::metrics::slow_queries),
        )
        .route(
            "/v1/diagnostics/config",
            get(crate::metrics::diagnostics_config),
        )
        .route("/v1/command", post(root_command))
        .route("/v1/batch", post(root_batch))
        .route("/v1/query", post(public_query))
        .route("/v1/cypher", post(public_cypher))
        .route("/v1/cypher/explain", post(public_cypher_explain))
        .route("/v1/transactions/begin", post(transaction_begin))
        .route("/v1/transactions/commit", post(transaction_commit))
        .route("/v1/transactions/rollback", post(transaction_rollback))
        .route("/v1/cache/put", post(root_cache_put))
        .route("/v1/cache/get", post(root_cache_get))
        .route("/v1/cache/check", post(root_cache_check))
        .route("/v1/cache/explain", post(root_cache_explain))
        .route("/v1/cache/invalidate", post(root_cache_invalidate))
        .route("/v1/cache/stats", post(root_cache_stats))
        .route("/v1/tenants/:tenant_id/command", post(command))
        .route("/v1/tenants/:tenant_id/batch", post(batch))
        .route("/v1/tenants/:tenant_id/runs/:run_id", get(run_get))
        .route("/v1/tenants/:tenant_id/graph/query", post(graph_query))
        .route(
            "/v1/tenants/:tenant_id/graph/nodes",
            post(graph_node_upsert),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/nodes/query",
            post(graph_node_query),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/nodes/:node_id",
            get(graph_node_get),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/edges",
            post(graph_edge_upsert),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/edges/:edge_id",
            get(graph_edge_get),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/neighbors",
            post(graph_neighbors),
        )
        .route("/v1/tenants/:tenant_id/graph/stats", get(graph_stats))
        .route("/v1/tenants/:tenant_id/graph/verify", get(graph_verify))
        .route(
            "/v1/tenants/:tenant_id/graph/rebuild-indexes",
            post(graph_rebuild_indexes),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/version/compile",
            post(graph_version_compile),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/version/diff",
            post(graph_version_diff),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/version/ref",
            post(graph_version_ref),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/version/log",
            post(graph_version_log_route),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/version/checkout",
            post(graph_version_checkout),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/version/merge",
            post(graph_version_merge),
        )
        .route("/v1/tenants/:tenant_id/context/pack", post(context_pack))
        .route(
            "/v1/tenants/:tenant_id/graph/vector/designate",
            post(graph_vector_designate),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/vector/search",
            post(graph_vector_search),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/vector/hybrid",
            post(graph_vector_hybrid),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/epistemic-neighbors",
            post(graph_epistemic_neighbors),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/ppr",
            post(graph_algorithm_ppr),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/components",
            post(graph_algorithm_components),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/pagerank",
            post(graph_algorithm_pagerank),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/algorithms/communities",
            post(graph_algorithm_communities),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/spatial/designate",
            post(graph_spatial_designate),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/spatial/radius",
            post(graph_spatial_radius),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/spatial/bbox",
            post(graph_spatial_bbox),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/fulltext/designate",
            post(graph_fulltext_designate),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/fulltext/search",
            post(graph_fulltext_search),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/bulk/nodes",
            post(graph_bulk_nodes),
        )
        .route(
            "/v1/tenants/:tenant_id/graph/bulk/edges",
            post(graph_bulk_edges),
        )
        .route(
            "/v1/tenants/:tenant_id/instant-kg/status",
            post(instant_kg_status),
        )
        .route(
            "/v1/tenants/:tenant_id/instant-kg/ppr",
            post(instant_kg_ppr),
        )
        .route(
            "/v1/tenants/:tenant_id/instant-kg/impact",
            post(instant_kg_impact),
        )
        .route(
            "/v1/tenants/:tenant_id/instant-kg/related-objects",
            post(instant_kg_related_objects),
        )
        .route(
            "/v1/tenants/:tenant_id/instant-kg/search",
            post(instant_kg_search),
        )
        .route(
            "/v1/tenants/:tenant_id/instant-kg/explain-edge",
            post(instant_kg_explain_edge),
        )
        .layer(cors)
        .with_state(state)
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "ok" })
}

async fn mcp_well_known(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.mcp_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let config = state.mcp_config();
    Json(mcp_manifest(state.config.public_url.as_deref(), &config)).into_response()
}

async fn agent_well_known(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.mcp_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let config = state.mcp_config();
    Json(agent_manifest(state.config.public_url.as_deref(), &config)).into_response()
}

async fn mcp_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !state.config.mcp_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !mcp_origin_allowed(&headers, &state.config.allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let auth_context = match require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        Ok(context) => context,
        Err(status) => return status.into_response(),
    };

    let config = state.mcp_config();
    let mcp_context = McpRequestContext::with_scopes(auth_context.scopes);
    Json(handle_mcp_request_with_context(
        &state,
        &config,
        &mcp_context,
        payload,
    ))
    .into_response()
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.store_ready() {
        Ok(report) => Json(json!({
            "status": "ready",
            "store": report.store,
            "mode": report.mode,
            "durability": report.durability,
            "strict_acid": report.strict_acid,
            "require_volume": report.require_volume,
            "data_dir": report.data_dir
        }))
        .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "not_ready",
                "store": "unavailable",
                "mode": state.config.storage_mode.as_str(),
                "error": error.code,
                "message": error.message
            })),
        )
            .into_response(),
    }
}

async fn search_home(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> axum::response::Response {
    render_search_response(
        &state,
        &headers,
        SearchQuery {
            q: None,
            tenant: None,
        },
    )
}

async fn search_html(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> axum::response::Response {
    render_search_response(&state, &headers, query)
}

async fn search_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> axum::response::Response {
    match execute_search(&state, &headers, &query) {
        Ok((tenant_id, search)) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "search": search
        }))
        .into_response(),
        Err(response) => response,
    }
}

fn render_search_response(
    state: &AppState,
    headers: &HeaderMap,
    query: SearchQuery,
) -> axum::response::Response {
    let (_tenant_id, search) = match execute_search(state, headers, &query) {
        Ok(result) => result,
        Err(response) => return response,
    };
    (
        [(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        render_serp_html(&search),
    )
        .into_response()
}

fn execute_search(
    state: &AppState,
    headers: &HeaderMap,
    query: &SearchQuery,
) -> Result<(String, rustyred_search::SubstrateSearch), axum::response::Response> {
    if let Err(status) = require_scope(
        headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return Err(status.into_response());
    }
    let tenant_id = resolve_route_tenant(state, query.tenant.as_deref())?;
    let store = search_snapshot_store(state, &tenant_id)?;
    let search = search_substrate(
        &store,
        query.q.as_deref().unwrap_or_default(),
        SearchOptions::default(),
    );
    Ok((tenant_id, search))
}

async fn crawl_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CrawlRouteBody>,
) -> axum::response::Response {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id = match resolve_route_tenant(&state, body.tenant.as_deref()) {
        Ok(tenant_id) => tenant_id,
        Err(response) => return response,
    };
    let scope_was_supplied = body.scope.is_some();
    let mut request =
        CrawlRequest::new(body.run_id.unwrap_or_else(default_crawl_run_id), body.seeds);
    request.budget = body.budget.unwrap_or_default();
    request.scope = body.scope.unwrap_or_default();
    if state.config.federate && !scope_was_supplied {
        request.scope.federable = true;
    }

    let federation_request = request.clone();
    let output = match run_live_crawl(request).await {
        Ok(output) => output,
        Err(error) => return rustyweb_error_response(error),
    };
    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    let transaction =
        match commit_batch_with_indexes(&state, &tenant_id, &mut store, output.graph.batch.clone())
        {
            Ok(transaction) => transaction,
            Err(error) => return graph_store_error_response(error),
        };
    let federation =
        submit_web_commons_fragment(&state, &tenant_id, &federation_request, &output).await;
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "receipt": output.receipt,
        "transaction": transaction,
        "federation": federation
    }))
    .into_response()
}

async fn federate_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FederateSubmitBody>,
) -> axum::response::Response {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "federation:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id = match resolve_route_tenant(&state, body.tenant.as_deref()) {
        Ok(tenant_id) => tenant_id,
        Err(response) => return response,
    };
    if let Some(fragment) = body.fragment {
        return ingest_web_commons_fragment(&state, &tenant_id, fragment);
    }
    let federable = body
        .federable
        .or_else(|| body.receipt.as_ref().map(|receipt| receipt.scope.federable))
        .unwrap_or(false);
    if !federable {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "not_federable",
                "message": "federation submit requires federable=true or a federable crawl receipt"
            })),
        )
            .into_response();
    }
    let graph_delta_hash = body.graph_delta_hash.or_else(|| {
        body.receipt
            .as_ref()
            .map(|receipt| receipt.graph_delta_hash.clone())
    });
    let graph_delta_hash = match graph_delta_hash.filter(|hash| !hash.trim().is_empty()) {
        Some(hash) => hash,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "fragment_required",
                    "message": "federation submit requires a signed Web Commons fragment or a non-empty receipt/hash"
                })),
            )
                .into_response();
        }
    };
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "accepted": true,
        "merged": false,
        "status": "validated_noop",
        "graph_delta_hash": graph_delta_hash
    }))
    .into_response()
}

fn ingest_web_commons_fragment(
    state: &AppState,
    tenant_id: &str,
    fragment: WebCommonsFragment,
) -> axum::response::Response {
    if let Err(response) = verify_web_commons_fragment_signature(&fragment) {
        return response;
    }
    let mut store = match state.tenant_graph_store(tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    let base = match store.graph_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return graph_store_error_response(error),
    };
    let trust = web_commons_peer_trust(&base, &fragment.peer_id);
    if trust.weight <= 0.0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "peer_blocked",
                "message": "web_commons peer is blocked",
                "peer_id": fragment.peer_id,
                "trust_tier": trust.tier,
            })),
        )
            .into_response();
    }

    let receipt = web_commons_receipt_for_fragment(&base, &fragment, &trust);
    let plan = match build_web_commons_ingest_plan(&fragment, receipt) {
        Ok(plan) => plan,
        Err(error) => return rustyweb_error_response(error),
    };
    let mut projected = match InMemoryGraphStore::from_snapshot(base.clone()) {
        Ok(store) => store,
        Err(error) => return graph_store_error_response(error),
    };
    if let Err(error) = apply_batch_to_store(&mut projected, &plan.batch) {
        return graph_store_error_response(error);
    }
    let target = projected.snapshot();
    let merge = merge_graph_snapshots(
        &base,
        &base,
        &target,
        GraphMergeOptions {
            strategy: GraphMergeStrategy::PreferTheirs,
            name: Some("web-commons-merge".to_string()),
            message: Some("merge signed Web Commons fragment".to_string()),
            ..GraphMergeOptions::default()
        },
    );
    if !merge.conflicts.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "web_commons_merge_conflict",
                "message": "signed Web Commons fragment produced merge conflicts",
                "merge": merge,
            })),
        )
            .into_response();
    }
    let transaction =
        match commit_batch_with_indexes(&state, &tenant_id, &mut store, plan.batch.clone()) {
            Ok(transaction) => transaction,
            Err(error) => return graph_store_error_response(error),
        };

    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "accepted": plan.receipt.accepted,
        "merged": true,
        "status": "merged",
        "graph_delta_hash": plan.receipt.graph_delta_hash,
        "receipt": plan.receipt,
        "merge": merge,
        "transaction": transaction,
    }))
    .into_response()
}

fn commit_batch_with_indexes(
    state: &AppState,
    tenant_id: &str,
    store: &mut TenantGraphStore,
    batch: rustyred_core::GraphMutationBatch,
) -> Result<rustyred_core::GraphTransaction, GraphStoreError> {
    let index_batch = batch.clone();
    let transaction = store.commit_batch(batch)?;
    index_committed_batch(state, tenant_id, &index_batch);
    Ok(transaction)
}

fn index_committed_batch(
    state: &AppState,
    tenant_id: &str,
    batch: &rustyred_core::GraphMutationBatch,
) {
    for mutation in &batch.mutations {
        state.observability.record_mutation();
        if let rustyred_core::GraphMutation::NodeUpsert(node) = mutation {
            state.maybe_index_node_spatially(tenant_id, node);
            state.maybe_index_node_fulltext(tenant_id, node);
        }
    }
}

async fn submit_web_commons_fragment(
    state: &AppState,
    tenant_id: &str,
    request: &CrawlRequest,
    output: &rustyred_search::CrawlRunOutput,
) -> Value {
    if !state.config.federate {
        return json!({"enabled": false, "submitted": false, "reason": "disabled"});
    }
    if !request.scope.federable {
        return json!({"enabled": true, "submitted": false, "reason": "scope_not_federable"});
    }
    let Some(hub_url) = state.config.federate_hub_url.as_deref() else {
        return json!({"enabled": true, "submitted": false, "reason": "hub_url_missing"});
    };
    let Some(private_key) = state.config.federate_private_key.as_deref() else {
        return json!({"enabled": true, "submitted": false, "reason": "private_key_missing"});
    };
    let public_peer_id = match public_peer_id_from_private_key(private_key) {
        Ok(peer_id) => peer_id,
        Err(error) => {
            return json!({"enabled": true, "submitted": false, "reason": "private_key_invalid", "error": error});
        }
    };
    let peer_id = state
        .config
        .federate_peer_id
        .clone()
        .unwrap_or_else(|| public_peer_id.clone());
    if normalize_peer_id(&peer_id) != public_peer_id {
        return json!({
            "enabled": true,
            "submitted": false,
            "reason": "peer_id_private_key_mismatch",
        });
    }
    let options = WebCommonsFragmentOptions {
        include_provenance: state.config.federate_provenance,
        snapshot_text_bytes: state.config.federate_snapshot_text_bytes,
    };
    let mut fragment = match build_web_commons_fragment(output, request, peer_id, &options) {
        Ok(fragment) => fragment,
        Err(error) => {
            return json!({"enabled": true, "submitted": false, "reason": "fragment_build_failed", "error": error.to_string()});
        }
    };
    if let Err(error) = sign_web_commons_fragment(&mut fragment, private_key) {
        return json!({"enabled": true, "submitted": false, "reason": "fragment_sign_failed", "error": error});
    }

    let submit_url = federation_submit_url(hub_url);
    let mut request_builder = reqwest::Client::new().post(&submit_url).json(&json!({
        "tenant": tenant_id,
        "federable": true,
        "graph_delta_hash": fragment.graph_delta_hash,
        "fragment": fragment,
    }));
    if let Some(token) = state.config.federate_token.as_deref() {
        request_builder = request_builder.bearer_auth(token);
    }
    match request_builder.send().await {
        Ok(response) => {
            let status = response.status();
            let body = response.json::<Value>().await.unwrap_or_else(|error| {
                json!({
                    "error": "invalid_hub_response",
                    "message": error.to_string(),
                })
            });
            json!({
                "enabled": true,
                "submitted": status.is_success(),
                "status": status.as_u16(),
                "hub_url": submit_url,
                "response": body,
            })
        }
        Err(error) => json!({
            "enabled": true,
            "submitted": false,
            "hub_url": submit_url,
            "error": error.to_string(),
        }),
    }
}

#[derive(Clone, Debug)]
struct WebCommonsPeerTrust {
    tier: String,
    weight: f64,
}

#[derive(Default)]
struct PageSupport {
    peer_weights: BTreeMap<String, f64>,
    domains: BTreeSet<String>,
    source_classes: BTreeSet<String>,
    fetched_times: BTreeSet<i128>,
    inbound_total: usize,
    inbound_external: usize,
}

impl PageSupport {
    fn weighted_peer_sum(&self) -> f64 {
        self.peer_weights.values().sum()
    }

    fn external_support_ratio(&self) -> f64 {
        if self.inbound_total == 0 {
            0.0
        } else {
            self.inbound_external as f64 / self.inbound_total as f64
        }
    }

    fn temporal_spread_ms(&self) -> i128 {
        match (
            self.fetched_times.iter().next(),
            self.fetched_times.iter().next_back(),
        ) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        }
    }
}

fn web_commons_receipt_for_fragment(
    base: &GraphSnapshot,
    fragment: &WebCommonsFragment,
    trust: &WebCommonsPeerTrust,
) -> WebCommonsReceipt {
    let support = web_commons_support(base, fragment, trust);
    let dispositions = fragment
        .pages
        .iter()
        .map(|page| page_disposition(page, support.get(&page.id)))
        .collect::<Vec<_>>();
    let accepted_pages = dispositions
        .iter()
        .filter(|disposition| disposition.disposition != "dropped")
        .count();
    let dropped_pages = dispositions.len().saturating_sub(accepted_pages);

    WebCommonsReceipt {
        accepted: accepted_pages > 0,
        peer_id: fragment.peer_id.clone(),
        graph_delta_hash: fragment.graph_delta_hash.clone(),
        accepted_pages,
        dropped_pages,
        dispositions,
    }
}

fn page_disposition(page: &PageRecord, support: Option<&PageSupport>) -> WebCommonsPageDisposition {
    if page.id.trim().is_empty() || page.url.trim().is_empty() || page.domain.trim().is_empty() {
        return WebCommonsPageDisposition {
            page_id: page.id.clone(),
            url: page.url.clone(),
            disposition: "dropped".to_string(),
            reason: "missing required page identity fields".to_string(),
        };
    }
    let Some(support) = support else {
        return WebCommonsPageDisposition {
            page_id: page.id.clone(),
            url: page.url.clone(),
            disposition: "probationary".to_string(),
            reason: "first attestation recorded".to_string(),
        };
    };
    let canonical = support.peer_weights.len() >= 2
        && support.weighted_peer_sum() >= 1.0
        && support.domains.len() >= 2
        && support.external_support_ratio() >= 0.5
        && support.temporal_spread_ms() >= 1;
    if canonical {
        WebCommonsPageDisposition {
            page_id: page.id.clone(),
            url: page.url.clone(),
            disposition: "canonical".to_string(),
            reason: "independent peer, domain, external-link, and temporal support satisfied"
                .to_string(),
        }
    } else {
        WebCommonsPageDisposition {
            page_id: page.id.clone(),
            url: page.url.clone(),
            disposition: "probationary".to_string(),
            reason: format!(
                "awaiting corroboration: peers={}, domains={}, external_support={:.2}, temporal_spread_ms={}",
                support.peer_weights.len(),
                support.domains.len(),
                support.external_support_ratio(),
                support.temporal_spread_ms()
            ),
        }
    }
}

fn web_commons_support(
    base: &GraphSnapshot,
    fragment: &WebCommonsFragment,
    current_trust: &WebCommonsPeerTrust,
) -> BTreeMap<String, PageSupport> {
    let mut support: BTreeMap<String, PageSupport> = BTreeMap::new();
    let peer_weights = web_commons_peer_weights(base);
    let mut page_domains = page_domains_from_snapshot(base);
    for page in &fragment.pages {
        page_domains.insert(page.id.clone(), page.domain.clone());
    }

    for node in &base.nodes {
        if !node
            .labels
            .iter()
            .any(|label| label == LABEL_WEB_COMMONS_ATTESTATION)
        {
            continue;
        }
        let Some(page_id) = json_string(&node.properties, "page_id") else {
            continue;
        };
        let peer_id = json_string(&node.properties, "peer_id").unwrap_or_default();
        let weight = peer_weights
            .get(&peer_id)
            .copied()
            .unwrap_or_else(|| trust_weight_for_tier("unknown"));
        add_page_support(
            support.entry(page_id).or_default(),
            peer_id,
            weight,
            json_string(&node.properties, "domain"),
            json_string(&node.properties, "source_class"),
            json_string(&node.properties, "fetched_at"),
        );
    }

    for page in &fragment.pages {
        add_page_support(
            support.entry(page.id.clone()).or_default(),
            fragment.peer_id.clone(),
            current_trust.weight,
            Some(page.domain.clone()),
            Some(page.source_class.clone()),
            page.fetched_at.clone(),
        );
    }

    for edge in &base.edges {
        if edge.edge_type == EDGE_LINKS_TO {
            add_inbound_support(&mut support, &page_domains, &edge.from_id, &edge.to_id);
        }
    }
    for edge in &fragment.edges {
        add_inbound_support(
            &mut support,
            &page_domains,
            &edge.from_page_id,
            &edge.to_page_id,
        );
    }

    support
}

fn add_page_support(
    support: &mut PageSupport,
    peer_id: String,
    weight: f64,
    domain: Option<String>,
    source_class: Option<String>,
    fetched_at: Option<String>,
) {
    if !peer_id.trim().is_empty() {
        support.peer_weights.insert(peer_id, weight);
    }
    if let Some(domain) = domain.filter(|value| !value.trim().is_empty()) {
        support.domains.insert(domain);
    }
    if let Some(source_class) = source_class.filter(|value| !value.trim().is_empty()) {
        support.source_classes.insert(source_class);
    }
    if let Some(timestamp) = fetched_at.and_then(|value| value.parse::<i128>().ok()) {
        support.fetched_times.insert(timestamp);
    }
}

fn add_inbound_support(
    support: &mut BTreeMap<String, PageSupport>,
    page_domains: &BTreeMap<String, String>,
    from_page_id: &str,
    to_page_id: &str,
) {
    let Some(from_domain) = page_domains.get(from_page_id) else {
        return;
    };
    let Some(to_domain) = page_domains.get(to_page_id) else {
        return;
    };
    let page_support = support.entry(to_page_id.to_string()).or_default();
    page_support.inbound_total += 1;
    page_support.domains.insert(from_domain.clone());
    if from_domain != to_domain {
        page_support.inbound_external += 1;
    }
}

fn page_domains_from_snapshot(snapshot: &GraphSnapshot) -> BTreeMap<String, String> {
    snapshot
        .nodes
        .iter()
        .filter(|node| node.labels.iter().any(|label| label == LABEL_PAGE))
        .filter_map(|node| {
            json_string(&node.properties, "domain").map(|domain| (node.id.clone(), domain))
        })
        .collect()
}

fn web_commons_peer_trust(snapshot: &GraphSnapshot, peer_id: &str) -> WebCommonsPeerTrust {
    let mut selected: Option<WebCommonsPeerTrust> = None;
    for node in snapshot.nodes.iter().filter(|node| {
        node.labels
            .iter()
            .any(|label| label == LABEL_WEB_COMMONS_PEER)
            && json_string(&node.properties, "peer_id").as_deref() == Some(peer_id)
    }) {
        let tier =
            json_string(&node.properties, "trust_tier").unwrap_or_else(|| "unknown".to_string());
        let weight = json_f64(&node.properties, "trust_weight")
            .unwrap_or_else(|| trust_weight_for_tier(&tier));
        if tier == "blocked" || weight <= 0.0 {
            return WebCommonsPeerTrust { tier, weight: 0.0 };
        }
        if selected
            .as_ref()
            .is_none_or(|current| weight > current.weight)
        {
            selected = Some(WebCommonsPeerTrust { tier, weight });
        }
    }
    selected.unwrap_or_else(|| WebCommonsPeerTrust {
        tier: "unknown".to_string(),
        weight: trust_weight_for_tier("unknown"),
    })
}

fn web_commons_peer_weights(snapshot: &GraphSnapshot) -> BTreeMap<String, f64> {
    snapshot
        .nodes
        .iter()
        .filter(|node| {
            node.labels
                .iter()
                .any(|label| label == LABEL_WEB_COMMONS_PEER)
        })
        .filter_map(|node| {
            let peer_id = json_string(&node.properties, "peer_id")?;
            let tier = json_string(&node.properties, "trust_tier")
                .unwrap_or_else(|| "unknown".to_string());
            let weight = json_f64(&node.properties, "trust_weight")
                .unwrap_or_else(|| trust_weight_for_tier(&tier));
            Some((peer_id, weight))
        })
        .fold(BTreeMap::new(), |mut weights, (peer_id, weight)| {
            weights
                .entry(peer_id)
                .and_modify(|current| {
                    if weight <= 0.0 {
                        *current = 0.0;
                    } else if *current > 0.0 {
                        *current = (*current).max(weight);
                    }
                })
                .or_insert(weight);
            weights
        })
}

fn trust_weight_for_tier(tier: &str) -> f64 {
    match tier {
        "self" | "verified" => 1.0,
        "blocked" => 0.0,
        _ => 0.3,
    }
}

fn verify_web_commons_fragment_signature(
    fragment: &WebCommonsFragment,
) -> Result<(), axum::response::Response> {
    if fragment.signature.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "unsigned_fragment",
                "message": "Web Commons fragment requires an Ed25519 signature"
            })),
        )
            .into_response());
    }
    let verifying_key = decode_ed25519_array::<32>(&fragment.peer_id, "peer_id")
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).map_err(|error| error.to_string()));
    let signature = decode_ed25519_array::<64>(&fragment.signature, "signature")
        .map(|bytes| Signature::from_bytes(&bytes));
    let signing_bytes = fragment.signing_bytes().map_err(|error| error.to_string());
    match (verifying_key, signature, signing_bytes) {
        (Ok(verifying_key), Ok(signature), Ok(signing_bytes)) => verifying_key
            .verify(&signing_bytes, &signature)
            .map_err(|error| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "bad_signature",
                        "message": error.to_string(),
                    })),
                )
                    .into_response()
            }),
        (verifying_key, signature, signing_bytes) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "bad_signature",
                "message": verifying_key
                    .err()
                    .or_else(|| signature.err())
                    .or_else(|| signing_bytes.err())
                    .unwrap_or_else(|| "invalid signature".to_string()),
            })),
        )
            .into_response()),
    }
}

fn sign_web_commons_fragment(
    fragment: &mut WebCommonsFragment,
    private_key: &str,
) -> Result<(), String> {
    let bytes = decode_ed25519_array::<32>(private_key, "private_key")?;
    let signing_key = SigningKey::from_bytes(&bytes);
    let expected_peer_id = hex::encode(signing_key.verifying_key().to_bytes());
    if normalize_peer_id(&fragment.peer_id) != expected_peer_id {
        return Err("peer_id does not match Ed25519 private key".to_string());
    }
    let signature = signing_key.sign(
        &fragment
            .signing_bytes()
            .map_err(|error| error.to_string())?,
    );
    fragment.signature = hex::encode(signature.to_bytes());
    Ok(())
}

fn public_peer_id_from_private_key(private_key: &str) -> Result<String, String> {
    let bytes = decode_ed25519_array::<32>(private_key, "private_key")?;
    let signing_key = SigningKey::from_bytes(&bytes);
    Ok(hex::encode(signing_key.verifying_key().to_bytes()))
}

fn normalize_peer_id(peer_id: &str) -> String {
    peer_id
        .trim()
        .strip_prefix("ed25519:")
        .unwrap_or_else(|| peer_id.trim())
        .to_ascii_lowercase()
}

fn decode_ed25519_array<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    let normalized = normalize_peer_id(value);
    let decoded = hex::decode(&normalized).map_err(|error| format!("{field}: {error}"))?;
    if decoded.len() != N {
        return Err(format!(
            "{field}: expected {N} bytes, got {}",
            decoded.len()
        ));
    }
    let mut bytes = [0u8; N];
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

fn federation_submit_url(hub_url: &str) -> String {
    let trimmed = hub_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/federate/submit") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/federate/submit")
    }
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn json_f64(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn resolve_route_tenant(
    state: &AppState,
    tenant: Option<&str>,
) -> Result<String, axum::response::Response> {
    resolve_tenant_id(tenant, &state.config.mcp_default_tenant)
        .map_err(query_surface_error_response)
}

fn search_snapshot_store(
    state: &AppState,
    tenant_id: &str,
) -> Result<InMemoryGraphStore, axum::response::Response> {
    let store = state
        .tenant_graph_store(tenant_id)
        .map_err(store_unavailable_response)?;
    let snapshot = store.graph_snapshot().map_err(graph_store_error_response)?;
    InMemoryGraphStore::from_snapshot(snapshot).map_err(graph_store_error_response)
}

fn default_crawl_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("rustyweb-{millis}")
}

fn rustyweb_error_response(error: RustyWebError) -> axum::response::Response {
    let status = match error {
        RustyWebError::Fetch { .. } | RustyWebError::HtmlParse { .. } => StatusCode::BAD_GATEWAY,
        RustyWebError::InvalidUrl { .. }
        | RustyWebError::BodyLimitExceeded { .. }
        | RustyWebError::EmptySeeds
        | RustyWebError::InvalidBudget { .. }
        | RustyWebError::BlockedUrl { .. }
        | RustyWebError::InvalidFragment { .. } => StatusCode::BAD_REQUEST,
    };
    (
        status,
        Json(json!({
            "error": "rustyweb_crawl_error",
            "message": error.to_string()
        })),
    )
        .into_response()
}

async fn command(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandBody>,
) -> impl IntoResponse {
    let scope = required_scope_for_command(&body.command);
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        scope,
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(&state, &tenant_id, &body.command, body.args)
}

async fn root_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RootCommandBody>,
) -> impl IntoResponse {
    let scope = required_scope_for_command(&body.command);
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        scope,
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    execute_tenant_command(&state, &tenant_id, &body.command, body.args)
}

async fn batch(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BatchBody>,
) -> impl IntoResponse {
    for item in &body.commands {
        let scope = required_scope_for_command(&item.command);
        if let Err(status) = require_scope(
            &headers,
            &state.config.api_tokens,
            scope,
            state.config.require_auth,
        ) {
            return status.into_response();
        }
    }
    execute_batch_commands(&state, &tenant_id, body.commands)
}

async fn root_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RootBatchBody>,
) -> impl IntoResponse {
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    for item in &body.commands {
        let scope = required_scope_for_command(&item.command);
        if let Err(status) = require_scope(
            &headers,
            &state.config.api_tokens,
            scope,
            state.config.require_auth,
        ) {
            return status.into_response();
        }
    }
    execute_batch_commands(&state, &tenant_id, body.commands)
}

async fn root_cache_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCachePutBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_put(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheLookupBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_get(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheLookupBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_check(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_explain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheLookupBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_explain(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_invalidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheInvalidateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_invalidate(&state, &tenant_id, body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn root_cache_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GraphCacheStatsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match execute_cache_stats(&state, &tenant_id) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn run_get(
    State(state): State<AppState>,
    Path((tenant_id, run_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "run:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(
        &state,
        &tenant_id,
        "RUSTYRED.RUN.GET",
        json!({ "run_id": run_id }),
    )
}

async fn public_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id = match resolve_tenant_id(
        body.get("tenant_id").and_then(Value::as_str),
        &state.config.mcp_default_tenant,
    ) {
        Ok(tenant_id) => tenant_id,
        Err(error) => return query_surface_error_response(error),
    };
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match execute_public_query(&store, &tenant_id, &body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => query_surface_error_response(error),
    }
}

async fn public_cypher(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PublicCypherBody>,
) -> impl IntoResponse {
    let write_scope = body.tx_id.is_some();
    let scope = if write_scope {
        "graph:write"
    } else {
        "graph:read"
    };

    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        scope,
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    if let Some(tx_id) = body.tx_id.as_deref() {
        if tx_id.trim().is_empty() {
            return query_surface_error_response(QuerySurfaceError::invalid(
                "missing_tx_id",
                "tx_id is required when staging transactional Cypher statements",
            ));
        }
        let mutations = match parse_tx_cypher_mutations(&body.query, &body.params) {
            Ok(mutations) => mutations,
            Err(error) => return query_surface_error_response(error),
        };
        let staged_mutations =
            match state.append_graph_transaction_mutations(&tenant_id, tx_id, mutations) {
                Ok(staged_mutations) => staged_mutations,
                Err(error) => return graph_store_error_response(transaction_state_error(error)),
            };
        return Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "query": body.query,
            "tx_id": tx_id,
            "subset": "opencypher_v0_1_write_tx",
            "staged_mutations": staged_mutations,
        }))
        .into_response();
    }
    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    state.observability.record_cypher();
    let start = std::time::Instant::now();
    let outcome = execute_cypher_query(&mut store, &tenant_id, &body);
    let nanos = start.elapsed().as_nanos() as u64;
    let detail = body.query.chars().take(120).collect::<String>();
    state
        .observability
        .record_query_timing(KIND_CYPHER, &detail, nanos, 0, 0);
    match outcome {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            state.observability.record_error();
            query_surface_error_response(error)
        }
    }
}

async fn transaction_begin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransactionBeginBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    let tx_id = match state.begin_graph_transaction(&tenant_id) {
        Ok(tx_id) => tx_id,
        Err(error) => {
            state.observability.record_error();
            return graph_store_error_response(transaction_state_error(error));
        }
    };
    state.observability.record_transaction_begin();
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "tx_id": tx_id,
    }))
    .into_response()
}

async fn transaction_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransactionMutationBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tx_id = body.tx_id.trim();
    if tx_id.is_empty() {
        return query_surface_error_response(QuerySurfaceError::invalid(
            "missing_tx_id",
            "tx_id is required for transaction commit",
        ));
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    let transaction = match state.commit_graph_transaction(&tenant_id, tx_id) {
        Ok(transaction) => transaction,
        Err(error) => {
            state.observability.record_error();
            return graph_store_error_response(transaction_state_error(error));
        }
    };
    state.observability.record_transaction_commit();
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "transaction": transaction,
    }))
    .into_response()
}

async fn transaction_rollback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransactionMutationBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tx_id = body.tx_id.trim();
    if tx_id.is_empty() {
        return query_surface_error_response(QuerySurfaceError::invalid(
            "missing_tx_id",
            "tx_id is required for transaction rollback",
        ));
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    if let Err(error) = state.rollback_graph_transaction(&tenant_id, tx_id) {
        state.observability.record_error();
        return graph_store_error_response(transaction_state_error(error));
    }
    state.observability.record_transaction_rollback();
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "tx_id": tx_id,
        "status": "rolled_back",
    }))
    .into_response()
}

async fn public_cypher_explain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PublicCypherBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let tenant_id =
        match resolve_tenant_id(body.tenant_id.as_deref(), &state.config.mcp_default_tenant) {
            Ok(tenant_id) => tenant_id,
            Err(error) => return query_surface_error_response(error),
        };
    match explain_cypher_query(&tenant_id, &body) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => query_surface_error_response(error),
    }
}

async fn graph_query(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphQueryBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(
        &state,
        &tenant_id,
        "RUSTYRED.DEBUG.CYPHER",
        json!({ "query": body.query, "graph": body.graph, "params": body.params }),
    )
}

async fn context_pack(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(args): Json<Value>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "context:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    execute_tenant_command(&state, &tenant_id, "RUSTYRED.CONTEXT.PACK", args)
}

async fn graph_vector_designate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<VectorDesignateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.designate_vector_property(&body.label, &body.property, body.dimension) {
        Ok(()) => Json(json!({
            "ok": true,
            "label": body.label,
            "property": body.property,
            "dimension": body.dimension
        }))
        .into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_vector_search(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<VectorSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };

    state.observability.record_vector_search();
    let label_ref = body.label.as_deref();
    let detail = format!(
        "label={} property={}",
        label_ref.unwrap_or("*"),
        body.property
    );
    let start = std::time::Instant::now();
    let outcome = store.vector_search(label_ref, &body.property, &body.query, body.k);
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_VECTOR_SEARCH, &detail, nanos, 0, 0);
    match outcome {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(node_id, distance)| {
                    let node = store.get_node(&node_id).ok().flatten();
                    json!({ "node_id": node_id, "distance": distance, "node": node })
                })
                .collect();
            Json(json!({ "ok": true, "results": items })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_vector_hybrid(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HybridSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };

    state.observability.record_vector_search();
    let label_ref = body.label.as_deref();
    let detail = format!(
        "label={} property={}",
        label_ref.unwrap_or("*"),
        body.property
    );
    let mut scoring = state.config.tenant_config(&tenant_id).hybrid_scoring;
    if let Some(alpha) = body.alpha {
        scoring = scoring.with_alpha(alpha);
    }
    if let Some(confidence_weighted) = body.confidence_weighted_graph_distance {
        scoring.confidence_weighted_graph_distance = confidence_weighted;
    }
    if let Some(edge_type_weights) = body.edge_type_weights {
        scoring.edge_type_weights = edge_type_weights;
    }
    let start = std::time::Instant::now();
    let outcome = store.hybrid_search_with_config(
        label_ref,
        &body.property,
        &body.query,
        body.k,
        &body.graph_seeds,
        body.max_hops,
        &scoring,
    );
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_VECTOR_SEARCH, &detail, nanos, 0, 0);
    match outcome {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(node_id, score)| {
                    let node = store.get_node(&node_id).ok().flatten();
                    json!({ "node_id": node_id, "score": score, "node": node })
                })
                .collect();
            Json(json!({
                "ok": true,
                "results": items,
                "scoring": {
                    "alpha": scoring.alpha,
                    "confidence_weighted_graph_distance": scoring.confidence_weighted_graph_distance,
                    "edge_type_weights": scoring.edge_type_weights,
                }
            }))
            .into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_epistemic_neighbors(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<EpistemicNeighborsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };

    let types_ref = body.epistemic_types.as_deref();
    match store.epistemic_neighbors(
        &body.node_id,
        types_ref,
        body.min_confidence,
        body.max_depth,
    ) {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(edge, node)| json!({ "edge": edge, "node": node }))
                .collect();
            Json(json!({ "ok": true, "results": items })).into_response()
        }
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_node_upsert(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NodeWriteBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    let record = body.into_record();
    let index_clone = record.clone();
    match store.upsert_node(record) {
        Ok(result) => {
            state.observability.record_mutation();
            state.maybe_index_node_spatially(&tenant_id, &index_clone);
            state.maybe_index_node_fulltext(&tenant_id, &index_clone);
            Json(json!({ "ok": true, "node": result })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_node_get(
    State(state): State<AppState>,
    Path((tenant_id, node_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.get_node(&node_id) {
        Ok(Some(node)) => Json(json!({ "ok": true, "node": node })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_node_query(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(query): Json<NodeQuery>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.query_nodes(query) {
        Ok(nodes) => Json(json!({ "ok": true, "nodes": nodes })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_edge_upsert(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<EdgeWriteBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.upsert_edge(body.into_record()) {
        Ok(result) => {
            state.observability.record_mutation();
            Json(json!({ "ok": true, "edge": result })).into_response()
        }
        Err(error) => {
            state.observability.record_error();
            graph_store_error_response(error)
        }
    }
}

async fn graph_edge_get(
    State(state): State<AppState>,
    Path((tenant_id, edge_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.get_edge(&edge_id) {
        Ok(Some(edge)) => Json(json!({ "ok": true, "edge": edge })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_neighbors(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(query): Json<NeighborQuery>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.neighbors(query) {
        Ok(neighbors) => Json(json!({ "ok": true, "neighbors": neighbors })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_stats(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.stats() {
        Ok(stats) => Json(json!({ "ok": true, "stats": stats })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_verify(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.verify() {
        Ok(report) => Json(json!({ "ok": report.ok, "verify": report })).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_rebuild_indexes(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.rebuild_indexes() {
        Ok(report) => Json(json!({
            "ok": report.after.ok,
            "rebuild": report
        }))
        .into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_version_compile(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(options): Json<GraphCompileOptions>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.graph_snapshot() {
        Ok(snapshot) => {
            let pack = compile_graph_pack(&snapshot, options);
            Json(json!({
                "ok": true,
                "tenant": tenant_id,
                "pack": pack
            }))
            .into_response()
        }
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_version_diff(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphVersionDiffBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let target = match body.target {
        Some(target) => target,
        None => {
            let store = match state.tenant_graph_store(&tenant_id) {
                Ok(store) => store,
                Err(error) => return store_unavailable_response(error),
            };
            match store.graph_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => return graph_store_error_response(error),
            }
        }
    };
    let diff = diff_graph_snapshots(&body.base, &target);
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "diff": diff
    }))
    .into_response()
}

async fn graph_version_ref(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphVersionRefBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    match store.graph_snapshot() {
        Ok(snapshot) => {
            let branch = body.options.branch.clone();
            let pack = compile_graph_pack(&snapshot, body.options);
            let ref_update = update_graph_ref(
                body.repository.unwrap_or_default(),
                pack,
                branch,
                body.updated_at_unix_ms,
            );
            Json(json!({
                "ok": true,
                "tenant": tenant_id,
                "ref_update": ref_update
            }))
            .into_response()
        }
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_version_log_route(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphVersionLogBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "log": graph_version_log(&body.repository, body.target.as_deref())
    }))
    .into_response()
}

async fn graph_version_checkout(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphVersionCheckoutBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    match checkout_graph_version(&body.repository, &body.target) {
        Some(checkout) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "checkout": checkout
        }))
        .into_response(),
        None => graph_store_error_response(GraphStoreError::new(
            "version_target_not_found",
            format!(
                "version target not found or has no payloads: {}",
                body.target
            ),
        )),
    }
}

async fn graph_version_merge(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GraphVersionMergeBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }

    let ours = match body.ours {
        Some(ours) => ours,
        None => {
            let store = match state.tenant_graph_store(&tenant_id) {
                Ok(store) => store,
                Err(error) => return store_unavailable_response(error),
            };
            match store.graph_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => return graph_store_error_response(error),
            }
        }
    };
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "merge": merge_graph_snapshots(&body.base, &ours, &body.theirs, body.options)
    }))
    .into_response()
}

fn execute_tenant_command(
    state: &AppState,
    tenant_id: &str,
    command: &str,
    args: Value,
) -> axum::response::Response {
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    if is_graph_command(command) {
        return Json(execute_tenant_graph_command(
            state, tenant_id, command, args,
        ))
        .into_response();
    }
    if is_cache_command(command) {
        return Json(execute_tenant_cache_command(
            state, tenant_id, command, args,
        ))
        .into_response();
    }
    let store = match state.tenant_store(tenant_id) {
        Ok(store) => store,
        Err(error) => return store_unavailable_response(error),
    };
    let mut executor = StoreBackedRustyredExecutor::new(store);
    let response = executor.execute_request(RustyredRequest::new(command, args));
    Json(response).into_response()
}

fn execute_tenant_graph_command(
    state: &AppState,
    tenant_id: &str,
    command_name: &str,
    args: Value,
) -> RustyredResponse {
    if let Err(error) = state.store_ready() {
        return RustyredResponse::err(
            command_name,
            RustyredError::new(error.code, error.message),
            "graph:unavailable",
        );
    }
    let mut store = match state.tenant_graph_store(tenant_id) {
        Ok(store) => store,
        Err(error) => {
            return RustyredResponse::err(
                command_name,
                RustyredError::new(error.code, error.message),
                "graph:unavailable",
            )
        }
    };
    execute_graph_store_command(&mut store, command_name, args)
}

fn execute_tenant_cache_command(
    state: &AppState,
    tenant_id: &str,
    command_name: &str,
    args: Value,
) -> RustyredResponse {
    let cache = match state.tenant_graph_cache(tenant_id) {
        Ok(cache) => cache,
        Err(error) => {
            return RustyredResponse::err(
                command_name,
                RustyredError::new(error.code, error.message),
                "graph:unavailable",
            )
        }
    };
    let graph_version = match current_graph_version(state, tenant_id) {
        Ok(version) => version,
        Err(error) => {
            return RustyredResponse::err(
                command_name,
                RustyredError::new(error.code, error.message),
                "graph:unavailable",
            )
        }
    };
    execute_graph_cache_command(&cache, command_name, args, graph_version)
}

fn execute_graph_store_command(
    store: &mut TenantGraphStore,
    command_name: &str,
    args: Value,
) -> RustyredResponse {
    let command = match RustyredCommand::from_name(command_name) {
        Ok(command) => command,
        Err(error) => return RustyredResponse::err(command_name, error, "graph:unavailable"),
    };
    match command {
        RustyredCommand::GraphNodeUpsert => {
            let node = match serde_json::from_value::<NodeWriteBody>(args) {
                Ok(body) => body.into_record(),
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            let response_node = rustyred_core::RustyredNode {
                id: node.id.clone(),
                labels: node.labels.clone(),
                properties: node.properties.clone(),
            };
            match store.upsert_node(node) {
                Ok(write) => {
                    let mut response = RustyredResponse::ok(
                        command.name(),
                        "ok",
                        json!({ "write": write, "node": response_node }),
                        graph_response_hash(store),
                    );
                    response.nodes.push(response_node);
                    response
                }
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        RustyredCommand::GraphEdgeUpsert => {
            let edge = match serde_json::from_value::<EdgeWriteBody>(args) {
                Ok(body) => body.into_record(),
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            let response_edge = rustyred_core::RustyredEdge {
                from_id: edge.from_id.clone(),
                edge_type: edge.edge_type.clone(),
                to_id: edge.to_id.clone(),
                properties: edge.properties.clone(),
            };
            match store.upsert_edge(edge) {
                Ok(write) => {
                    let mut response = RustyredResponse::ok(
                        command.name(),
                        "ok",
                        json!({ "write": write, "edge": response_edge }),
                        graph_response_hash(store),
                    );
                    response.edges.push(response_edge);
                    response
                }
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        RustyredCommand::GraphNodesQuery => {
            let query = match serde_json::from_value::<NodeQuery>(args) {
                Ok(query) => query,
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            let operation = if query.label.is_some() || !query.properties.is_empty() {
                "node_index_seek"
            } else {
                "node_scan"
            };
            match store.query_nodes(query) {
                Ok(hits) => {
                    let nodes = hits
                        .iter()
                        .map(|node| rustyred_core::RustyredNode {
                            id: node.id.clone(),
                            labels: node.labels.clone(),
                            properties: node.properties.clone(),
                        })
                        .collect::<Vec<_>>();
                    let mut response = RustyredResponse::ok(
                        command.name(),
                        "ok",
                        json!({
                            "nodes": hits,
                            "plan": { "operation": operation },
                            "stats": { "returned": nodes.len() },
                        }),
                        graph_response_hash(store),
                    );
                    response.nodes = nodes;
                    response
                }
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        RustyredCommand::GraphNeighbors => {
            let query = match serde_json::from_value::<NeighborQuery>(args) {
                Ok(query) => query,
                Err(error) => {
                    return graph_command_invalid_params(command.name(), error.to_string(), store)
                }
            };
            match store.neighbors(query) {
                Ok(hits) => RustyredResponse::ok(
                    command.name(),
                    "ok",
                    json!({
                        "neighbors": hits,
                        "plan": { "operation": "adjacency_seek" },
                        "stats": { "returned": hits.len() },
                    }),
                    graph_response_hash(store),
                ),
                Err(error) => graph_command_error(command.name(), error, store),
            }
        }
        RustyredCommand::GraphStats => match store.stats() {
            Ok(stats) => RustyredResponse::ok(
                command.name(),
                "ok",
                json!({ "stats": stats }),
                graph_stats_hash(&stats),
            ),
            Err(error) => graph_command_error(command.name(), error, store),
        },
        RustyredCommand::GraphVerify => match store.verify() {
            Ok(report) => RustyredResponse::ok(
                command.name(),
                if report.ok { "ok" } else { "drift_detected" },
                json!({ "report": report }),
                graph_response_hash(store),
            ),
            Err(error) => graph_command_error(command.name(), error, store),
        },
        RustyredCommand::GraphRebuildIndexes => match store.rebuild_indexes() {
            Ok(report) => RustyredResponse::ok(
                command.name(),
                if report.after.ok {
                    "ok"
                } else {
                    "canonical_graph_problem"
                },
                json!({ "report": report }),
                graph_response_hash(store),
            ),
            Err(error) => graph_command_error(command.name(), error, store),
        },
        _ => RustyredResponse::err(
            command.name(),
            RustyredError::unsupported_command(command.name()),
            graph_response_hash(store),
        ),
    }
}

fn is_graph_command(command: &str) -> bool {
    matches!(
        command.trim().to_ascii_uppercase().as_str(),
        "RUSTYRED.GRAPH.NODE.UPSERT"
            | "RUSTYRED.GRAPH.EDGE.UPSERT"
            | "RUSTYRED.GRAPH.NODES.QUERY"
            | "RUSTYRED.GRAPH.NEIGHBORS"
            | "RUSTYRED.GRAPH.STATS"
            | "RUSTYRED.GRAPH.VERIFY"
            | "RUSTYRED.GRAPH.REBUILD_INDEXES"
            | "RUSTYRED.GRAPH.REBUILD"
    )
}

fn is_cache_command(command: &str) -> bool {
    matches!(
        command.trim().to_ascii_uppercase().as_str(),
        "RUSTYRED.CACHE.PUT"
            | "RUSTYRED.CACHE.STORE"
            | "RUSTYRED.CACHE.GET"
            | "RUSTYRED.CACHE.CHECK"
            | "RUSTYRED.CACHE.EXPLAIN"
            | "RUSTYRED.CACHE.INVALIDATE"
            | "RUSTYRED.CACHE.STATS"
    )
}

fn graph_command_invalid_params(
    command: &str,
    message: String,
    store: &TenantGraphStore,
) -> RustyredResponse {
    RustyredResponse::err(
        command,
        RustyredError::new("invalid_graph_query", message),
        graph_response_hash(store),
    )
}

fn graph_command_error(
    command: &str,
    error: GraphStoreError,
    store: &TenantGraphStore,
) -> RustyredResponse {
    RustyredResponse::err(
        command,
        RustyredError::new(error.code, error.message),
        graph_response_hash(store),
    )
}

fn execute_graph_cache_command(
    cache: &std::sync::Arc<crate::graph_cache::GraphCacheTenant>,
    command_name: &str,
    args: Value,
    graph_version: u64,
) -> RustyredResponse {
    let upper = command_name.trim().to_ascii_uppercase();
    let result = match upper.as_str() {
        "RUSTYRED.CACHE.PUT" | "RUSTYRED.CACHE.STORE" => {
            serde_json::from_value::<GraphCachePutBody>(args)
                .map_err(|error| {
                    GraphStoreError::new("invalid_graph_cache_request", error.to_string())
                })
                .and_then(|body| cache.put(body, graph_version))
                .map(|payload| {
                    RustyredResponse::ok(
                        command_name,
                        "stored",
                        json!({ "cache": payload }),
                        cache_state_hash(cache, graph_version),
                    )
                })
        }
        "RUSTYRED.CACHE.GET" => serde_json::from_value::<GraphCacheLookupBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.get(body, graph_version))
            .map(|payload| {
                RustyredResponse::ok(
                    command_name,
                    if payload.accepted {
                        "hit"
                    } else {
                        payload.reason.as_str()
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "RUSTYRED.CACHE.CHECK" => serde_json::from_value::<GraphCacheLookupBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.check(body, graph_version))
            .map(|payload| {
                RustyredResponse::ok(
                    command_name,
                    if payload.accepted {
                        "hit"
                    } else {
                        payload.reason.as_str()
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "RUSTYRED.CACHE.EXPLAIN" => serde_json::from_value::<GraphCacheLookupBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.explain(body, graph_version))
            .map(|payload| {
                RustyredResponse::ok(
                    command_name,
                    if payload.accepted {
                        "explain_hit"
                    } else {
                        payload.reason.as_str()
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "RUSTYRED.CACHE.INVALIDATE" => serde_json::from_value::<GraphCacheInvalidateBody>(args)
            .map_err(|error| GraphStoreError::new("invalid_graph_cache_request", error.to_string()))
            .and_then(|body| cache.invalidate(body, graph_version))
            .map(|payload| {
                RustyredResponse::ok(
                    command_name,
                    if payload.removed > 0 {
                        "invalidated"
                    } else {
                        "no_match"
                    },
                    json!({ "cache": payload }),
                    cache_state_hash(cache, graph_version),
                )
            }),
        "RUSTYRED.CACHE.STATS" => cache.stats(graph_version).map(|payload| {
            RustyredResponse::ok(
                command_name,
                "ok",
                json!({ "cache": payload }),
                cache_state_hash(cache, graph_version),
            )
        }),
        _ => Err(GraphStoreError::new(
            "unsupported_graph_cache_command",
            format!("unsupported graph cache command: {command_name}"),
        )),
    };
    result.unwrap_or_else(|error| {
        RustyredResponse::err(
            command_name,
            RustyredError::new(error.code, error.message),
            cache_state_hash(cache, graph_version),
        )
    })
}

fn graph_response_hash(store: &TenantGraphStore) -> String {
    store
        .stats()
        .map(|stats| graph_stats_hash(&stats))
        .unwrap_or_else(|_| "graph:unavailable".to_string())
}

fn cache_state_hash(
    cache: &std::sync::Arc<crate::graph_cache::GraphCacheTenant>,
    graph_version: u64,
) -> String {
    cache
        .stats(graph_version)
        .map(|stats| stable_hash(stats))
        .unwrap_or_else(|_| format!("cache:unavailable:{graph_version}"))
}

fn graph_stats_hash(stats: &GraphStats) -> String {
    stable_hash(stats)
}

fn current_graph_version(state: &AppState, tenant_id: &str) -> Result<u64, GraphStoreError> {
    let store = state
        .tenant_graph_store(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    Ok(store.stats()?.version)
}

fn execute_cache_put(
    state: &AppState,
    tenant_id: &str,
    body: GraphCachePutBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.put(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_get(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheLookupBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.get(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_check(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheLookupBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.check(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_explain(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheLookupBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.explain(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_invalidate(
    state: &AppState,
    tenant_id: &str,
    body: GraphCacheInvalidateBody,
) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.invalidate(body, graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_cache_stats(state: &AppState, tenant_id: &str) -> Result<Value, GraphStoreError> {
    let graph_version = current_graph_version(state, tenant_id)?;
    let cache = state
        .tenant_graph_cache(tenant_id)
        .map_err(|error| GraphStoreError::new(error.code, error.message))?;
    let payload = cache.stats(graph_version)?;
    Ok(json!({
        "ok": true,
        "tenant": tenant_id,
        "cache": payload,
    }))
}

fn execute_batch_commands(
    state: &AppState,
    tenant_id: &str,
    commands: Vec<CommandBody>,
) -> axum::response::Response {
    if let Err(error) = state.store_ready() {
        return store_unavailable_response(error);
    }
    let needs_state_store = commands
        .iter()
        .any(|item| !is_graph_command(&item.command) && !is_cache_command(&item.command));
    let mut executor = if needs_state_store {
        let store = match state.tenant_store(tenant_id) {
            Ok(store) => store,
            Err(error) => return store_unavailable_response(error),
        };
        Some(StoreBackedRustyredExecutor::new(store))
    } else {
        None
    };
    let mut graph_store: Option<TenantGraphStore> = None;
    let mut graph_cache = None;
    let mut results = Vec::with_capacity(commands.len());
    for item in commands {
        let command = item.command;
        let args = item.args;
        let response = if is_graph_command(&command) {
            if graph_store.is_none() {
                match state.tenant_graph_store(tenant_id) {
                    Ok(store) => graph_store = Some(store),
                    Err(error) => {
                        results.push(RustyredResponse::err(
                            command,
                            RustyredError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            }
            execute_graph_store_command(
                graph_store.as_mut().expect("graph store initialized"),
                &command,
                args,
            )
        } else if is_cache_command(&command) {
            if graph_cache.is_none() {
                match state.tenant_graph_cache(tenant_id) {
                    Ok(cache) => graph_cache = Some(cache),
                    Err(error) => {
                        results.push(RustyredResponse::err(
                            command,
                            RustyredError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            }
            let graph_version = if let Some(store) = graph_store.as_ref() {
                match store.stats() {
                    Ok(stats) => stats.version,
                    Err(error) => {
                        results.push(RustyredResponse::err(
                            command,
                            RustyredError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            } else {
                match current_graph_version(state, tenant_id) {
                    Ok(version) => version,
                    Err(error) => {
                        results.push(RustyredResponse::err(
                            command,
                            RustyredError::new(error.code, error.message),
                            "graph:unavailable",
                        ));
                        continue;
                    }
                }
            };
            execute_graph_cache_command(
                graph_cache.as_ref().expect("graph cache initialized"),
                &command,
                args,
                graph_version,
            )
        } else {
            executor
                .as_mut()
                .expect("state executor initialized for non-graph command")
                .execute_request(RustyredRequest::new(command, args))
        };
        results.push(response);
    }
    let state_hash = executor
        .as_ref()
        .map(|executor| executor.state().hash())
        .unwrap_or_else(|| {
            graph_store
                .as_ref()
                .map(graph_response_hash)
                .or_else(|| {
                    graph_cache.as_ref().map(|cache| {
                        cache_state_hash(
                            cache,
                            current_graph_version(state, tenant_id).unwrap_or(0),
                        )
                    })
                })
                .unwrap_or_else(|| "graph:empty_batch".to_string())
        });
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "results": results,
        "state_hash": state_hash
    }))
    .into_response()
}

fn query_surface_error_response(error: QuerySurfaceError) -> axum::response::Response {
    (error.status(), Json(error.payload())).into_response()
}

fn store_unavailable_response(error: StoreAccessError) -> axum::response::Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(error.as_payload())).into_response()
}

fn transaction_state_error(error: StoreAccessError) -> GraphStoreError {
    if error.code == "store_mode_unsupported" {
        GraphStoreError::new("unsupported_operation", error.message)
    } else {
        GraphStoreError::new(error.code, error.message)
    }
}

fn graph_store_error_response(error: GraphStoreError) -> axum::response::Response {
    (
        graph_error_status(error.code.as_str()),
        Json(json!({
            "error": error.code,
            "message": error.message
        })),
    )
        .into_response()
}

fn graph_error_status(code: &str) -> StatusCode {
    match code {
        "empty_graph_field"
        | "empty_graph_transaction"
        | "missing_graph_endpoint"
        | "tombstoned_graph_endpoint"
        | "invalid_graph_record"
        | "invalid_graph_cache_request"
        | "invalid_instant_kg_request"
        | "unsupported_graph_cache_kind"
        | "unsupported_graph_cache_command"
        | "dimension_mismatch"
        | "invalid_vector_designation"
        | "unsupported_operation" => StatusCode::BAD_REQUEST,
        "tenant_memory_quota_exceeded" => StatusCode::TOO_MANY_REQUESTS,
        "redis_graph_store_error"
        | "redcore_io_error"
        | "redcore_aof_frame_invalid"
        | "redcore_aof_checksum_mismatch"
        | "redcore_lock_poisoned"
        | "redcore_lock_unavailable"
        | "redcore_strict_mode_invalid"
        | "redcore_writer_lock_poisoned"
        | "redcore_snapshot_lock_poisoned"
        | "graph_cache_lock_poisoned" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn required_scope_for_command(command: &str) -> &'static str {
    match command.trim().to_ascii_uppercase().as_str() {
        "RUSTYRED.RUN.GET" => "run:read",
        "RUSTYRED.RUN.BEGIN" | "RUSTYRED.RUN.STEP" => "run:write",
        "RUSTYRED.CONTEXT.GET" => "context:read",
        "RUSTYRED.CONTEXT.PACK" => "context:write",
        "RUSTYRED.GRAPH.NODE.UPSERT" | "RUSTYRED.GRAPH.EDGE.UPSERT" => "graph:write",
        "RUSTYRED.STATE.HASH"
        | "RUSTYRED.DEBUG.CYPHER"
        | "RUSTYRED.CYPHER"
        | "RUSTYRED.GRAPH.NODES.QUERY"
        | "RUSTYRED.GRAPH.NEIGHBORS"
        | "RUSTYRED.GRAPH.STATS"
        | "RUSTYRED.GRAPH.VERIFY"
        | "RUSTYRED.CACHE.GET"
        | "RUSTYRED.CACHE.CHECK"
        | "RUSTYRED.CACHE.EXPLAIN"
        | "RUSTYRED.CACHE.STATS" => "graph:read",
        "RUSTYRED.GRAPH.REBUILD_INDEXES" | "RUSTYRED.GRAPH.REBUILD" => "graph:write",
        "RUSTYRED.CACHE.PUT" | "RUSTYRED.CACHE.STORE" | "RUSTYRED.CACHE.INVALIDATE" => {
            "graph:write"
        }
        _ => "run:write",
    }
}

fn cors_layer(state: &AppState) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE]);
    if state
        .config
        .allowed_origins
        .iter()
        .any(|origin| origin == "*")
    {
        layer = layer.allow_origin(Any);
    } else {
        let origins = state
            .config
            .allowed_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect::<Vec<_>>();
        if !origins.is_empty() {
            layer = layer.allow_origin(origins);
        }
    }
    layer
}

fn mcp_origin_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return true;
    };
    allowed_origins.iter().any(|allowed| {
        allowed == "*" || allowed.trim_end_matches('/') == origin.trim_end_matches('/')
    })
}

// ===== Phase 6: Graph algorithm endpoints =====

#[derive(Debug, Deserialize)]
struct PprBody {
    seeds: std::collections::HashMap<String, f64>,
    #[serde(default = "default_ppr_alpha")]
    alpha: f64,
    #[serde(default = "default_ppr_epsilon")]
    epsilon: f64,
    #[serde(default = "default_ppr_max_pushes")]
    max_pushes: usize,
    #[serde(default)]
    top_k: Option<usize>,
}

fn default_ppr_alpha() -> f64 {
    0.15
}
fn default_ppr_epsilon() -> f64 {
    1e-4
}
fn default_ppr_max_pushes() -> usize {
    200_000
}

#[derive(Debug, Deserialize)]
struct ComponentsBody {
    #[serde(default)]
    directed: bool,
}

#[derive(Debug, Deserialize)]
struct PageRankBody {
    #[serde(default = "default_pr_damping")]
    damping: f64,
    #[serde(default = "default_pr_max_iter")]
    max_iter: usize,
    #[serde(default = "default_pr_tolerance")]
    tolerance: f64,
    #[serde(default)]
    top_k: Option<usize>,
}

fn default_pr_damping() -> f64 {
    0.85
}
fn default_pr_max_iter() -> usize {
    100
}
fn default_pr_tolerance() -> f64 {
    1e-6
}

#[derive(Debug, Deserialize, Default)]
struct CommunitiesBody {}

async fn graph_algorithm_ppr(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PprBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    state.observability.record_ppr();
    let start = std::time::Instant::now();
    let outcome = (|| -> Result<Value, GraphStoreError> {
        let edges = store.list_edges()?;
        let mut adjacency: std::collections::HashMap<String, Vec<(String, f64)>> =
            std::collections::HashMap::new();
        for edge in edges.iter() {
            if edge.tombstone {
                continue;
            }
            adjacency
                .entry(edge.from_id.clone())
                .or_default()
                .push((edge.to_id.clone(), edge.effective_confidence()));
        }
        let scores = rustyred_core::personalized_pagerank(
            &adjacency,
            &body.seeds,
            body.alpha,
            body.epsilon,
            body.max_pushes,
        );
        let mut entries: Vec<(String, f64)> = scores.into_iter().collect();
        entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        if let Some(k) = body.top_k {
            entries.truncate(k);
        }
        Ok(json!({
            "ok": true,
            "tenant": tenant_id,
            "alpha": body.alpha,
            "epsilon": body.epsilon,
            "scores": entries
                .into_iter()
                .map(|(node_id, score)| json!({ "node_id": node_id, "score": score }))
                .collect::<Vec<_>>(),
        }))
    })();
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_ALGO_PPR, "ppr", nanos, 0, 0);
    match outcome {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_algorithm_components(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ComponentsBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    state.observability.record_components();
    let start = std::time::Instant::now();
    let outcome = (|| -> Result<Value, GraphStoreError> {
        let edges = store.list_edges()?;
        let components = rustyred_core::connected_components(&edges, body.directed);
        Ok(json!({
            "ok": true,
            "tenant": tenant_id,
            "directed": body.directed,
            "components": components,
            "count": components.len(),
        }))
    })();
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_ALGO_COMPONENTS, "components", nanos, 0, 0);
    match outcome {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

async fn graph_algorithm_pagerank(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PageRankBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    state.observability.record_pagerank();
    let start = std::time::Instant::now();
    let outcome = (|| -> Result<Value, GraphStoreError> {
        let edges = store.list_edges()?;
        let rank = rustyred_core::pagerank(&edges, body.damping, body.max_iter, body.tolerance);
        let mut entries: Vec<(String, f64)> = rank.into_iter().collect();
        entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        if let Some(k) = body.top_k {
            entries.truncate(k);
        }
        Ok(json!({
            "ok": true,
            "tenant": tenant_id,
            "damping": body.damping,
            "scores": entries
                .into_iter()
                .map(|(node_id, score)| json!({ "node_id": node_id, "score": score }))
                .collect::<Vec<_>>(),
        }))
    })();
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_ALGO_PAGERANK, "pagerank", nanos, 0, 0);
    match outcome {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

// ===== Phase 3: Bulk loader =====
//
// Streaming JSONL + CSV. The handler consumes the HTTP body as it arrives,
// splits on newlines via `crate::bulk::LineSplitter`, parses each line via
// the parsers in `crate::bulk`, and flushes mutations in `batch_size` chunks
// (default 500) so a large body never blocks the worker on a single
// transaction. CSV branches use `text/csv` Content-Type and read the first
// row as header (or `?headers=...` query parameter); edges require
// `?from_col=...&to_col=...` (or default JSONL fields).

#[derive(Debug, Default, Deserialize)]
pub struct BulkQuery {
    #[serde(default)]
    pub batch_size: Option<usize>,
    /// Comma-separated header list. If absent and Content-Type is text/csv,
    /// the first row of the body is treated as the header row.
    #[serde(default)]
    pub headers: Option<String>,
    /// CSV-only: name of the source column for edges.
    #[serde(default)]
    pub from_col: Option<String>,
    /// CSV-only: name of the target column for edges.
    #[serde(default)]
    pub to_col: Option<String>,
}

async fn graph_bulk_nodes(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<BulkQuery>,
    body: Body,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/jsonl")
        .to_string();
    let is_csv = content_type.starts_with("text/csv");

    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "bulk_body_read_failed",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let batch_size = query.batch_size.unwrap_or(500).max(1);
    let mut splitter = crate::bulk::LineSplitter::default();
    let mut produced_lines = splitter.feed(&bytes);
    produced_lines.extend(splitter.flush());

    let mut csv_parser: Option<crate::bulk::CsvNodeParser> = None;
    let mut first_data_line = 1usize;
    if is_csv {
        if let Some(header_str) = query.headers.as_deref() {
            csv_parser = Some(crate::bulk::CsvNodeParser::new(
                header_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect(),
            ));
        } else if !produced_lines.is_empty() {
            let first = produced_lines.remove(0);
            first_data_line = 2;
            csv_parser = Some(crate::bulk::CsvNodeParser::new(
                first.split(',').map(|s| s.trim().to_string()).collect(),
            ));
        }
    }

    let mut inserted = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<Value> = Vec::new();
    let mut batches = 0usize;
    let mut pending: Vec<(usize, rustyred_core::NodeRecord)> = Vec::with_capacity(batch_size);

    for (line_no, line) in produced_lines.iter().enumerate() {
        let source_line = first_data_line + line_no;
        let parsed = if is_csv {
            csv_parser
                .as_ref()
                .map(|parser| parser.parse(line))
                .unwrap_or_else(|| Err("csv parser not initialized".into()))
        } else {
            crate::bulk::jsonl_parse_node(line)
        };
        match parsed {
            Ok(node) => {
                pending.push((source_line, node));
                if pending.len() >= batch_size {
                    flush_node_batch(
                        &mut store,
                        &state,
                        &tenant_id,
                        &mut pending,
                        &mut inserted,
                        &mut failed,
                        &mut errors,
                    );
                    batches += 1;
                }
            }
            Err(err) => {
                failed += 1;
                if errors.len() < 32 {
                    errors.push(bulk_parse_error(source_line, &err));
                }
            }
        }
    }

    if !pending.is_empty() {
        flush_node_batch(
            &mut store,
            &state,
            &tenant_id,
            &mut pending,
            &mut inserted,
            &mut failed,
            &mut errors,
        );
        batches += 1;
    }

    Json(json!({
        "ok": failed == 0,
        "tenant": tenant_id,
        "inserted": inserted,
        "failed": failed,
        "errors": errors,
        "batches": batches,
    }))
    .into_response()
}

fn flush_node_batch(
    store: &mut TenantGraphStore,
    state: &AppState,
    tenant_id: &str,
    pending: &mut Vec<(usize, rustyred_core::NodeRecord)>,
    inserted: &mut usize,
    failed: &mut usize,
    errors: &mut Vec<Value>,
) {
    if pending.is_empty() {
        return;
    }
    let snapshot: Vec<(usize, rustyred_core::NodeRecord)> = pending.drain(..).collect();
    let mutations: Vec<rustyred_core::GraphMutation> = snapshot
        .iter()
        .map(|(_, node)| rustyred_core::GraphMutation::NodeUpsert(node.clone()))
        .collect();
    let batch = rustyred_core::GraphMutationBatch::new(mutations);
    match store.commit_batch(batch) {
        Ok(_transaction) => {
            *inserted += snapshot.len();
            for (_, node) in &snapshot {
                state.observability.record_mutation();
                state.maybe_index_node_spatially(tenant_id, node);
                state.maybe_index_node_fulltext(tenant_id, node);
            }
        }
        Err(_) => {
            for (line, node) in snapshot {
                let record_id = node.id.clone();
                let batch = rustyred_core::GraphMutationBatch::new([
                    rustyred_core::GraphMutation::NodeUpsert(node.clone()),
                ]);
                match store.commit_batch(batch) {
                    Ok(_) => {
                        *inserted += 1;
                        state.observability.record_mutation();
                        state.maybe_index_node_spatially(tenant_id, &node);
                        state.maybe_index_node_fulltext(tenant_id, &node);
                    }
                    Err(err) => {
                        *failed += 1;
                        if errors.len() < 32 {
                            errors.push(bulk_store_error(line, &record_id, &err));
                        }
                    }
                }
            }
        }
    }
}

async fn graph_bulk_edges(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<BulkQuery>,
    body: Body,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let mut store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/jsonl")
        .to_string();
    let is_csv = content_type.starts_with("text/csv");

    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "bulk_body_read_failed",
                    "message": err.to_string(),
                })),
            )
                .into_response();
        }
    };

    let batch_size = query.batch_size.unwrap_or(500).max(1);
    let mut splitter = crate::bulk::LineSplitter::default();
    let mut produced_lines = splitter.feed(&bytes);
    produced_lines.extend(splitter.flush());

    let mut csv_parser: Option<crate::bulk::CsvEdgeParser> = None;
    let mut first_data_line = 1usize;
    if is_csv {
        let from_col = query.from_col.as_deref().unwrap_or("from_id");
        let to_col = query.to_col.as_deref().unwrap_or("to_id");
        if let Some(header_str) = query.headers.as_deref() {
            let header_vec: Vec<String> = header_str
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            match crate::bulk::CsvEdgeParser::new(header_vec, from_col, to_col) {
                Ok(parser) => csv_parser = Some(parser),
                Err(err) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": "bulk_csv_header_invalid",
                            "message": err,
                        })),
                    )
                        .into_response();
                }
            }
        } else if !produced_lines.is_empty() {
            let first = produced_lines.remove(0);
            first_data_line = 2;
            let header_vec: Vec<String> = first.split(',').map(|s| s.trim().to_string()).collect();
            match crate::bulk::CsvEdgeParser::new(header_vec, from_col, to_col) {
                Ok(parser) => csv_parser = Some(parser),
                Err(err) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": "bulk_csv_header_invalid",
                            "message": err,
                        })),
                    )
                        .into_response();
                }
            }
        }
    }

    let mut inserted = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<Value> = Vec::new();
    let mut batches = 0usize;
    let mut pending: Vec<(usize, rustyred_core::EdgeRecord)> = Vec::with_capacity(batch_size);

    for (line_no, line) in produced_lines.iter().enumerate() {
        let source_line = first_data_line + line_no;
        let parsed = if is_csv {
            csv_parser
                .as_ref()
                .map(|parser| parser.parse(line))
                .unwrap_or_else(|| Err("csv parser not initialized".into()))
        } else {
            crate::bulk::jsonl_parse_edge(line)
        };
        match parsed {
            Ok(edge) => {
                pending.push((source_line, edge));
                if pending.len() >= batch_size {
                    flush_edge_batch(
                        &mut store,
                        &state,
                        &mut pending,
                        &mut inserted,
                        &mut failed,
                        &mut errors,
                    );
                    batches += 1;
                }
            }
            Err(err) => {
                failed += 1;
                if errors.len() < 32 {
                    errors.push(bulk_parse_error(source_line, &err));
                }
            }
        }
    }

    if !pending.is_empty() {
        flush_edge_batch(
            &mut store,
            &state,
            &mut pending,
            &mut inserted,
            &mut failed,
            &mut errors,
        );
        batches += 1;
    }

    Json(json!({
        "ok": failed == 0,
        "tenant": tenant_id,
        "inserted": inserted,
        "failed": failed,
        "errors": errors,
        "batches": batches,
    }))
    .into_response()
}

fn flush_edge_batch(
    store: &mut TenantGraphStore,
    state: &AppState,
    pending: &mut Vec<(usize, rustyred_core::EdgeRecord)>,
    inserted: &mut usize,
    failed: &mut usize,
    errors: &mut Vec<Value>,
) {
    if pending.is_empty() {
        return;
    }
    let snapshot: Vec<(usize, rustyred_core::EdgeRecord)> = pending.drain(..).collect();
    let mutations: Vec<rustyred_core::GraphMutation> = snapshot
        .iter()
        .map(|(_, edge)| rustyred_core::GraphMutation::EdgeUpsert(edge.clone()))
        .collect();
    let batch = rustyred_core::GraphMutationBatch::new(mutations);
    match store.commit_batch(batch) {
        Ok(_transaction) => {
            *inserted += snapshot.len();
            for _ in snapshot {
                state.observability.record_mutation();
            }
        }
        Err(_) => {
            for (line, edge) in snapshot {
                let record_id = edge.id.clone();
                let batch = rustyred_core::GraphMutationBatch::new([
                    rustyred_core::GraphMutation::EdgeUpsert(edge),
                ]);
                match store.commit_batch(batch) {
                    Ok(_) => {
                        *inserted += 1;
                        state.observability.record_mutation();
                    }
                    Err(err) => {
                        *failed += 1;
                        if errors.len() < 32 {
                            errors.push(bulk_store_error(line, &record_id, &err));
                        }
                    }
                }
            }
        }
    }
}

fn bulk_parse_error(line: usize, message: &str) -> Value {
    json!({
        "line": line,
        "code": bulk_error_code(message),
        "message": message,
    })
}

fn bulk_store_error(line: usize, record_id: &str, error: &rustyred_core::GraphStoreError) -> Value {
    json!({
        "line": line,
        "code": error.code,
        "message": error.message,
        "record_id": record_id,
    })
}

fn bulk_error_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("properties") {
        "invalid_properties"
    } else if lower.contains("json") || lower.contains("expected") || lower.contains("eof") {
        "invalid_json"
    } else if lower.contains("missing") {
        "missing_required_field"
    } else if lower.contains("csv") {
        "invalid_csv_row"
    } else {
        "invalid_record"
    }
}

// ===== Phase 5: Full-text endpoints =====

#[derive(Debug, Deserialize)]
struct FullTextDesignateBody {
    label: String,
    property: String,
}

#[derive(Debug, Deserialize)]
struct FullTextSearchBody {
    #[serde(default)]
    label: Option<String>,
    property: String,
    query: String,
    #[serde(default = "default_fulltext_k")]
    k: usize,
}

fn default_fulltext_k() -> usize {
    10
}

async fn graph_fulltext_designate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<FullTextDesignateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    match state.designate_fulltext_property(&tenant_id, &body.label, &body.property) {
        Ok(()) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "label": body.label,
            "property": body.property,
        }))
        .into_response(),
        Err(error) => {
            state.observability.record_error();
            store_unavailable_response(error)
        }
    }
}

async fn graph_fulltext_search(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<FullTextSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    state.observability.record_fulltext_search();
    let detail = format!(
        "label={} property={}",
        body.label.as_deref().unwrap_or("*"),
        body.property
    );
    let start = std::time::Instant::now();
    let outcome = state.fulltext_search(
        &tenant_id,
        body.label.as_deref(),
        &body.property,
        &body.query,
        body.k,
    );
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_FULLTEXT_SEARCH, &detail, nanos, 0, 0);
    match outcome {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|(id, score)| json!({ "node_id": id, "score": score }))
                .collect();
            Json(json!({
                "ok": true,
                "tenant": tenant_id,
                "results": items,
            }))
            .into_response()
        }
        Err(error) => store_unavailable_response(error),
    }
}

// ===== Phase 8: Spatial endpoints =====

#[derive(Debug, Deserialize)]
struct SpatialDesignateBody {
    label: String,
    lat_property: String,
    lon_property: String,
    #[serde(default = "default_h3_resolution")]
    resolution: u8,
}

fn default_h3_resolution() -> u8 {
    8
}

#[derive(Debug, Deserialize)]
struct SpatialRadiusBody {
    label: String,
    lat_property: String,
    lon_property: String,
    lat: f64,
    lon: f64,
    radius_km: f64,
}

#[derive(Debug, Deserialize)]
struct SpatialBboxBody {
    label: String,
    lat_property: String,
    lon_property: String,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
}

async fn graph_spatial_designate(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SpatialDesignateBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:write",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    match state.designate_spatial_property(
        &tenant_id,
        &body.label,
        &body.lat_property,
        &body.lon_property,
        body.resolution,
    ) {
        Ok(()) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "label": body.label,
            "lat_property": body.lat_property,
            "lon_property": body.lon_property,
            "resolution": body.resolution,
        }))
        .into_response(),
        Err(error) => {
            state.observability.record_error();
            store_unavailable_response(error)
        }
    }
}

async fn graph_spatial_radius(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SpatialRadiusBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    state.observability.record_spatial_search();
    match state.spatial_radius_search(
        &tenant_id,
        &body.label,
        &body.lat_property,
        &body.lon_property,
        body.lat,
        body.lon,
        body.radius_km,
    ) {
        Ok(ids) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "count": ids.len(),
            "node_ids": ids,
        }))
        .into_response(),
        Err(error) => store_unavailable_response(error),
    }
}

async fn graph_spatial_bbox(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SpatialBboxBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    state.observability.record_spatial_search();
    match state.spatial_bbox_search(
        &tenant_id,
        &body.label,
        &body.lat_property,
        &body.lon_property,
        body.min_lat,
        body.min_lon,
        body.max_lat,
        body.max_lon,
    ) {
        Ok(ids) => Json(json!({
            "ok": true,
            "tenant": tenant_id,
            "count": ids.len(),
            "node_ids": ids,
        }))
        .into_response(),
        Err(error) => store_unavailable_response(error),
    }
}

async fn graph_algorithm_communities(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(_body): Json<CommunitiesBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let store = match state.tenant_graph_store(&tenant_id) {
        Ok(s) => s,
        Err(error) => return store_unavailable_response(error),
    };
    state.observability.record_communities();
    let start = std::time::Instant::now();
    let outcome = (|| -> Result<Value, GraphStoreError> {
        let edges = store.list_edges()?;
        let (community, modularity) = rustyred_core::label_propagation_communities(&edges);
        let mut entries: Vec<Value> = community
            .into_iter()
            .map(|(node_id, c)| json!({ "node_id": node_id, "community_id": c }))
            .collect();
        entries.sort_by(|a, b| {
            a["node_id"]
                .as_str()
                .unwrap_or("")
                .cmp(b["node_id"].as_str().unwrap_or(""))
        });
        Ok(json!({
            "ok": true,
            "tenant": tenant_id,
            "algorithm": "label_propagation",
            "communities": entries,
            "modularity": modularity,
        }))
    })();
    let nanos = start.elapsed().as_nanos() as u64;
    state
        .observability
        .record_query_timing(KIND_ALGO_COMMUNITIES, "communities", nanos, 0, 0);
    match outcome {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => graph_store_error_response(error),
    }
}

// ===== Harness Instant KG endpoints =====

#[derive(Debug, Deserialize, Default)]
struct InstantKgViewBody {
    #[serde(default)]
    manifest: Option<CodeKgManifest>,
    #[serde(default)]
    delta: Option<SessionDelta>,
}

#[derive(Debug, Deserialize)]
struct InstantKgPprBody {
    #[serde(flatten)]
    view: InstantKgViewBody,
    seeds: std::collections::HashMap<String, f64>,
    #[serde(default = "default_ppr_alpha")]
    alpha: f64,
    #[serde(default = "default_ppr_epsilon")]
    epsilon: f64,
    #[serde(default = "default_ppr_max_pushes")]
    max_pushes: usize,
    #[serde(default = "default_instant_kg_top_k")]
    top_k: usize,
}

#[derive(Debug, Deserialize)]
struct InstantKgImpactBody {
    #[serde(flatten)]
    view: InstantKgViewBody,
    #[serde(default)]
    seed: Option<String>,
    #[serde(default)]
    symbol_name: Option<String>,
    #[serde(default = "default_impact_direction")]
    direction: String,
    #[serde(default = "default_impact_depth")]
    max_depth: usize,
}

#[derive(Debug, Deserialize)]
struct InstantKgRelatedBody {
    #[serde(flatten)]
    view: InstantKgViewBody,
    seed: String,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default = "default_instant_kg_top_k")]
    top_k: usize,
}

#[derive(Debug, Deserialize)]
struct InstantKgSearchBody {
    #[serde(flatten)]
    view: InstantKgViewBody,
    query: String,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default = "default_instant_kg_top_k")]
    top_k: usize,
}

#[derive(Debug, Deserialize)]
struct InstantKgExplainEdgeBody {
    #[serde(flatten)]
    view: InstantKgViewBody,
    src: String,
    dst: String,
}

fn default_instant_kg_top_k() -> usize {
    10
}

fn default_impact_direction() -> String {
    "out".to_string()
}

fn default_impact_depth() -> usize {
    2
}

fn instant_kg_view(
    state: &AppState,
    tenant_id: &str,
    body: InstantKgViewBody,
) -> Result<HarnessInstantKg, axum::response::Response> {
    let store = state
        .tenant_graph_store(tenant_id)
        .map_err(store_unavailable_response)?;
    let base = store.graph_snapshot().map_err(graph_store_error_response)?;
    Ok(HarnessInstantKg::new(
        base,
        body.manifest,
        body.delta.unwrap_or_default(),
    ))
}

fn instant_kg_direction(value: &str) -> Direction {
    if value.eq_ignore_ascii_case("in") || value.eq_ignore_ascii_case("incoming") {
        Direction::In
    } else {
        Direction::Out
    }
}

async fn instant_kg_status(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InstantKgViewBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let view = match instant_kg_view(&state, &tenant_id, body) {
        Ok(view) => view,
        Err(response) => return response,
    };
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "status": view.status(),
        "stats": view.stats(),
    }))
    .into_response()
}

async fn instant_kg_ppr(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InstantKgPprBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let view = match instant_kg_view(&state, &tenant_id, body.view) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let results = view.ppr(
        &body.seeds,
        body.alpha,
        body.epsilon,
        body.max_pushes,
        body.top_k,
    );
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "status": view.status(),
        "results": results,
    }))
    .into_response()
}

async fn instant_kg_impact(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InstantKgImpactBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let InstantKgImpactBody {
        view: view_body,
        seed,
        symbol_name,
        direction,
        max_depth,
    } = body;
    let seed = seed
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let symbol_name = symbol_name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if seed.is_none() && symbol_name.is_none() {
        return graph_store_error_response(GraphStoreError::new(
            "invalid_instant_kg_request",
            "instant KG impact requires seed or symbol_name",
        ));
    }
    let direction = instant_kg_direction(&direction);
    let view = match instant_kg_view(&state, &tenant_id, view_body) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let seed = match seed {
        Some(seed) => seed,
        None => match symbol_name
            .as_deref()
            .and_then(|symbol| view.resolve_symbol_name(symbol))
        {
            Some(seed) => seed,
            None => {
                return graph_store_error_response(GraphStoreError::new(
                    "invalid_instant_kg_request",
                    "instant KG impact could not resolve symbol_name",
                ))
            }
        },
    };
    let results = view.impact(&seed, direction, max_depth);
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "seed": seed,
        "status": view.status(),
        "results": results,
    }))
    .into_response()
}

async fn instant_kg_related_objects(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InstantKgRelatedBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let view = match instant_kg_view(&state, &tenant_id, body.view) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let results = view.related_objects(&body.seed, &body.kinds, body.top_k);
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "seed": body.seed,
        "status": view.status(),
        "results": results,
    }))
    .into_response()
}

async fn instant_kg_search(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InstantKgSearchBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let view = match instant_kg_view(&state, &tenant_id, body.view) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let results = view.search(&body.query, &body.kinds, body.top_k);
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "query": body.query,
        "status": view.status(),
        "results": results,
    }))
    .into_response()
}

async fn instant_kg_explain_edge(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InstantKgExplainEdgeBody>,
) -> impl IntoResponse {
    if let Err(status) = require_scope(
        &headers,
        &state.config.api_tokens,
        "graph:read",
        state.config.require_auth,
    ) {
        return status.into_response();
    }
    let view = match instant_kg_view(&state, &tenant_id, body.view) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let explanations = view.explain_edge(&body.src, &body.dst);
    Json(json!({
        "ok": true,
        "tenant": tenant_id,
        "src": body.src,
        "dst": body.dst,
        "status": view.status(),
        "explanations": explanations,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::body::{to_bytes, Body};
    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::Json;
    use serde_json::{json, Value};

    use super::{
        commit_batch_with_indexes, crawl_submit, default_ppr_alpha, default_ppr_epsilon,
        default_ppr_max_pushes, default_pr_damping, default_pr_max_iter, default_pr_tolerance,
        execute_graph_store_command, execute_tenant_cache_command, execute_tenant_command,
        federate_submit, graph_algorithm_communities, graph_algorithm_components,
        graph_algorithm_pagerank, graph_algorithm_ppr, graph_bulk_edges, graph_bulk_nodes,
        graph_error_status, graph_fulltext_search, graph_vector_hybrid, graph_vector_search,
        graph_version_checkout, graph_version_compile, graph_version_diff, graph_version_log_route,
        graph_version_merge, graph_version_ref, instant_kg_explain_edge, instant_kg_impact,
        instant_kg_ppr, instant_kg_search, instant_kg_status, is_cache_command, is_graph_command,
        mcp_origin_allowed, public_cypher, public_peer_id_from_private_key,
        required_scope_for_command, search_json, sign_web_commons_fragment,
        submit_web_commons_fragment, transaction_begin, transaction_commit, transaction_rollback,
        BulkQuery, CommunitiesBody, ComponentsBody, CrawlRouteBody, FederateSubmitBody,
        FullTextSearchBody, GraphVersionCheckoutBody, GraphVersionDiffBody, GraphVersionLogBody,
        GraphVersionMergeBody, GraphVersionRefBody, HybridSearchBody, InstantKgExplainEdgeBody,
        InstantKgImpactBody, InstantKgPprBody, InstantKgSearchBody, InstantKgViewBody,
        PageRankBody, PprBody, PublicCypherBody, SearchQuery, TransactionBeginBody,
        TransactionMutationBody, VectorSearchBody,
    };
    use crate::{
        config::{Config, StorageMode, TenantConfigOverride},
        metrics::diagnostics_config,
        state::AppState,
    };
    use rustyred_core::{GraphMutation, GraphMutationBatch, NodeRecord, RedCoreDurability};
    use rustyred_search::{
        DomainRecord, LinkEdge, PageRecord, SnapshotRecord, SourceLicense, WebCommonsFragment,
        LABEL_WEB_COMMONS_PEER,
    };

    async fn response_payload_json(response: axum::response::Response) -> Value {
        serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    fn memory_product_state() -> AppState {
        AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::None,
            snapshot_interval_writes: 0,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: Default::default(),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: rustyred_core::HybridScoringConfig::default(),
            redis_url: "not-a-redis-url".to_string(),
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
        })
    }

    fn test_private_key(byte_hex: &str) -> String {
        byte_hex.repeat(32)
    }

    fn test_fragment(
        key_byte_hex: &str,
        page_id: &str,
        url: &str,
        fetched_at: i64,
    ) -> WebCommonsFragment {
        let private_key = test_private_key(key_byte_hex);
        let peer_id = public_peer_id_from_private_key(&private_key).unwrap();
        let domain = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        WebCommonsFragment {
            protocol_version: rustyred_search::WEB_COMMONS_PROTOCOL_VERSION,
            peer_id,
            graph_delta_hash: format!("blake3:{key_byte_hex}:{page_id}"),
            pages: vec![PageRecord {
                id: page_id.to_string(),
                url: url.to_string(),
                domain: domain.clone(),
                title: None,
                status: Some(200),
                fetched_at: Some(fetched_at.to_string()),
                source_class: "unknown".to_string(),
            }],
            snapshots: vec![SnapshotRecord {
                id: format!("content_snapshot:{key_byte_hex}:{page_id}"),
                page_id: page_id.to_string(),
                content_hash: format!("hash-{key_byte_hex}-{page_id}"),
                text: format!("snapshot text for {page_id}"),
                content_type: Some("text/html".to_string()),
            }],
            domains: vec![DomainRecord {
                domain: domain.clone(),
                page_ids: vec![page_id.to_string()],
            }],
            edges: Vec::new(),
            provenance: None,
            source_licenses: vec![SourceLicense {
                domain,
                source_graph: "test".to_string(),
                source_license: "CC0".to_string(),
            }],
            signature: String::new(),
        }
    }

    fn linked_fragment(
        key_byte_hex: &str,
        source_id: &str,
        source_url: &str,
        target_id: &str,
        target_url: &str,
        fetched_at: i64,
    ) -> WebCommonsFragment {
        let mut fragment = test_fragment(key_byte_hex, target_id, target_url, fetched_at);
        let source_domain = source_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        fragment.pages.push(PageRecord {
            id: source_id.to_string(),
            url: source_url.to_string(),
            domain: source_domain.clone(),
            title: None,
            status: Some(200),
            fetched_at: Some(fetched_at.to_string()),
            source_class: "blog".to_string(),
        });
        fragment.domains.push(DomainRecord {
            domain: source_domain.clone(),
            page_ids: vec![source_id.to_string()],
        });
        fragment.source_licenses.push(SourceLicense {
            domain: source_domain,
            source_graph: "test".to_string(),
            source_license: "CC0".to_string(),
        });
        fragment.edges.push(LinkEdge {
            from_page_id: source_id.to_string(),
            to_page_id: target_id.to_string(),
            anchor: None,
        });
        fragment.pages.sort_by(|left, right| left.id.cmp(&right.id));
        fragment
    }

    fn verified_peer_node(id: &str, peer_id: &str) -> NodeRecord {
        NodeRecord::new(
            id,
            [LABEL_WEB_COMMONS_PEER],
            json!({
                "peer_id": peer_id,
                "trust_tier": "verified",
                "trust_weight": 1.0,
            }),
        )
    }

    #[tokio::test]
    async fn search_json_reads_crawled_page_graph() {
        let state = memory_product_state();
        let output = rustyred_search::build_v2_fixture_crawl(
            rustyred_search::CrawlRequest::new(
                "search-route",
                vec!["http://example.com/apple".to_string()],
            ),
            &[rustyred_search::FetchedPage::html(
                "http://example.com/apple",
                "<html><body>apple cultivation notes</body></html>",
            )],
        )
        .unwrap();
        state
            .tenant_graph_store("default")
            .unwrap()
            .commit_batch(output.graph.batch)
            .unwrap();

        let response = search_json(
            State(state),
            HeaderMap::new(),
            Query(SearchQuery {
                q: Some("apple".to_string()),
                tenant: Some("default".to_string()),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["tenant"], "default");
        assert_eq!(payload["search"]["matched_count"], 1);
        assert_eq!(
            payload["search"]["hits"][0]["url"],
            "http://example.com/apple"
        );
    }

    #[tokio::test]
    async fn crawled_content_updates_fulltext_designations() {
        let state = memory_product_state();
        let tenant_id = "flint";
        state
            .designate_fulltext_property(tenant_id, "ContentSnapshot", "text")
            .unwrap();
        let output = rustyred_search::build_v2_fixture_crawl(
            rustyred_search::CrawlRequest::new(
                "civic-fulltext",
                vec!["https://example.com/flint-carriage-town".to_string()],
            ),
            &[rustyred_search::FetchedPage::html(
                "https://example.com/flint-carriage-town",
                "<html><body>Carriage Town porchfest reconstruction route notes</body></html>",
            )],
        )
        .unwrap();
        let mut store = state.tenant_graph_store(tenant_id).unwrap();
        commit_batch_with_indexes(&state, tenant_id, &mut store, output.graph.batch).unwrap();

        let response = graph_fulltext_search(
            State(state),
            Path(tenant_id.to_string()),
            HeaderMap::new(),
            Json(FullTextSearchBody {
                label: Some("ContentSnapshot".to_string()),
                property: "text".to_string(),
                query: "porchfest reconstruction".to_string(),
                k: 5,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["tenant"], tenant_id);
        assert_eq!(payload["results"].as_array().unwrap().len(), 1);
        assert!(payload["results"][0]["node_id"]
            .as_str()
            .unwrap()
            .starts_with("content_snapshot:"));
    }

    #[tokio::test]
    async fn crawl_route_rejects_empty_seed_lists_before_fetching() {
        let response = crawl_submit(
            State(memory_product_state()),
            HeaderMap::new(),
            Json(CrawlRouteBody {
                tenant: None,
                run_id: Some("empty-crawl".to_string()),
                seeds: Vec::new(),
                budget: None,
                scope: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["error"], "rustyweb_crawl_error");
    }

    #[tokio::test]
    async fn crawl_federation_status_is_receipt_shaped_when_hub_missing() {
        let mut request = rustyred_search::CrawlRequest::new(
            "federation-status",
            vec!["https://example.com/apple".to_string()],
        );
        request.scope.federable = true;
        let output = rustyred_search::build_v2_fixture_crawl(
            request.clone(),
            &[rustyred_search::FetchedPage::html(
                "https://example.com/apple",
                "<html><body>apple cultivation notes</body></html>",
            )],
        )
        .unwrap();

        let status =
            submit_web_commons_fragment(&memory_product_state(), "default", &request, &output)
                .await;

        assert_eq!(status["enabled"], true);
        assert_eq!(status["submitted"], false);
        assert_eq!(status["reason"], "hub_url_missing");
    }

    #[tokio::test]
    async fn federate_submit_accepts_federable_receipts_as_noop() {
        let response = federate_submit(
            State(memory_product_state()),
            HeaderMap::new(),
            Json(FederateSubmitBody {
                tenant: None,
                federable: Some(true),
                graph_delta_hash: Some("blake3:test".to_string()),
                receipt: None,
                fragment: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["accepted"], true);
        assert_eq!(payload["merged"], false);
        assert_eq!(payload["status"], "validated_noop");
        assert_eq!(payload["graph_delta_hash"], "blake3:test");
    }

    #[tokio::test]
    async fn federate_submit_rejects_unsigned_web_commons_fragment() {
        let fragment = test_fragment("01", "page:target", "https://target.example/", 1000);

        let response = federate_submit(
            State(memory_product_state()),
            HeaderMap::new(),
            Json(FederateSubmitBody {
                tenant: None,
                federable: Some(true),
                graph_delta_hash: None,
                receipt: None,
                fragment: Some(fragment),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["error"], "unsigned_fragment");
    }

    #[tokio::test]
    async fn federate_submit_merges_signed_fragment_as_probationary() {
        let private_key = test_private_key("01");
        let mut fragment = test_fragment("01", "page:target", "https://target.example/", 1000);
        sign_web_commons_fragment(&mut fragment, &private_key).unwrap();

        let response = federate_submit(
            State(memory_product_state()),
            HeaderMap::new(),
            Json(FederateSubmitBody {
                tenant: None,
                federable: Some(true),
                graph_delta_hash: None,
                receipt: None,
                fragment: Some(fragment),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["merged"], true);
        assert_eq!(
            payload["receipt"]["dispositions"][0]["disposition"],
            "probationary"
        );
    }

    #[tokio::test]
    async fn federate_submit_rejects_blocked_peer() {
        let private_key = test_private_key("03");
        let peer_id = public_peer_id_from_private_key(&private_key).unwrap();
        let state = memory_product_state();
        state
            .tenant_graph_store("default")
            .unwrap()
            .commit_batch(GraphMutationBatch::new([GraphMutation::NodeUpsert(
                NodeRecord::new(
                    "peer:blocked",
                    [LABEL_WEB_COMMONS_PEER],
                    json!({
                        "peer_id": peer_id,
                        "trust_tier": "blocked",
                        "trust_weight": 0.0,
                    }),
                ),
            )]))
            .unwrap();
        let mut fragment = test_fragment("03", "page:target", "https://target.example/", 1000);
        sign_web_commons_fragment(&mut fragment, &private_key).unwrap();

        let response = federate_submit(
            State(state),
            HeaderMap::new(),
            Json(FederateSubmitBody {
                tenant: None,
                federable: Some(true),
                graph_delta_hash: None,
                receipt: None,
                fragment: Some(fragment),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["error"], "peer_blocked");
    }

    #[tokio::test]
    async fn federate_submit_promotes_independently_supported_page() {
        let state = memory_product_state();
        let private_one = test_private_key("04");
        let private_two = test_private_key("05");
        let peer_one = public_peer_id_from_private_key(&private_one).unwrap();
        let peer_two = public_peer_id_from_private_key(&private_two).unwrap();
        state
            .tenant_graph_store("default")
            .unwrap()
            .commit_batch(GraphMutationBatch::new([
                GraphMutation::NodeUpsert(verified_peer_node("peer:one", &peer_one)),
                GraphMutation::NodeUpsert(verified_peer_node("peer:two", &peer_two)),
            ]))
            .unwrap();

        let mut first = linked_fragment(
            "04",
            "page:source-a",
            "https://a.example/",
            "page:target",
            "https://target.example/",
            1000,
        );
        sign_web_commons_fragment(&mut first, &private_one).unwrap();
        let first_response = federate_submit(
            State(state.clone()),
            HeaderMap::new(),
            Json(FederateSubmitBody {
                tenant: None,
                federable: Some(true),
                graph_delta_hash: None,
                receipt: None,
                fragment: Some(first),
            }),
        )
        .await;
        assert_eq!(first_response.status(), StatusCode::OK);

        let mut second = linked_fragment(
            "05",
            "page:source-c",
            "https://c.example/",
            "page:target",
            "https://target.example/",
            2000,
        );
        sign_web_commons_fragment(&mut second, &private_two).unwrap();
        let second_response = federate_submit(
            State(state),
            HeaderMap::new(),
            Json(FederateSubmitBody {
                tenant: None,
                federable: Some(true),
                graph_delta_hash: None,
                receipt: None,
                fragment: Some(second),
            }),
        )
        .await;

        assert_eq!(second_response.status(), StatusCode::OK);
        let payload = response_payload_json(second_response).await;
        let dispositions = payload["receipt"]["dispositions"].as_array().unwrap();
        let target = dispositions
            .iter()
            .find(|disposition| disposition["page_id"] == "page:target")
            .unwrap();
        assert_eq!(target["disposition"], "canonical");
    }

    #[test]
    fn maps_core_commands_to_product_scopes() {
        assert_eq!(required_scope_for_command("RUSTYRED.RUN.GET"), "run:read");
        assert_eq!(
            required_scope_for_command("RUSTYRED.RUN.BEGIN"),
            "run:write"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.CONTEXT.PACK"),
            "context:write"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.DEBUG.CYPHER"),
            "graph:read"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.GRAPH.NODE.UPSERT"),
            "graph:write"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.GRAPH.EDGE.UPSERT"),
            "graph:write"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.GRAPH.NODES.QUERY"),
            "graph:read"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.GRAPH.STATS"),
            "graph:read"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.GRAPH.VERIFY"),
            "graph:read"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.GRAPH.REBUILD_INDEXES"),
            "graph:write"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.CACHE.CHECK"),
            "graph:read"
        );
        assert_eq!(
            required_scope_for_command("RUSTYRED.CACHE.PUT"),
            "graph:write"
        );
    }

    #[test]
    fn detects_graph_commands_case_insensitively() {
        assert!(is_graph_command("rustyred.graph.node.upsert"));
        assert!(is_graph_command(" RUSTYRED.GRAPH.NEIGHBORS "));
        assert!(is_graph_command("RUSTYRED.GRAPH.VERIFY"));
        assert!(is_graph_command("rustyred.graph.rebuild_indexes"));
        assert!(!is_graph_command("RUSTYRED.RUN.BEGIN"));
        assert!(is_cache_command("rustyred.cache.check"));
        assert!(is_cache_command(" RUSTYRED.CACHE.PUT "));
        assert!(!is_cache_command("RUSTYRED.RUN.BEGIN"));
    }

    #[test]
    fn graph_commands_share_store_unavailable_http_status() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Redis,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::AofEverysec,
            snapshot_interval_writes: 1_000,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: Default::default(),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: rustyred_core::HybridScoringConfig::default(),
            redis_url: "not-a-redis-url".to_string(),
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
        });

        let response =
            execute_tenant_command(&state, "tenant-a", "RUSTYRED.GRAPH.STATS", json!({}));

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn graph_rebuild_command_returns_before_and_after_reports() {
        let state = memory_product_state();
        let mut store = state.tenant_graph_store("tenant-a").unwrap();

        let write = execute_graph_store_command(
            &mut store,
            "RUSTYRED.GRAPH.NODE.UPSERT",
            json!({
                "id": "node:a",
                "labels": ["File"],
                "properties": { "path": "src/lib.rs" }
            }),
        );
        let rebuild =
            execute_graph_store_command(&mut store, "RUSTYRED.GRAPH.REBUILD_INDEXES", json!({}));

        assert!(write.ok);
        assert!(rebuild.ok);
        assert_eq!(rebuild.status, "ok");
        assert_eq!(rebuild.payload["report"]["before"]["ok"], true);
        assert_eq!(rebuild.payload["report"]["after"]["ok"], true);
    }

    #[test]
    fn maps_graph_store_errors_to_http_statuses() {
        assert_eq!(
            graph_error_status("missing_graph_endpoint"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            graph_error_status("invalid_graph_cache_request"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            graph_error_status("redis_graph_store_error"),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            graph_error_status("tenant_memory_quota_exceeded"),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn cache_command_reports_stale_after_graph_write_advances_version() {
        let state = memory_product_state();

        let first_write = execute_tenant_command(
            &state,
            "tenant-a",
            "RUSTYRED.GRAPH.NODE.UPSERT",
            json!({
                "id": "node:a",
                "labels": ["File"],
                "properties": { "path": "src/lib.rs" }
            }),
        );
        assert_eq!(first_write.status(), StatusCode::OK);

        let cache_put = execute_tenant_command(
            &state,
            "tenant-a",
            "RUSTYRED.CACHE.PUT",
            json!({
                "kind": "query_result",
                "key": { "label": "File", "path": "src/lib.rs" },
                "value": { "nodes": ["node:a"] },
                "metadata": { "operation": "node_match" }
            }),
        );
        assert_eq!(cache_put.status(), StatusCode::OK);

        let second_write = execute_tenant_command(
            &state,
            "tenant-a",
            "RUSTYRED.GRAPH.NODE.UPSERT",
            json!({
                "id": "node:b",
                "labels": ["File"],
                "properties": { "path": "src/main.rs" }
            }),
        );
        assert_eq!(second_write.status(), StatusCode::OK);

        let cache_check = execute_tenant_cache_command(
            &state,
            "tenant-a",
            "RUSTYRED.CACHE.CHECK",
            json!({
                "kind": "query_result",
                "key": { "label": "File", "path": "src/lib.rs" }
            }),
        );
        assert!(cache_check.ok);
        assert_eq!(cache_check.status, "graph_version_mismatch");
        assert_eq!(cache_check.payload["cache"]["stale"], true);
        assert_eq!(cache_check.payload["cache"]["accepted"], false);
    }

    #[test]
    fn mcp_origin_check_allows_absent_or_configured_origin() {
        let allowed = vec!["https://app.example.com".to_string()];
        assert!(mcp_origin_allowed(&HeaderMap::new(), &allowed));

        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("https://app.example.com"),
        );
        assert!(mcp_origin_allowed(&headers, &allowed));

        headers.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(!mcp_origin_allowed(&headers, &allowed));
    }

    #[tokio::test]
    async fn graph_vector_hybrid_reports_effective_scoring_overrides() {
        let state = memory_product_state();
        let mut store = state.tenant_graph_store("tenant-hybrid").unwrap();
        store
            .designate_vector_property("Doc", "embedding", 2)
            .unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:a",
                ["Doc"],
                json!({ "embedding": [1.0, 0.0] }),
            ))
            .unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:b",
                ["Doc"],
                json!({ "embedding": [0.8, 0.2] }),
            ))
            .unwrap();
        store
            .upsert_edge(rustyred_core::EdgeRecord::new(
                "edge:ab",
                "node:a",
                "CONTRADICTS",
                "node:b",
                json!({}),
            ))
            .unwrap();

        let response = graph_vector_hybrid(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-hybrid".to_string()),
            HeaderMap::new(),
            Json(HybridSearchBody {
                query: vec![1.0, 0.0],
                k: 2,
                label: Some("Doc".to_string()),
                property: "embedding".to_string(),
                graph_seeds: vec!["node:a".to_string()],
                max_hops: 2,
                alpha: Some(0.2),
                confidence_weighted_graph_distance: Some(false),
                edge_type_weights: Some(std::collections::BTreeMap::from([(
                    "CONTRADICTS".to_string(),
                    -2.0,
                )])),
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["ok"], true);
        let alpha = payload["scoring"]["alpha"].as_f64().unwrap();
        assert!((alpha - 0.2).abs() < 1e-6);
        assert_eq!(
            payload["scoring"]["confidence_weighted_graph_distance"],
            false
        );
        assert_eq!(payload["scoring"]["edge_type_weights"]["CONTRADICTS"], -2.0);
        assert!(payload["results"].is_array());
    }

    #[tokio::test]
    async fn graph_version_compile_and_diff_routes_return_public_pack() {
        let state = memory_product_state();
        let mut store = state.tenant_graph_store("tenant-version").unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:ada",
                ["Person"],
                json!({ "name": "Ada" }),
            ))
            .unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:bob",
                ["Person"],
                json!({ "name": "Bob" }),
            ))
            .unwrap();
        store
            .upsert_edge(rustyred_core::EdgeRecord::new(
                "edge:knows",
                "node:ada",
                "KNOWS",
                "node:bob",
                json!({}),
            ))
            .unwrap();
        let base = store.graph_snapshot().unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:ada",
                ["Person"],
                json!({ "name": "Ada Lovelace" }),
            ))
            .unwrap();
        drop(store);

        let compile_response = graph_version_compile(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-version".to_string()),
            HeaderMap::new(),
            Json(rustyred_core::GraphCompileOptions {
                name: Some("public-redcore".to_string()),
                timestamp_unix_ms: Some(1),
                ..rustyred_core::GraphCompileOptions::default()
            }),
        )
        .await
        .into_response();
        assert_eq!(compile_response.status(), StatusCode::OK);
        let compile_payload = response_payload_json(compile_response).await;
        assert_eq!(compile_payload["ok"], true);
        assert_eq!(
            compile_payload["pack"]["protocol_version"],
            rustyred_core::VERSIONED_GRAPH_PROTOCOL_VERSION
        );
        assert_eq!(compile_payload["pack"]["manifest"]["objects_total"], 3);
        assert_eq!(
            compile_payload["pack"]["capabilities"][0]["kind"],
            "graph_manifest"
        );

        let diff_response = graph_version_diff(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-version".to_string()),
            HeaderMap::new(),
            Json(GraphVersionDiffBody {
                base: base.clone(),
                target: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(diff_response.status(), StatusCode::OK);
        let diff_payload = response_payload_json(diff_response).await;
        assert_eq!(
            diff_payload["diff"]["modified"].as_array().unwrap().len(),
            1
        );
        assert_eq!(diff_payload["diff"]["unchanged"], 2);

        let ref_response = graph_version_ref(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-version".to_string()),
            HeaderMap::new(),
            Json(GraphVersionRefBody {
                repository: None,
                updated_at_unix_ms: Some(2),
                options: rustyred_core::GraphCompileOptions {
                    branch: Some("main".to_string()),
                    timestamp_unix_ms: Some(1),
                    ..rustyred_core::GraphCompileOptions::default()
                },
            }),
        )
        .await
        .into_response();
        assert_eq!(ref_response.status(), StatusCode::OK);
        let ref_payload = response_payload_json(ref_response).await;
        assert_eq!(ref_payload["ref_update"]["reference"]["name"], "main");
        let repository: rustyred_core::GraphVersionRepository =
            serde_json::from_value(ref_payload["ref_update"]["repository"].clone()).unwrap();

        let log_response = graph_version_log_route(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-version".to_string()),
            HeaderMap::new(),
            Json(GraphVersionLogBody {
                repository: repository.clone(),
                target: Some("main".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(log_response.status(), StatusCode::OK);
        let log_payload = response_payload_json(log_response).await;
        assert_eq!(log_payload["log"]["commits"].as_array().unwrap().len(), 1);

        let checkout_response = graph_version_checkout(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-version".to_string()),
            HeaderMap::new(),
            Json(GraphVersionCheckoutBody {
                repository,
                target: "main".to_string(),
            }),
        )
        .await
        .into_response();
        assert_eq!(checkout_response.status(), StatusCode::OK);
        let checkout_payload = response_payload_json(checkout_response).await;
        assert_eq!(
            checkout_payload["checkout"]["snapshot"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let mut theirs = base.clone();
        theirs.nodes.push(rustyred_core::NodeRecord::new(
            "node:theirs",
            ["Person"],
            json!({ "name": "Theirs" }),
        ));
        let merge_response = graph_version_merge(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-version".to_string()),
            HeaderMap::new(),
            Json(GraphVersionMergeBody {
                base,
                ours: None,
                theirs,
                options: rustyred_core::GraphMergeOptions {
                    timestamp_unix_ms: Some(3),
                    ..rustyred_core::GraphMergeOptions::default()
                },
            }),
        )
        .await
        .into_response();
        assert_eq!(merge_response.status(), StatusCode::OK);
        let merge_payload = response_payload_json(merge_response).await;
        assert_eq!(merge_payload["merge"]["status"], "clean");
    }

    #[tokio::test]
    async fn instant_kg_routes_overlay_session_delta_without_mutating_base() {
        let state = memory_product_state();
        let mut store = state.tenant_graph_store("tenant-kg").unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "file:lib",
                ["File"],
                json!({ "path": "src/lib.rs", "content": "encoder pipeline" }),
            ))
            .unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "sym:old",
                ["Symbol"],
                json!({ "name": "old_symbol" }),
            ))
            .unwrap();
        store
            .upsert_edge(rustyred_core::EdgeRecord::new(
                "edge:old",
                "file:lib",
                "contains",
                "sym:old",
                json!({ "path": "src/lib.rs", "line": 7 }),
            ))
            .unwrap();
        drop(store);

        let delta = rustyred_core::SessionDelta {
            tombstoned_object_ids: vec!["sym:old".to_string()],
            objects: vec![rustyred_core::NodeRecord::new(
                "sym:new",
                ["Symbol"],
                json!({ "name": "new_symbol", "content": "instant kg carry" }),
            )],
            edges: vec![rustyred_core::EdgeRecord::new(
                "edge:new",
                "file:lib",
                "contains",
                "sym:new",
                json!({ "path": "src/lib.rs", "line": 9 }),
            )],
            ..rustyred_core::SessionDelta::default()
        };

        let status_response = instant_kg_status(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-kg".to_string()),
            HeaderMap::new(),
            Json(InstantKgViewBody {
                manifest: None,
                delta: Some(delta.clone()),
            }),
        )
        .await
        .into_response();
        assert_eq!(status_response.status(), StatusCode::OK);
        let status = response_payload_json(status_response).await;
        assert_eq!(status["status"]["total_objects"], 2);
        assert_eq!(status["status"]["tombstoned_objects"], 1);

        let ppr_response = instant_kg_ppr(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-kg".to_string()),
            HeaderMap::new(),
            Json(InstantKgPprBody {
                view: InstantKgViewBody {
                    manifest: None,
                    delta: Some(delta.clone()),
                },
                seeds: std::collections::HashMap::from([("file:lib".to_string(), 1.0)]),
                alpha: default_ppr_alpha(),
                epsilon: default_ppr_epsilon(),
                max_pushes: default_ppr_max_pushes(),
                top_k: 5,
            }),
        )
        .await
        .into_response();
        assert_eq!(ppr_response.status(), StatusCode::OK);
        let ppr = response_payload_json(ppr_response).await;
        let result_ids: Vec<_> = ppr["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["object_id"].as_str())
            .collect();
        assert!(result_ids.contains(&"sym:new"));
        assert!(!result_ids.contains(&"sym:old"));

        let search_response = instant_kg_search(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-kg".to_string()),
            HeaderMap::new(),
            Json(InstantKgSearchBody {
                view: InstantKgViewBody {
                    manifest: None,
                    delta: Some(delta.clone()),
                },
                query: "instant".to_string(),
                kinds: vec!["Symbol".to_string()],
                top_k: 5,
            }),
        )
        .await
        .into_response();
        assert_eq!(search_response.status(), StatusCode::OK);
        let search = response_payload_json(search_response).await;
        assert_eq!(search["results"][0]["object_id"], "sym:new");

        let impact_response = instant_kg_impact(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-kg".to_string()),
            HeaderMap::new(),
            Json(InstantKgImpactBody {
                view: InstantKgViewBody {
                    manifest: None,
                    delta: Some(delta.clone()),
                },
                seed: Some("file:lib".to_string()),
                symbol_name: None,
                direction: "out".to_string(),
                max_depth: 1,
            }),
        )
        .await
        .into_response();
        assert_eq!(impact_response.status(), StatusCode::OK);
        let impact = response_payload_json(impact_response).await;
        assert_eq!(impact["results"][0]["object_id"], "sym:new");

        let symbol_impact_response = instant_kg_impact(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-kg".to_string()),
            HeaderMap::new(),
            Json(InstantKgImpactBody {
                view: InstantKgViewBody {
                    manifest: None,
                    delta: Some(delta.clone()),
                },
                seed: None,
                symbol_name: Some("new_symbol".to_string()),
                direction: "in".to_string(),
                max_depth: 1,
            }),
        )
        .await
        .into_response();
        assert_eq!(symbol_impact_response.status(), StatusCode::OK);
        let symbol_impact = response_payload_json(symbol_impact_response).await;
        assert_eq!(symbol_impact["seed"], "sym:new");
        assert_eq!(symbol_impact["results"][0]["object_id"], "file:lib");

        let explain_response = instant_kg_explain_edge(
            axum::extract::State(state),
            axum::extract::Path("tenant-kg".to_string()),
            HeaderMap::new(),
            Json(InstantKgExplainEdgeBody {
                view: InstantKgViewBody {
                    manifest: None,
                    delta: Some(delta),
                },
                src: "file:lib".to_string(),
                dst: "sym:new".to_string(),
            }),
        )
        .await
        .into_response();
        assert_eq!(explain_response.status(), StatusCode::OK);
        let explain = response_payload_json(explain_response).await;
        assert_eq!(explain["explanations"][0]["layer"], "delta");
    }

    #[tokio::test]
    async fn route_metrics_include_vector_fulltext_and_algorithm_histograms() {
        let state = memory_product_state();
        let tenant_id = "tenant-metrics".to_string();
        let mut store = state.tenant_graph_store(&tenant_id).unwrap();
        store
            .designate_vector_property("Doc", "embedding", 2)
            .unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:a",
                ["Doc"],
                json!({ "embedding": [1.0, 0.0], "text": "alpha document" }),
            ))
            .unwrap();
        store
            .upsert_node(rustyred_core::NodeRecord::new(
                "node:b",
                ["Doc"],
                json!({ "embedding": [0.8, 0.2], "text": "beta document" }),
            ))
            .unwrap();
        store
            .upsert_edge(rustyred_core::EdgeRecord::new(
                "edge:ab",
                "node:a",
                "REL",
                "node:b",
                json!({}),
            ))
            .unwrap();
        drop(store);
        state
            .designate_fulltext_property(&tenant_id, "Doc", "text")
            .unwrap();

        let vector_response = graph_vector_search(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id.clone()),
            HeaderMap::new(),
            Json(VectorSearchBody {
                query: vec![1.0, 0.0],
                k: 2,
                label: Some("Doc".to_string()),
                property: "embedding".to_string(),
            }),
        )
        .await
        .into_response();
        assert_eq!(vector_response.status(), StatusCode::OK);

        let hybrid_response = graph_vector_hybrid(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id.clone()),
            HeaderMap::new(),
            Json(HybridSearchBody {
                query: vec![1.0, 0.0],
                k: 2,
                label: Some("Doc".to_string()),
                property: "embedding".to_string(),
                graph_seeds: vec!["node:a".to_string()],
                max_hops: 2,
                alpha: None,
                confidence_weighted_graph_distance: None,
                edge_type_weights: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(hybrid_response.status(), StatusCode::OK);

        let fulltext_response = graph_fulltext_search(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id.clone()),
            HeaderMap::new(),
            Json(FullTextSearchBody {
                label: Some("Doc".to_string()),
                property: "text".to_string(),
                query: "alpha".to_string(),
                k: 2,
            }),
        )
        .await
        .into_response();
        assert_eq!(fulltext_response.status(), StatusCode::OK);

        let ppr_response = graph_algorithm_ppr(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id.clone()),
            HeaderMap::new(),
            Json(PprBody {
                seeds: std::collections::HashMap::from([("node:a".to_string(), 1.0)]),
                alpha: default_ppr_alpha(),
                epsilon: default_ppr_epsilon(),
                max_pushes: default_ppr_max_pushes(),
                top_k: Some(2),
            }),
        )
        .await
        .into_response();
        assert_eq!(ppr_response.status(), StatusCode::OK);

        let components_response = graph_algorithm_components(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id.clone()),
            HeaderMap::new(),
            Json(ComponentsBody { directed: false }),
        )
        .await
        .into_response();
        assert_eq!(components_response.status(), StatusCode::OK);

        let pagerank_response = graph_algorithm_pagerank(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id.clone()),
            HeaderMap::new(),
            Json(PageRankBody {
                damping: default_pr_damping(),
                max_iter: default_pr_max_iter(),
                tolerance: default_pr_tolerance(),
                top_k: Some(2),
            }),
        )
        .await
        .into_response();
        assert_eq!(pagerank_response.status(), StatusCode::OK);

        let communities_response = graph_algorithm_communities(
            axum::extract::State(state.clone()),
            axum::extract::Path(tenant_id),
            HeaderMap::new(),
            Json(CommunitiesBody::default()),
        )
        .await
        .into_response();
        assert_eq!(communities_response.status(), StatusCode::OK);

        let metrics = state.observability.render_prometheus();
        assert!(metrics.contains("rustyred_vector_search_latency_seconds_count 2"));
        assert!(metrics.contains("rustyred_fulltext_search_latency_seconds_count 1"));
        assert!(metrics.contains("rustyred_algorithm_latency_seconds_ppr_count 1"));
        assert!(metrics.contains("rustyred_algorithm_latency_seconds_components_count 1"));
        assert!(metrics.contains("rustyred_algorithm_latency_seconds_pagerank_count 1"));
        assert!(metrics.contains("rustyred_algorithm_latency_seconds_communities_count 1"));
    }

    #[tokio::test]
    async fn diagnostics_config_reports_startup_only_tenant_overrides() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::None,
            snapshot_interval_writes: 0,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: BTreeMap::from([(
                "tenant-a".to_string(),
                TenantConfigOverride {
                    durability: Some(RedCoreDurability::AofAlways),
                    snapshot_interval_writes: Some(42),
                    strict_acid: Some(true),
                    tenant_memory_quota_bytes: Some(4_096),
                    hybrid_scoring: Some(rustyred_core::HybridScoringConfig {
                        alpha: 0.25,
                        confidence_weighted_graph_distance: false,
                        edge_type_weights: BTreeMap::from([("CONTRADICTS".to_string(), -2.0)]),
                    }),
                },
            )]),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: rustyred_core::HybridScoringConfig::default(),
            redis_url: "not-a-redis-url".to_string(),
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
        });

        let response = diagnostics_config(axum::extract::State(state), HeaderMap::new())
            .await
            .unwrap()
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["tenant_config_overrides"], 1);
        assert_eq!(payload["tenant_config_runtime_mutation_supported"], false);
        assert_eq!(payload["tenant_config_tenants"], json!(["tenant-a"]));
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["durability"],
            "aof_always"
        );
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["snapshot_interval_writes"],
            42
        );
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["strict_acid"],
            true
        );
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["tenant_memory_quota_bytes"],
            4096
        );
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["hybrid_scoring"]["alpha"],
            0.25
        );
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["hybrid_scoring"]
                ["confidence_weighted_graph_distance"],
            false
        );
        assert_eq!(
            payload["tenant_config_overrides_detail"]["tenant-a"]["hybrid_scoring"]
                ["edge_type_weights"]["CONTRADICTS"],
            -2.0
        );
    }

    #[tokio::test]
    async fn transaction_routes_support_begin_stage_and_commit() {
        let state = memory_product_state();
        let begin_response = transaction_begin(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionBeginBody {
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(begin_response.status(), StatusCode::OK);

        let begin_payload = response_payload_json(begin_response).await;
        let tx_id = begin_payload["tx_id"]
            .as_str()
            .expect("transaction id in begin response");

        let stage_response = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-tx".to_string()),
                query: "CREATE (n:File {id: $id, path: $path})".to_string(),
                params: BTreeMap::from([
                    ("id".to_string(), json!("node:tx-commit")),
                    ("path".to_string(), json!("src/main.rs")),
                ]),
                tx_id: Some(tx_id.to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(stage_response.status(), StatusCode::OK);

        let stage_payload = response_payload_json(stage_response).await;
        assert_eq!(stage_payload["ok"], true);
        assert_eq!(stage_payload["staged_mutations"], 1);
        assert_eq!(stage_payload["tx_id"], tx_id);

        let commit_response = transaction_commit(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionMutationBody {
                tx_id: tx_id.to_string(),
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(commit_response.status(), StatusCode::OK);

        let commit_payload = response_payload_json(commit_response).await;
        assert_eq!(commit_payload["ok"], true);
        assert_eq!(commit_payload["tenant"], "tenant-tx");
        assert!(commit_payload["transaction"]["writes"].as_array().is_some());

        let store = state.tenant_graph_store("tenant-tx").unwrap();
        let node = store.get_node("node:tx-commit").unwrap().unwrap();
        assert_eq!(node.id, "node:tx-commit");
    }

    #[tokio::test]
    async fn transaction_routes_support_rollback() {
        let state = memory_product_state();
        let begin_response = transaction_begin(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionBeginBody {
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(begin_response.status(), StatusCode::OK);
        let begin_payload = response_payload_json(begin_response).await;
        let tx_id = begin_payload["tx_id"].as_str().unwrap();

        let stage_response = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-tx".to_string()),
                query: "CREATE (n:File {id: $id, path: $path})".to_string(),
                params: BTreeMap::from([
                    ("id".to_string(), json!("node:tx-rollback")),
                    ("path".to_string(), json!("src/rollback.rs")),
                ]),
                tx_id: Some(tx_id.to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(stage_response.status(), StatusCode::OK);

        let rollback_response = transaction_rollback(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionMutationBody {
                tx_id: tx_id.to_string(),
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(rollback_response.status(), StatusCode::OK);
        let rollback_payload = response_payload_json(rollback_response).await;
        assert_eq!(rollback_payload["status"], "rolled_back");
        assert_eq!(rollback_payload["tx_id"], tx_id);

        let store = state.tenant_graph_store("tenant-tx").unwrap();
        assert!(store.get_node("node:tx-rollback").unwrap().is_none());
    }

    #[tokio::test]
    async fn transaction_commit_rejects_missing_tx_id() {
        let state = memory_product_state();
        let response = transaction_commit(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(TransactionMutationBody {
                tx_id: String::new(),
                tenant_id: Some("tenant-tx".to_string()),
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["error"], "missing_tx_id");
    }

    // ===== Phase 3-B: streaming bulk loader tests =====

    #[tokio::test]
    async fn bulk_nodes_jsonl_streaming_inserts_two_nodes() {
        let state = memory_product_state();
        let body = Body::from(
            "{\"id\":\"n1\",\"labels\":[\"Doc\"],\"properties\":{}}\n\
             {\"id\":\"n2\",\"labels\":[\"Doc\"],\"properties\":{}}\n"
                .to_string(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/jsonl"),
        );
        let response = graph_bulk_nodes(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-bulk".to_string()),
            headers,
            axum::extract::Query(BulkQuery::default()),
            body,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["inserted"], 2);
        assert_eq!(payload["failed"], 0);
    }

    #[tokio::test]
    async fn bulk_nodes_csv_streaming_uses_first_row_headers() {
        let state = memory_product_state();
        let body = Body::from("id,label,path\nnA,Doc,src/a.rs\nnB,Doc,src/b.rs\n".to_string());
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_static("text/csv"));
        let response = graph_bulk_nodes(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-bulk".to_string()),
            headers,
            axum::extract::Query(BulkQuery::default()),
            body,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_payload_json(response).await;
        assert_eq!(payload["inserted"], 2);
    }

    #[tokio::test]
    async fn bulk_nodes_respects_explicit_batch_size_one_per_batch() {
        let state = memory_product_state();
        let body = Body::from(
            "{\"id\":\"n1\",\"labels\":[\"Doc\"],\"properties\":{}}\n\
             {\"id\":\"n2\",\"labels\":[\"Doc\"],\"properties\":{}}\n"
                .to_string(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/jsonl"),
        );
        let response = graph_bulk_nodes(
            axum::extract::State(state),
            axum::extract::Path("tenant-bulk".to_string()),
            headers,
            axum::extract::Query(BulkQuery {
                batch_size: Some(1),
                headers: None,
                from_col: None,
                to_col: None,
            }),
            body,
        )
        .await
        .into_response();
        let payload = response_payload_json(response).await;
        assert_eq!(payload["inserted"], 2);
        assert_eq!(payload["batches"], 2);
    }

    #[tokio::test]
    async fn bulk_nodes_reports_per_line_parse_errors_and_keeps_good_rows() {
        let state = memory_product_state();
        let body = Body::from(
            "{\"id\":\"bad\",\"labels\":[\"Doc\"],\"properties\":[]}\n\
             {\"id\":\"good\",\"labels\":[\"Doc\"],\"properties\":{}}\n"
                .to_string(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/jsonl"),
        );

        let response = graph_bulk_nodes(
            axum::extract::State(state),
            axum::extract::Path("tenant-bulk-errors".to_string()),
            headers,
            axum::extract::Query(BulkQuery::default()),
            body,
        )
        .await
        .into_response();
        let payload = response_payload_json(response).await;

        assert_eq!(payload["inserted"], 1);
        assert_eq!(payload["failed"], 1);
        assert_eq!(payload["errors"][0]["line"], 1);
        assert_eq!(payload["errors"][0]["code"], "invalid_properties");
    }

    #[tokio::test]
    async fn bulk_edges_jsonl_streaming_inserts_one_edge() {
        let state = memory_product_state();
        let nodes_body = Body::from(
            "{\"id\":\"a\",\"labels\":[\"Doc\"],\"properties\":{}}\n\
             {\"id\":\"b\",\"labels\":[\"Doc\"],\"properties\":{}}\n"
                .to_string(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/jsonl"),
        );
        let _ = graph_bulk_nodes(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-edges".to_string()),
            headers.clone(),
            axum::extract::Query(BulkQuery::default()),
            nodes_body,
        )
        .await;

        let edges_body = Body::from(
            "{\"id\":\"e1\",\"from_id\":\"a\",\"to_id\":\"b\",\"type\":\"CITES\",\"properties\":{}}\n"
                .to_string(),
        );
        let response = graph_bulk_edges(
            axum::extract::State(state),
            axum::extract::Path("tenant-edges".to_string()),
            headers,
            axum::extract::Query(BulkQuery::default()),
            edges_body,
        )
        .await
        .into_response();
        let payload = response_payload_json(response).await;
        assert_eq!(payload["inserted"], 1);
    }

    #[tokio::test]
    async fn bulk_edges_retries_batch_to_report_missing_endpoint_line() {
        let state = memory_product_state();
        let nodes_body = Body::from(
            "{\"id\":\"a\",\"labels\":[\"Doc\"],\"properties\":{}}\n\
             {\"id\":\"b\",\"labels\":[\"Doc\"],\"properties\":{}}\n"
                .to_string(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/jsonl"),
        );
        let _ = graph_bulk_nodes(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-edge-errors".to_string()),
            headers.clone(),
            axum::extract::Query(BulkQuery::default()),
            nodes_body,
        )
        .await;

        let edges_body = Body::from(
            "{\"id\":\"e1\",\"from_id\":\"a\",\"to_id\":\"b\",\"type\":\"CITES\",\"properties\":{}}\n\
             {\"id\":\"e2\",\"from_id\":\"a\",\"to_id\":\"missing\",\"type\":\"CITES\",\"properties\":{}}\n"
                .to_string(),
        );
        let response = graph_bulk_edges(
            axum::extract::State(state),
            axum::extract::Path("tenant-edge-errors".to_string()),
            headers,
            axum::extract::Query(BulkQuery::default()),
            edges_body,
        )
        .await
        .into_response();
        let payload = response_payload_json(response).await;

        assert_eq!(payload["inserted"], 1);
        assert_eq!(payload["failed"], 1);
        assert_eq!(payload["errors"][0]["line"], 2);
        assert_eq!(payload["errors"][0]["code"], "missing_graph_endpoint");
        assert_eq!(payload["errors"][0]["record_id"], "e2");
    }

    // ===== Phase 3-A: auto-tx write Cypher tests =====

    #[tokio::test]
    async fn public_cypher_create_auto_opens_and_commits_transaction() {
        let state = memory_product_state();
        let response = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-w".to_string()),
                query: "CREATE (n:Doc {id: 'a', path: 'src/lib.rs'})".to_string(),
                params: BTreeMap::new(),
                tx_id: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let store = state.tenant_graph_store("tenant-w").unwrap();
        let node = store.get_node("a").unwrap().unwrap();
        assert_eq!(node.id, "a");
        assert!(node.labels.contains(&"Doc".to_string()));
    }

    #[tokio::test]
    async fn public_cypher_merge_is_idempotent_with_on_create_then_on_match() {
        let state = memory_product_state();
        let first = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-merge".to_string()),
                query:
                    "MERGE (n:Doc {id: 'a'}) ON CREATE SET n.seen = 1 ON MATCH SET n.seen = n.seen + 1"
                        .to_string(),
                params: BTreeMap::new(),
                tx_id: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(first.status(), StatusCode::OK);

        let second = public_cypher(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            Json(PublicCypherBody {
                tenant_id: Some("tenant-merge".to_string()),
                query:
                    "MERGE (n:Doc {id: 'a'}) ON CREATE SET n.seen = 1 ON MATCH SET n.seen = n.seen + 1"
                        .to_string(),
                params: BTreeMap::new(),
                tx_id: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(second.status(), StatusCode::OK);

        let store = state.tenant_graph_store("tenant-merge").unwrap();
        let node = store.get_node("a").unwrap().unwrap();
        assert_eq!(node.properties["seen"].as_i64(), Some(2));
    }

    #[tokio::test]
    async fn bulk_edges_csv_requires_from_to_columns() {
        let state = memory_product_state();
        // seed source/target nodes first
        let nodes_body = Body::from(
            "{\"id\":\"a\",\"labels\":[\"Doc\"],\"properties\":{}}\n\
             {\"id\":\"b\",\"labels\":[\"Doc\"],\"properties\":{}}\n"
                .to_string(),
        );
        let mut nh = HeaderMap::new();
        nh.insert(
            "Content-Type",
            HeaderValue::from_static("application/jsonl"),
        );
        let _ = graph_bulk_nodes(
            axum::extract::State(state.clone()),
            axum::extract::Path("tenant-edges".to_string()),
            nh,
            axum::extract::Query(BulkQuery::default()),
            nodes_body,
        )
        .await;

        let body = Body::from("id,src,dst,type\ne1,a,b,CITES\n".to_string());
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_static("text/csv"));
        let response = graph_bulk_edges(
            axum::extract::State(state),
            axum::extract::Path("tenant-edges".to_string()),
            headers,
            axum::extract::Query(BulkQuery {
                batch_size: None,
                headers: None,
                from_col: Some("src".into()),
                to_col: Some("dst".into()),
            }),
            body,
        )
        .await
        .into_response();
        let payload = response_payload_json(response).await;
        assert_eq!(payload["inserted"], 1);
    }
}
