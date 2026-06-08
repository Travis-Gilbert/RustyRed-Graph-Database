use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use rustyred_core::store::RedisRustyredStore;
use rustyred_core::{
    make_fulltext_backend, make_spatial_backend, sanitize_tenant_segment, EdgeRecord,
    EpistemicType, FullTextBackend, FullTextDesignation, GraphMutation, GraphMutationBatch,
    GraphRebuildReport, GraphSnapshot, GraphStats, GraphStoreError, GraphStoreResult,
    GraphTransaction, GraphWriteResult, HybridScoringConfig, InMemoryGraphStore, NeighborHit,
    NeighborQuery, NodeQuery, NodeRecord, RedCoreGraphStore, RedCoreOptions, RedisGraphStore,
    SpatialBackend, SpatialDesignation, VectorDesignation, VerifyReport,
};
#[cfg(feature = "geometry")]
use rustyred_core::{GeometryDesignation, GeometryEncoding, GeometryIndex};
use rustyred_mcp::{McpError, McpGraphBackend, McpGraphProvider, McpServerConfig};
use serde_json::json;
#[cfg(feature = "geometry")]
use serde_json::Value;

use crate::config::{Config, StorageMode};
use crate::graph_cache::GraphCacheTenant;
use crate::observability::Observability;

const GRAPH_TRANSACTION_TTL_MS: u64 = 5 * 60 * 1000;

#[derive(Clone, Debug)]
struct GraphTransactionContext {
    tenant_id: String,
    snapshot_version: u64,
    created_at_ms: u64,
    mutations: GraphMutationBatch,
}

/// Per-tenant Phase 8 spatial indexes. Keyed by tenant_id then by
/// (label, lat_property, lon_property).
type SpatialIndexes = BTreeMap<String, BTreeMap<(String, String, String), Box<dyn SpatialBackend>>>;

/// Per-tenant Phase 5 full-text indexes. Keyed by tenant_id then by
/// (label, property).
type FullTextIndexes = BTreeMap<String, BTreeMap<(String, String), Box<dyn FullTextBackend>>>;

#[cfg(feature = "geometry")]
type GeometryIndexes = BTreeMap<String, BTreeMap<(String, String), GeometryIndex>>;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub observability: Observability,
    redcore_stores: Arc<Mutex<BTreeMap<String, Arc<RedCoreTenantExecutor>>>>,
    graph_caches: Arc<Mutex<BTreeMap<String, Arc<GraphCacheTenant>>>>,
    graph_transactions: Arc<Mutex<BTreeMap<String, GraphTransactionContext>>>,
    next_graph_txn_id: Arc<AtomicU64>,
    spatial_indexes: Arc<Mutex<SpatialIndexes>>,
    fulltext_indexes: Arc<Mutex<FullTextIndexes>>,
    #[cfg(feature = "geometry")]
    geometry_indexes: Arc<Mutex<GeometryIndexes>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let observability = Observability::new_with_log(
            config.slow_query_threshold_nanos,
            config.slow_query_capacity,
            config.slow_query_log.clone(),
        );
        Self {
            config: Arc::new(config),
            observability,
            redcore_stores: Arc::new(Mutex::new(BTreeMap::new())),
            graph_caches: Arc::new(Mutex::new(BTreeMap::new())),
            graph_transactions: Arc::new(Mutex::new(BTreeMap::new())),
            next_graph_txn_id: Arc::new(AtomicU64::new(1)),
            spatial_indexes: Arc::new(Mutex::new(BTreeMap::new())),
            fulltext_indexes: Arc::new(Mutex::new(BTreeMap::new())),
            #[cfg(feature = "geometry")]
            geometry_indexes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    // ===== Phase 5: full-text designation + indexing =====

    pub fn designate_fulltext_property(
        &self,
        tenant_id: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StoreAccessError> {
        let store = self.tenant_graph_store(tenant_id)?;
        let designation = FullTextDesignation {
            label: label.to_string(),
            property: property.to_string(),
        };
        // §P5-A pa5.3: env switch resolves at backend construction time so
        // each `/fulltext/designate` call honors the current
        // RUSTY_RED_FULLTEXT_BACKEND setting. Default is the hand-rolled BM25;
        // `tantivy` selects the tantivy-backed impl when the feature is built.
        let mut index = make_fulltext_backend(designation)
            .map_err(|err| StoreAccessError::unsupported(err.message()))?;
        // Bulk-index any existing nodes for the label.
        let nodes = store
            .query_nodes(NodeQuery {
                label: Some(label.to_string()),
                ..NodeQuery::default()
            })
            .map_err(StoreAccessError::from)?;
        for node in nodes {
            if let Some(text) = node.properties.get(property).and_then(|v| v.as_str()) {
                index.upsert(&node.id, text);
            }
        }
        let mut indexes = self
            .fulltext_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("fulltext index lock poisoned"))?;
        indexes
            .entry(tenant_id.to_string())
            .or_default()
            .insert((label.to_string(), property.to_string()), index);
        Ok(())
    }

    pub fn maybe_index_node_fulltext(&self, tenant_id: &str, node: &NodeRecord) {
        let Ok(mut indexes) = self.fulltext_indexes.lock() else {
            return;
        };
        let Some(tenant_map) = indexes.get_mut(tenant_id) else {
            return;
        };
        for ((label, property), index) in tenant_map.iter_mut() {
            if !node.labels.iter().any(|l| l == label) {
                continue;
            }
            if let Some(text) = node.properties.get(property).and_then(|v| v.as_str()) {
                index.upsert(&node.id, text);
            } else {
                index.remove(&node.id);
            }
        }
    }

    pub fn fulltext_search(
        &self,
        tenant_id: &str,
        label: Option<&str>,
        property: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<(String, f32)>, StoreAccessError> {
        let indexes = self
            .fulltext_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("fulltext index lock poisoned"))?;
        let Some(tenant_map) = indexes.get(tenant_id) else {
            return Err(StoreAccessError::unsupported(
                "no fulltext designations for this tenant",
            ));
        };
        // If label given, search just that (label, property). Otherwise union
        // over every label that has this property indexed.
        let mut combined: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        for ((idx_label, idx_property), index) in tenant_map.iter() {
            if idx_property != property {
                continue;
            }
            if let Some(label_filter) = label {
                if idx_label != label_filter {
                    continue;
                }
            }
            for (id, score) in index.search(query, k) {
                let slot = combined.entry(id).or_insert(0.0);
                if score > *slot {
                    *slot = score;
                }
            }
        }
        if combined.is_empty() {
            return Err(StoreAccessError::unsupported(
                "no matching fulltext designation; call /fulltext/designate first",
            ));
        }
        let mut entries: Vec<(String, f32)> = combined.into_iter().collect();
        entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        entries.truncate(k);
        Ok(entries)
    }

    // ===== Phase 8: spatial designation + indexing =====

    pub fn designate_spatial_property(
        &self,
        tenant_id: &str,
        label: &str,
        lat_property: &str,
        lon_property: &str,
        resolution: u8,
    ) -> Result<(), StoreAccessError> {
        if !(0..=15).contains(&resolution) {
            return Err(StoreAccessError::unsupported(format!(
                "spatial resolution {resolution} is outside 0..=15"
            )));
        }
        let store = self.tenant_graph_store(tenant_id)?;
        let designation = SpatialDesignation {
            label: label.to_string(),
            lat_property: lat_property.to_string(),
            lon_property: lon_property.to_string(),
            resolution,
        };
        // §P8-A pa8.3: env switch resolves at backend construction time so
        // each `/spatial/designate` call honors the current
        // RUSTY_RED_SPATIAL_BACKEND setting. Default is the H3 impl; `s2`
        // selects the S2-cell impl when the feature is built.
        let mut index: Box<dyn SpatialBackend> = make_spatial_backend(designation.clone())
            .map_err(|err| StoreAccessError::unsupported(err.message()))?;
        // Bulk-index any existing nodes for the label.
        let nodes = store
            .query_nodes(NodeQuery {
                label: Some(label.to_string()),
                ..NodeQuery::default()
            })
            .map_err(StoreAccessError::from)?;
        for node in nodes {
            if let (Some(lat), Some(lon)) = (
                node.properties.get(lat_property).and_then(|v| v.as_f64()),
                node.properties.get(lon_property).and_then(|v| v.as_f64()),
            ) {
                let _ = SpatialBackend::upsert(index.as_mut(), &node.id, lat, lon);
            }
        }
        let mut indexes = self
            .spatial_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("spatial index lock poisoned"))?;
        indexes.entry(tenant_id.to_string()).or_default().insert(
            (
                label.to_string(),
                lat_property.to_string(),
                lon_property.to_string(),
            ),
            index,
        );
        Ok(())
    }

    /// Index a node into any designations for its label whose lat+lon
    /// properties are present. Called on the write path.
    pub fn maybe_index_node_spatially(&self, tenant_id: &str, node: &NodeRecord) {
        let Ok(mut indexes) = self.spatial_indexes.lock() else {
            return;
        };
        let Some(tenant_map) = indexes.get_mut(tenant_id) else {
            return;
        };
        for ((label, lat_prop, lon_prop), index) in tenant_map.iter_mut() {
            if !node.labels.iter().any(|l| l == label) {
                continue;
            }
            let lat = node.properties.get(lat_prop).and_then(|v| v.as_f64());
            let lon = node.properties.get(lon_prop).and_then(|v| v.as_f64());
            if let (Some(lat), Some(lon)) = (lat, lon) {
                let _ = SpatialBackend::upsert(index.as_mut(), &node.id, lat, lon);
            }
        }
    }

    pub fn spatial_radius_search(
        &self,
        tenant_id: &str,
        label: &str,
        lat_property: &str,
        lon_property: &str,
        lat: f64,
        lon: f64,
        radius_km: f64,
    ) -> Result<Vec<String>, StoreAccessError> {
        let indexes = self
            .spatial_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("spatial index lock poisoned"))?;
        let key = (
            label.to_string(),
            lat_property.to_string(),
            lon_property.to_string(),
        );
        let Some(tenant_map) = indexes.get(tenant_id) else {
            return Err(StoreAccessError::unsupported(
                "no spatial designations for this tenant",
            ));
        };
        let Some(index) = tenant_map.get(&key) else {
            return Err(StoreAccessError::unsupported(
                "spatial designation not found; call /spatial/designate first",
            ));
        };
        index
            .radius_search(lat, lon, radius_km)
            .map_err(|e| StoreAccessError::unsupported(e.message()))
    }

    pub fn spatial_bbox_search(
        &self,
        tenant_id: &str,
        label: &str,
        lat_property: &str,
        lon_property: &str,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    ) -> Result<Vec<String>, StoreAccessError> {
        let indexes = self
            .spatial_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("spatial index lock poisoned"))?;
        let key = (
            label.to_string(),
            lat_property.to_string(),
            lon_property.to_string(),
        );
        let Some(tenant_map) = indexes.get(tenant_id) else {
            return Err(StoreAccessError::unsupported(
                "no spatial designations for this tenant",
            ));
        };
        let Some(index) = tenant_map.get(&key) else {
            return Err(StoreAccessError::unsupported(
                "spatial designation not found; call /spatial/designate first",
            ));
        };
        Ok(index.bbox_search(min_lat, min_lon, max_lat, max_lon))
    }

    #[cfg(feature = "geometry")]
    pub fn designate_geometry_property(
        &self,
        tenant_id: &str,
        label: &str,
        property: &str,
        encoding: GeometryEncoding,
        resolution: u8,
    ) -> Result<(), StoreAccessError> {
        if !(0..=15).contains(&resolution) {
            return Err(StoreAccessError::unsupported(format!(
                "geometry resolution {resolution} is outside 0..=15"
            )));
        }
        let store = self.tenant_graph_store(tenant_id)?;
        let designation = GeometryDesignation::property(
            label.to_string(),
            property.to_string(),
            encoding,
            resolution,
        );
        let mut index = GeometryIndex::new(designation)
            .map_err(|err| StoreAccessError::unsupported(err.message()))?;
        let nodes = store
            .query_nodes(NodeQuery {
                label: Some(label.to_string()),
                ..NodeQuery::default()
            })
            .map_err(StoreAccessError::from)?;
        for node in nodes {
            let _ = index.upsert_from_properties(&node.id, &node.properties);
        }
        let mut indexes = self
            .geometry_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("geometry index lock poisoned"))?;
        indexes
            .entry(tenant_id.to_string())
            .or_default()
            .insert((label.to_string(), property.to_string()), index);
        Ok(())
    }

    #[cfg(feature = "geometry")]
    pub fn maybe_index_node_geometry(&self, tenant_id: &str, node: &NodeRecord) {
        let Ok(mut indexes) = self.geometry_indexes.lock() else {
            return;
        };
        let Some(tenant_map) = indexes.get_mut(tenant_id) else {
            return;
        };
        for ((label, _property), index) in tenant_map.iter_mut() {
            if !node.labels.iter().any(|node_label| node_label == label) {
                continue;
            }
            match index.upsert_from_properties(&node.id, &node.properties) {
                Ok(()) => {}
                Err(_) => index.remove(&node.id),
            }
        }
    }

    #[cfg(feature = "geometry")]
    pub fn geometry_contains_point(
        &self,
        tenant_id: &str,
        label: &str,
        property: &str,
        lat: f64,
        lon: f64,
    ) -> Result<Vec<String>, StoreAccessError> {
        let indexes = self
            .geometry_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("geometry index lock poisoned"))?;
        let index = geometry_index(&indexes, tenant_id, label, property)?;
        index
            .contains_point(lat, lon)
            .map_err(|err| StoreAccessError::unsupported(err.message()))
    }

    #[cfg(feature = "geometry")]
    pub fn geometry_intersects(
        &self,
        tenant_id: &str,
        label: &str,
        property: &str,
        encoding: GeometryEncoding,
        geometry: &Value,
    ) -> Result<Vec<String>, StoreAccessError> {
        let indexes = self
            .geometry_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("geometry index lock poisoned"))?;
        let index = geometry_index(&indexes, tenant_id, label, property)?;
        index
            .intersects_value(encoding, geometry)
            .map_err(|err| StoreAccessError::unsupported(err.message()))
    }

    #[cfg(feature = "geometry")]
    pub fn geometry_within(
        &self,
        tenant_id: &str,
        label: &str,
        property: &str,
        encoding: GeometryEncoding,
        geometry: &Value,
    ) -> Result<Vec<String>, StoreAccessError> {
        let indexes = self
            .geometry_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("geometry index lock poisoned"))?;
        let index = geometry_index(&indexes, tenant_id, label, property)?;
        index
            .within_value(encoding, geometry)
            .map_err(|err| StoreAccessError::unsupported(err.message()))
    }

    pub fn begin_graph_transaction(&self, tenant_id: &str) -> Result<String, StoreAccessError> {
        self.purge_expired_graph_transactions()?;
        let store = match self.tenant_graph_store(tenant_id)? {
            TenantGraphStore::RedCore(store) => store,
            TenantGraphStore::Redis(_) => {
                return Err(StoreAccessError::unsupported(
                    "graph transactions are supported for RedCore-backed tenants only",
                ));
            }
        };
        let snapshot_version = store.stats().map_err(StoreAccessError::from)?.version;
        let tx_id = format!(
            "tx-{}",
            self.next_graph_txn_id.fetch_add(1, Ordering::Relaxed)
        );
        let context = GraphTransactionContext {
            tenant_id: tenant_id.to_string(),
            snapshot_version,
            created_at_ms: now_millis(),
            mutations: GraphMutationBatch::default(),
        };
        let mut transactions = self
            .graph_transactions
            .lock()
            .map_err(|_| StoreAccessError::internal("graph transaction store lock poisoned"))?;
        transactions.insert(tx_id.clone(), context);
        Ok(tx_id)
    }

    pub fn append_graph_transaction_mutations(
        &self,
        tenant_id: &str,
        tx_id: &str,
        batch: GraphMutationBatch,
    ) -> Result<usize, StoreAccessError> {
        self.purge_expired_graph_transactions()?;
        if batch.mutations.is_empty() {
            return Err(StoreAccessError::from(GraphStoreError::new(
                "empty_graph_transaction",
                "transaction batch must include at least one mutation",
            )));
        }
        let mut transactions = self
            .graph_transactions
            .lock()
            .map_err(|_| StoreAccessError::internal("graph transaction store lock poisoned"))?;
        let Some(context) = transactions.get_mut(tx_id) else {
            return Err(StoreAccessError::unsupported("graph transaction not found"));
        };
        if context.tenant_id != tenant_id {
            return Err(StoreAccessError::unsupported(
                "graph transaction tenant mismatch",
            ));
        }
        context.mutations.mutations.extend(batch.mutations);
        Ok(context.mutations.mutations.len())
    }

    pub fn commit_graph_transaction(
        &self,
        tenant_id: &str,
        tx_id: &str,
    ) -> Result<GraphTransaction, StoreAccessError> {
        self.purge_expired_graph_transactions()?;
        let store = match self.tenant_graph_store(tenant_id)? {
            TenantGraphStore::RedCore(store) => store,
            TenantGraphStore::Redis(_) => {
                return Err(StoreAccessError::unsupported(
                    "graph transactions are supported for RedCore-backed tenants only",
                ));
            }
        };
        let context = {
            let transactions = self
                .graph_transactions
                .lock()
                .map_err(|_| StoreAccessError::internal("graph transaction store lock poisoned"))?;
            let context = transactions.get(tx_id).ok_or_else(|| {
                StoreAccessError::unsupported("graph transaction not found or already committed")
            })?;
            if context.tenant_id != tenant_id {
                return Err(StoreAccessError::unsupported(
                    "graph transaction tenant mismatch",
                ));
            }
            context.clone()
        };
        if context.mutations.mutations.is_empty() {
            return Err(StoreAccessError::from(GraphStoreError::new(
                "empty_graph_transaction",
                "graph transactions must include at least one mutation",
            )));
        }
        let current_version = store.stats().map_err(StoreAccessError::from)?.version;
        if current_version != context.snapshot_version {
            return Err(StoreAccessError::unsupported(
                "graph transaction snapshot conflict",
            ));
        }
        let committed_mutations = context.mutations.clone();
        let transaction = store
            .commit_batch(committed_mutations.clone())
            .map_err(StoreAccessError::from)?;
        for mutation in &committed_mutations.mutations {
            if let GraphMutation::NodeUpsert(node) = mutation {
                self.maybe_index_node_spatially(tenant_id, node);
                self.maybe_index_node_fulltext(tenant_id, node);
                #[cfg(feature = "geometry")]
                self.maybe_index_node_geometry(tenant_id, node);
            }
        }
        let mut transactions = self
            .graph_transactions
            .lock()
            .map_err(|_| StoreAccessError::internal("graph transaction store lock poisoned"))?;
        transactions.remove(tx_id);
        Ok(transaction)
    }

    pub fn rollback_graph_transaction(
        &self,
        tenant_id: &str,
        tx_id: &str,
    ) -> Result<(), StoreAccessError> {
        self.purge_expired_graph_transactions()?;
        let mut transactions = self
            .graph_transactions
            .lock()
            .map_err(|_| StoreAccessError::internal("graph transaction store lock poisoned"))?;
        let Some(context) = transactions.get(tx_id) else {
            return Err(StoreAccessError::unsupported("graph transaction not found"));
        };
        if context.tenant_id != tenant_id {
            return Err(StoreAccessError::unsupported(
                "graph transaction tenant mismatch",
            ));
        }
        transactions.remove(tx_id);
        Ok(())
    }

    fn purge_expired_graph_transactions(&self) -> Result<(), StoreAccessError> {
        let now_ms = now_millis();
        self.purge_expired_graph_transactions_at(now_ms)
    }

    fn purge_expired_graph_transactions_at(&self, now_ms: u64) -> Result<(), StoreAccessError> {
        let mut transactions = self
            .graph_transactions
            .lock()
            .map_err(|_| StoreAccessError::internal("graph transaction store lock poisoned"))?;
        transactions.retain(|_, context| {
            now_ms.saturating_sub(context.created_at_ms) <= GRAPH_TRANSACTION_TTL_MS
        });
        Ok(())
    }

    pub fn tenant_store(&self, tenant_id: &str) -> Result<RedisRustyredStore, StoreAccessError> {
        self.config.validate().map_err(StoreAccessError::internal)?;
        if self.config.storage_mode != StorageMode::Redis {
            return Err(StoreAccessError::unsupported(
                "run/context state commands are available only in RUSTY_RED_MODE=redis in this slice",
            ));
        }
        RedisRustyredStore::new(&self.config.redis_url, self.tenant_state_key(tenant_id))
            .map_err(StoreAccessError::from)
    }

    pub fn tenant_state_key(&self, tenant_id: &str) -> String {
        let safe_tenant = sanitize_tenant_segment(tenant_id);
        format!("{}:{}:state:v1", self.config.redis_key_prefix, safe_tenant)
    }

    pub fn tenant_graph_store(
        &self,
        tenant_id: &str,
    ) -> Result<TenantGraphStore, StoreAccessError> {
        self.config.validate().map_err(StoreAccessError::internal)?;
        match self.config.storage_mode {
            StorageMode::Embedded => Ok(TenantGraphStore::RedCore(
                self.redcore_store_for_tenant(tenant_id)?,
            )),
            StorageMode::Memory => Ok(TenantGraphStore::RedCore(
                self.memory_store_for_tenant(tenant_id)?,
            )),
            StorageMode::Redis => RedisGraphStore::tenant(
                &self.config.redis_url,
                &self.config.redis_key_prefix,
                tenant_id,
            )
            .map(TenantGraphStore::Redis)
            .map_err(StoreAccessError::from),
        }
    }

    pub fn store_ready(&self) -> Result<ReadyReport, StoreAccessError> {
        self.config.validate().map_err(StoreAccessError::internal)?;
        match self.config.storage_mode {
            StorageMode::Embedded => {
                let data_dir = PathBuf::from(&self.config.data_dir);
                RedCoreGraphStore::readiness_check(
                    &data_dir,
                    self.config.durability,
                    self.config.strict_acid,
                )
                .map_err(StoreAccessError::from)?;
                Ok(ReadyReport {
                    mode: "embedded".to_string(),
                    store: "ready".to_string(),
                    durability: self.config.durability.as_str().to_string(),
                    strict_acid: self.config.strict_acid,
                    require_volume: self.config.require_volume,
                    data_dir: Some(data_dir.display().to_string()),
                })
            }
            StorageMode::Memory => Ok(ReadyReport {
                mode: "memory".to_string(),
                store: "ready".to_string(),
                durability: "none".to_string(),
                strict_acid: false,
                require_volume: false,
                data_dir: None,
            }),
            StorageMode::Redis => {
                let key = format!("{}:__ready__:state:v1", self.config.redis_key_prefix);
                RedisRustyredStore::new(&self.config.redis_url, key)
                    .and_then(|store| store.ping())
                    .map_err(StoreAccessError::from)?;
                Ok(ReadyReport {
                    mode: "redis".to_string(),
                    store: "ready".to_string(),
                    durability: "redis".to_string(),
                    strict_acid: false,
                    require_volume: false,
                    data_dir: None,
                })
            }
        }
    }

    pub fn mcp_config(&self) -> McpServerConfig {
        McpServerConfig {
            name: self.config.service_name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_tenant: self.config.mcp_default_tenant.clone(),
            read_only: self.config.mcp_read_only,
            allow_admin: self.config.mcp_allow_admin,
        }
    }

    pub fn tenant_graph_cache(
        &self,
        tenant_id: &str,
    ) -> Result<Arc<GraphCacheTenant>, StoreAccessError> {
        let safe_tenant = sanitize_tenant_segment(tenant_id);
        let mut caches = self
            .graph_caches
            .lock()
            .map_err(|_| StoreAccessError::internal("graph cache tenant map lock poisoned"))?;
        if let Some(cache) = caches.get(&safe_tenant) {
            return Ok(cache.clone());
        }
        let cache = Arc::new(GraphCacheTenant::default());
        caches.insert(safe_tenant, cache.clone());
        Ok(cache)
    }

    fn redcore_store_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Arc<RedCoreTenantExecutor>, StoreAccessError> {
        let safe_tenant = sanitize_tenant_segment(tenant_id);
        let mut stores = self
            .redcore_stores
            .lock()
            .map_err(|_| StoreAccessError::internal("redcore tenant map lock poisoned"))?;
        if let Some(store) = stores.get(&safe_tenant) {
            return Ok(store.clone());
        }
        let data_dir = PathBuf::from(&self.config.data_dir)
            .join("tenants")
            .join(&safe_tenant);
        let tenant_config = self.config.tenant_config(tenant_id);
        let options = RedCoreOptions {
            durability: tenant_config.durability,
            snapshot_interval_writes: tenant_config.snapshot_interval_writes,
            strict_acid: tenant_config.strict_acid,
        };
        let store = Arc::new(RedCoreTenantExecutor::new(
            RedCoreGraphStore::open(data_dir, options)?,
            tenant_config.tenant_memory_quota_bytes,
        )?);
        stores.insert(safe_tenant, store.clone());
        Ok(store)
    }

    fn memory_store_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Arc<RedCoreTenantExecutor>, StoreAccessError> {
        let safe_tenant = sanitize_tenant_segment(tenant_id);
        let mut stores = self
            .redcore_stores
            .lock()
            .map_err(|_| StoreAccessError::internal("redcore tenant map lock poisoned"))?;
        if let Some(store) = stores.get(&safe_tenant) {
            return Ok(store.clone());
        }
        let tenant_config = self.config.tenant_config(tenant_id);
        let store = Arc::new(RedCoreTenantExecutor::new(
            RedCoreGraphStore::memory(),
            tenant_config.tenant_memory_quota_bytes,
        )?);
        stores.insert(safe_tenant, store.clone());
        Ok(store)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ReadyReport {
    pub mode: String,
    pub store: String,
    pub durability: String,
    pub strict_acid: bool,
    pub require_volume: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
}

#[derive(Debug)]
pub struct StoreAccessError {
    pub code: String,
    pub message: String,
}

impl StoreAccessError {
    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: "store_mode_unsupported".to_string(),
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: "store_internal_error".to_string(),
            message: message.into(),
        }
    }

    pub fn as_payload(&self) -> serde_json::Value {
        json!({
            "error": "store_unavailable",
            "code": self.code,
            "message": self.message
        })
    }
}

impl From<redis::RedisError> for StoreAccessError {
    fn from(error: redis::RedisError) -> Self {
        Self {
            code: "redis_store_error".to_string(),
            message: error.to_string(),
        }
    }
}

impl From<GraphStoreError> for StoreAccessError {
    fn from(error: GraphStoreError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

#[derive(Debug)]
pub struct RedCoreTenantExecutor {
    writer: Mutex<RedCoreGraphStore>,
    committed_snapshot: RwLock<InMemoryGraphStore>,
    tenant_memory_quota_bytes: usize,
    /// §P6-A pa6.1: cached `(graph_version, edges)` pair. Algorithm endpoints
    /// share the underlying allocation across concurrent calls; any mutation
    /// that bumps `graph_version` triggers a rebuild on the next read.
    cached_edges: RwLock<Option<(u64, Arc<Vec<EdgeRecord>>)>>,
}

impl RedCoreTenantExecutor {
    fn new(store: RedCoreGraphStore, tenant_memory_quota_bytes: usize) -> GraphStoreResult<Self> {
        let committed_snapshot = store.committed_view();
        Ok(Self {
            writer: Mutex::new(store),
            committed_snapshot: RwLock::new(committed_snapshot),
            tenant_memory_quota_bytes,
            cached_edges: RwLock::new(None),
        })
    }

    /// §P6-A pa6.1: cheap `Arc<Vec<EdgeRecord>>` clone for algorithm endpoints.
    /// Reads the cached arc when `graph_version` matches the current snapshot;
    /// rebuilds otherwise. Concurrent callers share the same allocation.
    pub fn list_edges_arc(&self) -> GraphStoreResult<Arc<Vec<EdgeRecord>>> {
        let current_version = self.stats()?.version;
        {
            let guard = self.cached_edges.read().map_err(|_| {
                GraphStoreError::new(
                    "redcore_snapshot_lock_poisoned",
                    "RedCore arc-cache lock poisoned",
                )
            })?;
            if let Some((cached_version, arc)) = guard.as_ref() {
                if *cached_version == current_version {
                    return Ok(Arc::clone(arc));
                }
            }
        }
        let edges = self.with_snapshot(|snapshot| snapshot.snapshot().edges)?;
        let arc = Arc::new(edges);
        let mut guard = self.cached_edges.write().map_err(|_| {
            GraphStoreError::new(
                "redcore_snapshot_lock_poisoned",
                "RedCore arc-cache lock poisoned",
            )
        })?;
        *guard = Some((current_version, Arc::clone(&arc)));
        Ok(arc)
    }

    pub fn commit_batch(&self, batch: GraphMutationBatch) -> GraphStoreResult<GraphTransaction> {
        let mut writer = self.lock_writer()?;
        self.enforce_tenant_memory_quota(&writer, &batch)?;
        let transaction = writer.commit_batch(batch)?;
        let committed_snapshot = writer.committed_view();
        *self.committed_snapshot.write().map_err(|_| {
            GraphStoreError::new(
                "redcore_snapshot_lock_poisoned",
                "RedCore committed snapshot lock poisoned",
            )
        })? = committed_snapshot;
        Ok(transaction)
    }

    pub fn upsert_node(&self, node: NodeRecord) -> GraphStoreResult<GraphWriteResult> {
        self.commit_batch(GraphMutationBatch::new([GraphMutation::NodeUpsert(node)]))?
            .writes
            .into_iter()
            .next()
            .ok_or_else(|| GraphStoreError::new("redcore_missing_write", "node write vanished"))
    }

    pub fn upsert_edge(&self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        self.commit_batch(GraphMutationBatch::new([GraphMutation::EdgeUpsert(edge)]))?
            .writes
            .into_iter()
            .next()
            .ok_or_else(|| GraphStoreError::new("redcore_missing_write", "edge write vanished"))
    }

    #[cfg(test)]
    pub fn read_barrier(&self) -> GraphStoreResult<u64> {
        Ok(self.lock_writer()?.status().last_txn_id)
    }

    pub fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        self.with_snapshot(|snapshot| snapshot.get_node(id).cloned())
    }

    pub fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        self.with_snapshot(|snapshot| snapshot.get_edge(id).cloned())
    }

    pub fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        self.with_snapshot(|snapshot| snapshot.query_nodes(query))
    }

    pub fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        self.with_snapshot(|snapshot| snapshot.neighbors(query))
    }

    pub fn stats(&self) -> GraphStoreResult<GraphStats> {
        self.with_snapshot(|snapshot| {
            let mut stats = snapshot.stats();
            stats.memory_quota_bytes = self.tenant_memory_quota_bytes;
            stats
        })
    }

    pub fn verify(&self) -> GraphStoreResult<VerifyReport> {
        self.with_snapshot(|snapshot| snapshot.verify())
    }

    pub fn rebuild_indexes(&self) -> GraphStoreResult<GraphRebuildReport> {
        let mut writer = self.lock_writer()?;
        let report = writer.rebuild_indexes()?;
        let committed_snapshot = writer.committed_view();
        *self.committed_snapshot.write().map_err(|_| {
            GraphStoreError::new(
                "redcore_snapshot_lock_poisoned",
                "RedCore committed snapshot lock poisoned",
            )
        })? = committed_snapshot;
        Ok(report)
    }

    pub fn labels(&self) -> GraphStoreResult<Vec<String>> {
        self.with_snapshot(|snapshot| snapshot.labels())
    }

    pub fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        self.with_snapshot(|snapshot| snapshot.edge_types())
    }

    pub fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        self.with_snapshot(|snapshot| snapshot.property_keys())
    }

    /// Phase 6: snapshot all live edges for graph-algorithm endpoints.
    /// Returns a clone of the edge vector; caller must not hold a lock.
    pub fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        self.with_snapshot(|snapshot| snapshot.snapshot().edges)
    }

    pub fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        self.with_snapshot(|snapshot| snapshot.snapshot())
    }

    pub fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        self.with_snapshot(|snapshot| {
            snapshot.epistemic_neighbors(node_id, epistemic_types, min_confidence, max_depth)
        })
    }

    pub fn designate_vector_property(
        &self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        let mut writer = self.lock_writer()?;
        writer.designate_vector_property(label, property_name, dimension)?;
        let committed_snapshot = writer.committed_view();
        *self.committed_snapshot.write().map_err(|_| {
            GraphStoreError::new(
                "redcore_snapshot_lock_poisoned",
                "RedCore committed snapshot lock poisoned",
            )
        })? = committed_snapshot;
        Ok(())
    }

    pub fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        self.with_snapshot(|snapshot| snapshot.vector_designations())
    }

    pub fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.with_snapshot(|snapshot| snapshot.vector_search(label, property_name, query, k))?
    }

    pub fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.with_snapshot(|snapshot| {
            snapshot.hybrid_search(label, property_name, query, k, graph_seeds, max_hops, alpha)
        })?
    }

    pub fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.with_snapshot(|snapshot| {
            snapshot.hybrid_search_with_config(
                label,
                property_name,
                query,
                k,
                graph_seeds,
                max_hops,
                config,
            )
        })?
    }

    fn lock_writer(&self) -> GraphStoreResult<std::sync::MutexGuard<'_, RedCoreGraphStore>> {
        self.writer.lock().map_err(|_| {
            GraphStoreError::new(
                "redcore_writer_lock_poisoned",
                "RedCore writer lock poisoned",
            )
        })
    }

    fn with_snapshot<T>(&self, read: impl FnOnce(&InMemoryGraphStore) -> T) -> GraphStoreResult<T> {
        let snapshot = self.committed_snapshot.read().map_err(|_| {
            GraphStoreError::new(
                "redcore_snapshot_lock_poisoned",
                "RedCore committed snapshot lock poisoned",
            )
        })?;
        Ok(read(&snapshot))
    }

    fn enforce_tenant_memory_quota(
        &self,
        writer: &RedCoreGraphStore,
        batch: &GraphMutationBatch,
    ) -> GraphStoreResult<()> {
        if self.tenant_memory_quota_bytes == 0 {
            return Ok(());
        }

        let mut projected_store = InMemoryGraphStore::from_snapshot(writer.graph_snapshot())?;
        for mutation in &batch.mutations {
            match mutation {
                GraphMutation::NodeUpsert(node) => {
                    projected_store.upsert_node(node.clone())?;
                }
                GraphMutation::EdgeUpsert(edge) => {
                    projected_store.upsert_edge(edge.clone())?;
                }
            }
        }

        let projected_memory = projected_store.stats().memory_bytes;
        if projected_memory > self.tenant_memory_quota_bytes {
            return Err(GraphStoreError::new(
                "tenant_memory_quota_exceeded",
                format!(
                    "tenant memory quota exceeded: projected {projected_memory} > quota {}",
                    self.tenant_memory_quota_bytes,
                ),
            ));
        }

        Ok(())
    }
}

#[cfg(feature = "geometry")]
fn geometry_index<'a>(
    indexes: &'a GeometryIndexes,
    tenant_id: &str,
    label: &str,
    property: &str,
) -> Result<&'a GeometryIndex, StoreAccessError> {
    let Some(tenant_map) = indexes.get(tenant_id) else {
        return Err(StoreAccessError::unsupported(
            "no geometry designations for this tenant",
        ));
    };
    tenant_map
        .get(&(label.to_string(), property.to_string()))
        .ok_or_else(|| {
            StoreAccessError::unsupported(
                "geometry designation not found; call /spatial/designate_geometry first",
            )
        })
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[derive(Clone)]
pub enum TenantGraphStore {
    RedCore(Arc<RedCoreTenantExecutor>),
    Redis(RedisGraphStore),
}

impl TenantGraphStore {
    pub fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<GraphWriteResult> {
        match self {
            Self::RedCore(store) => store.upsert_node(node),
            Self::Redis(store) => store.upsert_node(node),
        }
    }

    pub fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        match self {
            Self::RedCore(store) => store.upsert_edge(edge),
            Self::Redis(store) => store.upsert_edge(edge),
        }
    }

    /// Apply a `GraphMutationBatch` in a single transaction. RedCore uses its
    /// transactional `commit_batch`. Redis loops per-mutation upserts (it has
    /// no atomic batch primitive yet); callers still get the bulk-loader's
    /// `batches:` counter, just without single-transaction semantics on Redis.
    pub fn commit_batch(
        &mut self,
        batch: rustyred_core::GraphMutationBatch,
    ) -> GraphStoreResult<rustyred_core::GraphTransaction> {
        match self {
            Self::RedCore(executor) => executor.commit_batch(batch),
            Self::Redis(store) => {
                let mut writes: Vec<rustyred_core::GraphWriteResult> = Vec::new();
                for mutation in batch.mutations {
                    match mutation {
                        rustyred_core::GraphMutation::NodeUpsert(node) => {
                            writes.push(store.upsert_node(node)?);
                        }
                        rustyred_core::GraphMutation::EdgeUpsert(edge) => {
                            writes.push(store.upsert_edge(edge)?);
                        }
                    }
                }
                let graph_version = store.stats().map(|s| s.version).unwrap_or(0);
                Ok(rustyred_core::GraphTransaction {
                    txn_id: writes.len() as u64,
                    graph_version,
                    writes,
                })
            }
        }
    }

    pub fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        match self {
            Self::RedCore(store) => store.get_node(id),
            Self::Redis(store) => store.get_node(id),
        }
    }

    pub fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        match self {
            Self::RedCore(store) => store.get_edge(id),
            Self::Redis(store) => store.get_edge(id),
        }
    }

    pub fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        match self {
            Self::RedCore(store) => store.query_nodes(query),
            Self::Redis(store) => store.query_nodes(query),
        }
    }

    pub fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        match self {
            Self::RedCore(store) => store.neighbors(query),
            Self::Redis(store) => store.neighbors(query),
        }
    }

    pub fn stats(&self) -> GraphStoreResult<GraphStats> {
        match self {
            Self::RedCore(store) => store.stats(),
            Self::Redis(store) => store.stats(),
        }
    }

    pub fn verify(&self) -> GraphStoreResult<VerifyReport> {
        match self {
            Self::RedCore(store) => store.verify(),
            Self::Redis(store) => store.verify(),
        }
    }

    pub fn rebuild_indexes(&mut self) -> GraphStoreResult<GraphRebuildReport> {
        match self {
            Self::RedCore(store) => store.rebuild_indexes(),
            Self::Redis(store) => store.rebuild_indexes(),
        }
    }

    pub fn labels(&self) -> GraphStoreResult<Vec<String>> {
        match self {
            Self::RedCore(store) => store.labels(),
            Self::Redis(store) => store.labels(),
        }
    }

    pub fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        match self {
            Self::RedCore(store) => store.edge_types(),
            Self::Redis(store) => store.edge_types(),
        }
    }

    pub fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        match self {
            Self::RedCore(store) => store.property_keys(),
            Self::Redis(store) => store.property_keys(),
        }
    }

    /// Phase 6: snapshot all live edges for graph algorithms.
    /// Redis backend is currently unsupported (would require a full scan).
    pub fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        match self {
            Self::RedCore(store) => store.list_edges(),
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "graph algorithms are not supported on Redis graph stores",
            )),
        }
    }

    pub fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        match self {
            Self::RedCore(store) => store.graph_snapshot(),
            Self::Redis(_) => Err(GraphStoreError::new(
                "legacy_redis_instant_kg_unsupported",
                "instant KG requires the native RedCore graph store; RUSTY_RED_MODE=redis is a legacy compatibility path and should be changed to RUSTY_RED_MODE=embedded",
            )),
        }
    }

    /// §P6-A pa6.1: `Arc<Vec<EdgeRecord>>` snapshot used by the algorithm
    /// endpoints. The RedCore variant returns a shared, version-cached arc;
    /// callers must not mutate.
    pub fn list_edges_arc(&self) -> GraphStoreResult<Arc<Vec<EdgeRecord>>> {
        match self {
            Self::RedCore(store) => store.list_edges_arc(),
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "graph algorithms are not supported on Redis graph stores",
            )),
        }
    }

    pub fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        match self {
            Self::RedCore(store) => {
                store.epistemic_neighbors(node_id, epistemic_types, min_confidence, max_depth)
            }
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "epistemic_neighbors is not supported on Redis graph stores",
            )),
        }
    }

    pub fn designate_vector_property(
        &self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        match self {
            Self::RedCore(store) => {
                store.designate_vector_property(label, property_name, dimension)
            }
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "designate_vector_property is not supported on Redis graph stores",
            )),
        }
    }

    pub fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        match self {
            Self::RedCore(store) => store.vector_designations(),
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "vector_designations is not supported on Redis graph stores",
            )),
        }
    }

    pub fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        match self {
            Self::RedCore(store) => store.vector_search(label, property_name, query, k),
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "vector_search is not supported on Redis graph stores",
            )),
        }
    }

    pub fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        match self {
            Self::RedCore(store) => {
                store.hybrid_search(label, property_name, query, k, graph_seeds, max_hops, alpha)
            }
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "hybrid_search is not supported on Redis graph stores",
            )),
        }
    }

    pub fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        match self {
            Self::RedCore(store) => store.hybrid_search_with_config(
                label,
                property_name,
                query,
                k,
                graph_seeds,
                max_hops,
                config,
            ),
            Self::Redis(_) => Err(GraphStoreError::new(
                "unsupported_operation",
                "hybrid_search is not supported on Redis graph stores",
            )),
        }
    }
}

impl McpGraphBackend for TenantGraphStore {
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        TenantGraphStore::get_node(self, id)
    }

    fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        TenantGraphStore::get_edge(self, id)
    }

    fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        TenantGraphStore::query_nodes(self, query)
    }

    fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        TenantGraphStore::neighbors(self, query)
    }

    fn stats(&self) -> GraphStoreResult<GraphStats> {
        TenantGraphStore::stats(self)
    }

    fn verify(&self) -> GraphStoreResult<VerifyReport> {
        TenantGraphStore::verify(self)
    }

    fn labels(&self) -> GraphStoreResult<Vec<String>> {
        TenantGraphStore::labels(self)
    }

    fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        TenantGraphStore::edge_types(self)
    }

    fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        TenantGraphStore::property_keys(self)
    }

    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        TenantGraphStore::list_edges(self)
    }

    fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        TenantGraphStore::graph_snapshot(self)
    }

    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<()> {
        TenantGraphStore::upsert_node(self, node).map(|_| ())
    }

    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        TenantGraphStore::upsert_edge(self, edge).map(|_| ())
    }

    fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        TenantGraphStore::vector_designations(self)
    }

    fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        TenantGraphStore::designate_vector_property(self, label, property_name, dimension)
    }

    fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        TenantGraphStore::vector_search(self, label, property_name, query, k)
    }

    fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        TenantGraphStore::hybrid_search(
            self,
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            alpha,
        )
    }

    fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        TenantGraphStore::hybrid_search_with_config(
            self,
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            config,
        )
    }

    fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        TenantGraphStore::epistemic_neighbors(
            self,
            node_id,
            epistemic_types,
            min_confidence,
            max_depth,
        )
    }

    /// §P6-A pa6.1: override the trait defaults to read `list_edges_arc` so
    /// concurrent algorithm endpoints share one allocation rather than each
    /// cloning the live edge vector.
    fn algo_ppr(
        &self,
        seeds: &std::collections::HashMap<String, f64>,
        alpha: f64,
        epsilon: f64,
        max_pushes: usize,
    ) -> GraphStoreResult<std::collections::HashMap<String, f64>> {
        let edges = self.list_edges_arc()?;
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
        Ok(rustyred_core::personalized_pagerank(
            &adjacency, seeds, alpha, epsilon, max_pushes,
        ))
    }

    fn algo_components(&self, directed: bool) -> GraphStoreResult<Vec<Vec<String>>> {
        let edges = self.list_edges_arc()?;
        Ok(rustyred_core::connected_components(
            edges.as_slice(),
            directed,
        ))
    }

    fn algo_pagerank(
        &self,
        damping: f64,
        max_iter: usize,
        tolerance: f64,
    ) -> GraphStoreResult<std::collections::HashMap<String, f64>> {
        let edges = self.list_edges_arc()?;
        Ok(rustyred_core::pagerank(
            edges.as_slice(),
            damping,
            max_iter,
            tolerance,
        ))
    }

    fn algo_communities(&self) -> GraphStoreResult<(std::collections::HashMap<String, u64>, f64)> {
        let edges = self.list_edges_arc()?;
        Ok(rustyred_core::label_propagation_communities(
            edges.as_slice(),
        ))
    }
}

#[derive(Clone)]
pub struct ProductMcpBackend {
    state: AppState,
    tenant_id: String,
    store: TenantGraphStore,
}

impl McpGraphBackend for ProductMcpBackend {
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        self.store.get_node(id)
    }

    fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        self.store.get_edge(id)
    }

    fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        self.store.query_nodes(query)
    }

    fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        self.store.neighbors(query)
    }

    fn stats(&self) -> GraphStoreResult<GraphStats> {
        self.store.stats()
    }

    fn verify(&self) -> GraphStoreResult<VerifyReport> {
        self.store.verify()
    }

    fn labels(&self) -> GraphStoreResult<Vec<String>> {
        self.store.labels()
    }

    fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        self.store.edge_types()
    }

    fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        self.store.property_keys()
    }

    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        self.store.list_edges()
    }

    fn graph_snapshot(&self) -> GraphStoreResult<GraphSnapshot> {
        self.store.graph_snapshot()
    }

    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<()> {
        self.store.upsert_node(node.clone())?;
        self.state.observability.record_mutation();
        self.state
            .maybe_index_node_spatially(&self.tenant_id, &node);
        self.state.maybe_index_node_fulltext(&self.tenant_id, &node);
        #[cfg(feature = "geometry")]
        self.state.maybe_index_node_geometry(&self.tenant_id, &node);
        Ok(())
    }

    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        self.store.upsert_edge(edge)?;
        self.state.observability.record_mutation();
        Ok(())
    }

    fn vector_designations(&self) -> GraphStoreResult<Vec<VectorDesignation>> {
        self.store.vector_designations()
    }

    fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        self.store
            .designate_vector_property(label, property_name, dimension)
    }

    fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.store.vector_search(label, property_name, query, k)
    }

    fn hybrid_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        alpha: f32,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.store
            .hybrid_search(label, property_name, query, k, graph_seeds, max_hops, alpha)
    }

    fn hybrid_scoring_config(&self) -> HybridScoringConfig {
        self.state
            .config
            .tenant_config(&self.tenant_id)
            .hybrid_scoring
    }

    fn hybrid_search_with_config(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.store.hybrid_search_with_config(
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            config,
        )
    }

    fn designate_fulltext_property(&mut self, label: &str, property: &str) -> GraphStoreResult<()> {
        self.state
            .designate_fulltext_property(&self.tenant_id, label, property)
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    fn fulltext_search(
        &self,
        label: Option<&str>,
        property: &str,
        query: &str,
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.state
            .fulltext_search(&self.tenant_id, label, property, query, k)
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    fn designate_spatial_property(
        &mut self,
        label: &str,
        lat_property: &str,
        lon_property: &str,
        resolution: u8,
    ) -> GraphStoreResult<()> {
        self.state
            .designate_spatial_property(
                &self.tenant_id,
                label,
                lat_property,
                lon_property,
                resolution,
            )
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    fn spatial_radius_search(
        &self,
        label: &str,
        lat_property: &str,
        lon_property: &str,
        lat: f64,
        lon: f64,
        radius_km: f64,
    ) -> GraphStoreResult<Vec<String>> {
        self.state
            .spatial_radius_search(
                &self.tenant_id,
                label,
                lat_property,
                lon_property,
                lat,
                lon,
                radius_km,
            )
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    fn spatial_bbox_search(
        &self,
        label: &str,
        lat_property: &str,
        lon_property: &str,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    ) -> GraphStoreResult<Vec<String>> {
        self.state
            .spatial_bbox_search(
                &self.tenant_id,
                label,
                lat_property,
                lon_property,
                min_lat,
                min_lon,
                max_lat,
                max_lon,
            )
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    #[cfg(feature = "geometry")]
    fn designate_geometry_property(
        &mut self,
        label: &str,
        property: &str,
        encoding: &str,
        resolution: u8,
    ) -> GraphStoreResult<()> {
        let encoding = parse_geometry_encoding_for_mcp(encoding)?;
        self.state
            .designate_geometry_property(&self.tenant_id, label, property, encoding, resolution)
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    #[cfg(feature = "geometry")]
    fn spatial_contains_point(
        &self,
        label: &str,
        property: &str,
        lat: f64,
        lon: f64,
    ) -> GraphStoreResult<Vec<String>> {
        self.state
            .geometry_contains_point(&self.tenant_id, label, property, lat, lon)
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    #[cfg(feature = "geometry")]
    fn spatial_intersects_geometry(
        &self,
        label: &str,
        property: &str,
        encoding: &str,
        geometry: &Value,
    ) -> GraphStoreResult<Vec<String>> {
        let encoding = parse_geometry_encoding_for_mcp(encoding)?;
        self.state
            .geometry_intersects(&self.tenant_id, label, property, encoding, geometry)
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    #[cfg(feature = "geometry")]
    fn spatial_within_geometry(
        &self,
        label: &str,
        property: &str,
        encoding: &str,
        geometry: &Value,
    ) -> GraphStoreResult<Vec<String>> {
        let encoding = parse_geometry_encoding_for_mcp(encoding)?;
        self.state
            .geometry_within(&self.tenant_id, label, property, encoding, geometry)
            .map_err(|error| GraphStoreError::new(error.code, error.message))
    }

    fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> GraphStoreResult<Vec<(EdgeRecord, NodeRecord)>> {
        self.store
            .epistemic_neighbors(node_id, epistemic_types, min_confidence, max_depth)
    }
}

#[cfg(feature = "geometry")]
fn parse_geometry_encoding_for_mcp(raw: &str) -> GraphStoreResult<GeometryEncoding> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "point" => Ok(GeometryEncoding::Point),
        "wkb" => Ok(GeometryEncoding::Wkb),
        "wkt" => Ok(GeometryEncoding::Wkt),
        "subgraph" => Ok(GeometryEncoding::Subgraph),
        other => Err(GraphStoreError::new(
            "invalid_geometry_encoding",
            format!("unsupported geometry encoding: {other}"),
        )),
    }
}

impl McpGraphProvider for AppState {
    type Backend = ProductMcpBackend;

    fn backend_for_tenant(&self, tenant: &str) -> Result<Self::Backend, McpError> {
        let store = self
            .tenant_graph_store(tenant)
            .map_err(|error| McpError::internal(error.message))?;
        Ok(ProductMcpBackend {
            state: self.clone(),
            tenant_id: tenant.to_string(),
            store,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, StorageMode};

    use super::{AppState, RedCoreTenantExecutor};
    use rustyred_core::{
        EdgeRecord, GraphMutation, GraphMutationBatch, NeighborQuery, NodeRecord,
        RedCoreDurability, RedCoreGraphStore,
    };
    use serde_json::json;
    use std::sync::{mpsc, Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn tenant_state_keys_use_graph_store_tenant_normalization() {
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
        });

        assert_eq!(
            state.tenant_state_key("Tenant.One!"),
            "rusty-red:TenantOne:state:v1"
        );
    }

    #[test]
    fn instant_kg_snapshot_rejects_legacy_redis_mode() {
        let mut config = memory_config();
        config.storage_mode = StorageMode::Redis;
        config.redis_url = "redis://127.0.0.1:6379".to_string();

        let state = AppState::new(config);
        let store = state
            .tenant_graph_store("tenant-kg")
            .expect("Redis client URL should parse without connecting");
        let err = store
            .graph_snapshot()
            .expect_err("Redis mode must be diagnostic");

        assert_eq!(err.code, "legacy_redis_instant_kg_unsupported");
        assert!(err.message.contains("RUSTY_RED_MODE=embedded"));

        let mcp = rustyred_mcp::handle_mcp_request(
            &state,
            &state.mcp_config(),
            json!({
                "jsonrpc": "2.0",
                "id": "instant-kg-status",
                "method": "tools/call",
                "params": {
                    "name": "harness_kg_status",
                    "arguments": { "tenant": "tenant-kg" }
                }
            }),
        );
        assert_eq!(
            mcp["error"]["data"]["code"],
            "legacy_redis_instant_kg_unsupported"
        );
    }

    #[test]
    fn embedded_graph_store_reopens_from_configured_data_dir_without_redis() {
        let data_dir = unique_test_dir("rusty-red-product-redcore");
        let config = Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Embedded,
            data_dir: data_dir.display().to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: true,
            concurrency: "single_writer".to_string(),
            txn_isolation: "serializable".to_string(),
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
        };
        {
            let state = AppState::new(config.clone());
            state.store_ready().unwrap();
            let mut store = state.tenant_graph_store("Tenant.One!").unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:embedded",
                    ["Embedded"],
                    json!({ "mode": "redcore" }),
                ))
                .unwrap();
        }

        let state = AppState::new(config);
        let store = state.tenant_graph_store("Tenant.One!").unwrap();
        assert_eq!(
            store.get_node("node:embedded").unwrap().unwrap().labels,
            vec!["Embedded".to_string()]
        );

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn embedded_readiness_rejects_missing_required_volume() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Embedded,
            data_dir: "data/rusty-red".to_string(),
            require_volume: true,
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

        let error = state.store_ready().unwrap_err();

        assert_eq!(error.code, "store_internal_error");
        assert!(error.message.contains("REQUIRE_VOLUME"));
    }

    #[test]
    fn memory_readiness_ignores_volume_requirement_and_reports_no_durability() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: true,
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

        let report = state.store_ready().unwrap();

        assert_eq!(report.mode, "memory");
        assert_eq!(report.durability, "none");
        assert!(!report.require_volume);
    }

    #[test]
    fn redcore_executor_serializes_concurrent_writes_with_monotonic_txn_ids() {
        let executor =
            Arc::new(RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 0).unwrap());
        let start = Arc::new(Barrier::new(9));
        let handles = (0..8)
            .map(|idx| {
                let executor = executor.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    executor
                        .commit_batch(GraphMutationBatch::new([GraphMutation::NodeUpsert(
                            NodeRecord::new(
                                format!("node:{idx}"),
                                ["Concurrent"],
                                json!({ "idx": idx }),
                            ),
                        )]))
                        .unwrap()
                        .txn_id
                })
            })
            .collect::<Vec<_>>();

        start.wait();
        let mut txn_ids = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        txn_ids.sort_unstable();

        assert_eq!(txn_ids, (1_u64..=8).collect::<Vec<_>>());
        assert_eq!(executor.stats().unwrap().nodes_total, 8);
        assert_eq!(executor.read_barrier().unwrap(), 8);
    }

    #[test]
    fn redcore_executor_vector_search_designate_then_upsert() {
        // Regression: the read-side committed snapshot was rebuilt from a
        // node/edge-only GraphSnapshot, which dropped vector designations, so
        // vector_search through the executor always returned []. The committed
        // view must carry designations and the HNSW index.
        let executor = RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 0).unwrap();
        executor
            .designate_vector_property("Claim", "embedding", 3)
            .unwrap();
        executor
            .upsert_node(NodeRecord::new(
                "claim:a",
                ["Claim"],
                json!({ "embedding": [1.0, 0.0, 0.0] }),
            ))
            .unwrap();
        executor
            .upsert_node(NodeRecord::new(
                "claim:b",
                ["Claim"],
                json!({ "embedding": [0.0, 1.0, 0.0] }),
            ))
            .unwrap();

        let results = executor
            .vector_search(Some("Claim"), "embedding", &[1.0, 0.0, 0.0], 5)
            .unwrap();

        assert!(
            results.iter().any(|(id, _)| id == "claim:a"),
            "designate-then-upsert must be vector-searchable through the executor, got {results:?}"
        );
    }

    #[test]
    fn redcore_executor_vector_search_upsert_then_designate_backfills() {
        // The other ordering: designating after nodes exist must backfill the
        // committed view's index too.
        let executor = RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 0).unwrap();
        executor
            .upsert_node(NodeRecord::new(
                "claim:a",
                ["Claim"],
                json!({ "embedding": [1.0, 0.0, 0.0] }),
            ))
            .unwrap();
        executor
            .designate_vector_property("Claim", "embedding", 3)
            .unwrap();

        let results = executor
            .vector_search(Some("Claim"), "embedding", &[1.0, 0.0, 0.0], 5)
            .unwrap();

        assert_eq!(results.first().map(|(id, _)| id.as_str()), Some("claim:a"));
    }

    #[test]
    fn redcore_executor_reads_only_committed_snapshots() {
        let executor = RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 0).unwrap();
        let error = executor
            .commit_batch(GraphMutationBatch::new([
                GraphMutation::NodeUpsert(NodeRecord::new(
                    "node:a",
                    ["File"],
                    json!({ "path": "src/lib.rs" }),
                )),
                GraphMutation::EdgeUpsert(EdgeRecord::new(
                    "edge:missing",
                    "node:a",
                    "IMPORTS",
                    "node:missing",
                    json!({}),
                )),
            ]))
            .unwrap_err();

        assert_eq!(error.code, "missing_graph_endpoint");
        assert!(executor.get_node("node:a").unwrap().is_none());
        assert_eq!(executor.stats().unwrap().version, 0);
        assert_eq!(executor.read_barrier().unwrap(), 0);

        let transaction = executor
            .commit_batch(GraphMutationBatch::new([
                GraphMutation::NodeUpsert(NodeRecord::new(
                    "node:a",
                    ["File"],
                    json!({ "path": "src/lib.rs" }),
                )),
                GraphMutation::NodeUpsert(NodeRecord::new(
                    "node:b",
                    ["File"],
                    json!({ "path": "src/main.rs" }),
                )),
                GraphMutation::EdgeUpsert(EdgeRecord::new(
                    "edge:ab",
                    "node:a",
                    "IMPORTS",
                    "node:b",
                    json!({}),
                )),
            ]))
            .unwrap();

        assert_eq!(executor.read_barrier().unwrap(), transaction.txn_id);
        assert_eq!(
            executor.neighbors(NeighborQuery::out("node:a")).unwrap()[0].node_id,
            "node:b"
        );
        assert!(executor.verify().unwrap().ok);
    }

    #[test]
    fn redcore_snapshot_reads_do_not_wait_for_writer_lock() {
        let executor =
            Arc::new(RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 0).unwrap());
        executor
            .commit_batch(GraphMutationBatch::new([GraphMutation::NodeUpsert(
                NodeRecord::new("node:committed", ["File"], json!({ "path": "src/lib.rs" })),
            )]))
            .unwrap();
        let _writer_guard = executor.lock_writer().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = executor.clone();

        thread::spawn(move || {
            let node = reader.get_node("node:committed").unwrap();
            tx.send(node.map(|node| node.id)).unwrap();
        });

        assert_eq!(
            rx.recv_timeout(Duration::from_millis(250)).unwrap(),
            Some("node:committed".to_string())
        );
    }

    #[test]
    fn redcore_executor_enforces_tenant_memory_quota_on_commit() {
        let executor = RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 1).unwrap();
        let error = executor
            .commit_batch(GraphMutationBatch::new([GraphMutation::NodeUpsert(
                NodeRecord::new("node:oversize", ["File"], json!({ "path": "src/lib.rs" })),
            )]))
            .unwrap_err();

        assert_eq!(error.code, "tenant_memory_quota_exceeded");
    }

    #[test]
    fn redcore_executor_includes_tenant_memory_quota_in_stats() {
        let executor = RedCoreTenantExecutor::new(RedCoreGraphStore::memory(), 128).unwrap();
        let stats = executor.stats().unwrap();

        assert_eq!(stats.memory_quota_bytes, 128);
    }

    #[test]
    fn graph_transactions_expire_after_ttl_interval() {
        let state = AppState::new(memory_config());

        let tx_id = state.begin_graph_transaction("tenant-a").unwrap();
        let mut stale_time = super::now_millis();
        stale_time += super::GRAPH_TRANSACTION_TTL_MS + 1;
        state
            .purge_expired_graph_transactions_at(stale_time)
            .expect("graph transaction expiry check");

        let error = state
            .append_graph_transaction_mutations(
                "tenant-a",
                &tx_id,
                GraphMutationBatch::new([GraphMutation::NodeUpsert(NodeRecord::new(
                    "node:ttl",
                    ["File"],
                    json!({ "path": "src/ttl.rs" }),
                ))]),
            )
            .unwrap_err();

        assert_eq!(error.code, "store_mode_unsupported");
        assert_eq!(error.message, "graph transaction not found");
    }

    #[test]
    fn graph_transaction_wrong_tenant_commit_preserves_staged_work() {
        let state = AppState::new(memory_config());
        let tx_id = state.begin_graph_transaction("tenant-a").unwrap();
        state
            .append_graph_transaction_mutations(
                "tenant-a",
                &tx_id,
                GraphMutationBatch::new([GraphMutation::NodeUpsert(NodeRecord::new(
                    "node:tenant-a",
                    ["File"],
                    json!({ "path": "src/lib.rs" }),
                ))]),
            )
            .unwrap();

        let error = state
            .commit_graph_transaction("tenant-b", &tx_id)
            .unwrap_err();
        assert_eq!(error.code, "store_mode_unsupported");
        assert_eq!(error.message, "graph transaction tenant mismatch");

        let transaction = state.commit_graph_transaction("tenant-a", &tx_id).unwrap();
        assert_eq!(transaction.writes.len(), 1);
        let store = state.tenant_graph_store("tenant-a").unwrap();
        assert!(store.get_node("node:tenant-a").unwrap().is_some());
    }

    #[test]
    fn graph_transaction_wrong_tenant_rollback_preserves_staged_work() {
        let state = AppState::new(memory_config());
        let tx_id = state.begin_graph_transaction("tenant-a").unwrap();

        let error = state
            .rollback_graph_transaction("tenant-b", &tx_id)
            .unwrap_err();
        assert_eq!(error.code, "store_mode_unsupported");
        assert_eq!(error.message, "graph transaction tenant mismatch");

        state
            .rollback_graph_transaction("tenant-a", &tx_id)
            .expect("owner tenant can still rollback after wrong-tenant attempt");
        let error = state
            .rollback_graph_transaction("tenant-a", &tx_id)
            .unwrap_err();
        assert_eq!(error.message, "graph transaction not found");
    }

    #[test]
    fn graph_transactions_do_not_survive_state_restart() {
        let config = memory_config();
        let tx_id = {
            let active_state = AppState::new(config.clone());
            active_state.begin_graph_transaction("tenant-a").unwrap()
        };
        let fresh_state = AppState::new(config);
        let error = fresh_state
            .commit_graph_transaction("tenant-a", &tx_id)
            .unwrap_err();

        assert_eq!(error.code, "store_mode_unsupported");
        assert_eq!(
            error.message,
            "graph transaction not found or already committed"
        );
    }

    #[test]
    fn product_mcp_backend_reaches_fulltext_spatial_and_bulk_tools() {
        let state = AppState::new(memory_config());
        let mut config = state.mcp_config();
        config.read_only = false;
        let write_context = rustyred_mcp::McpRequestContext::with_scopes(["graph:write"]);

        let bulk = rustyred_mcp::handle_mcp_request_with_context(
            &state,
            &config,
            &write_context,
            json!({
                "jsonrpc": "2.0",
                "id": "bulk",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.bulk.nodes",
                    "arguments": {
                        "tenant": "tenant-mcp",
                        "nodes": [
                            {
                                "id": "place:a",
                                "labels": ["Place"],
                                "properties": {
                                    "body": "north campus library",
                                    "lat": 42.0,
                                    "lon": -83.0
                                }
                            }
                        ]
                    }
                }
            }),
        );
        assert_eq!(bulk["result"]["structuredContent"]["inserted"], 1);

        let fulltext_designate = rustyred_mcp::handle_mcp_request_with_context(
            &state,
            &config,
            &write_context,
            json!({
                "jsonrpc": "2.0",
                "id": "ft-designate",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.fulltext.designate",
                    "arguments": {
                        "tenant": "tenant-mcp",
                        "label": "Place",
                        "property": "body"
                    }
                }
            }),
        );
        assert_eq!(
            fulltext_designate["result"]["structuredContent"]["designated"]["property"],
            "body"
        );
        let fulltext_search = rustyred_mcp::handle_mcp_request(
            &state,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "ft-search",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.fulltext.search",
                    "arguments": {
                        "tenant": "tenant-mcp",
                        "label": "Place",
                        "property": "body",
                        "query": "library"
                    }
                }
            }),
        );
        assert_eq!(
            fulltext_search["result"]["structuredContent"]["results"][0]["node_id"],
            "place:a"
        );

        let spatial_designate = rustyred_mcp::handle_mcp_request_with_context(
            &state,
            &config,
            &write_context,
            json!({
                "jsonrpc": "2.0",
                "id": "sp-designate",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.spatial.designate",
                    "arguments": {
                        "tenant": "tenant-mcp",
                        "label": "Place",
                        "lat_property": "lat",
                        "lon_property": "lon"
                    }
                }
            }),
        );
        assert_eq!(
            spatial_designate["result"]["structuredContent"]["designated"]["label"],
            "Place"
        );
        let spatial_search = rustyred_mcp::handle_mcp_request(
            &state,
            &config,
            json!({
                "jsonrpc": "2.0",
                "id": "sp-radius",
                "method": "tools/call",
                "params": {
                    "name": "rustyred.spatial.radius",
                    "arguments": {
                        "tenant": "tenant-mcp",
                        "label": "Place",
                        "lat_property": "lat",
                        "lon_property": "lon",
                        "lat": 42.0,
                        "lon": -83.0,
                        "radius_km": 1.0
                    }
                }
            }),
        );
        assert_eq!(
            spatial_search["result"]["structuredContent"]["node_ids"][0],
            "place:a"
        );
    }

    fn memory_config() -> Config {
        Config {
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
        }
    }

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{label}-{unique}"))
    }

    // ---- §P6-A pa6.1 algorithm-cache tests --------------------------------

    fn arc_cache_test_state() -> AppState {
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

    #[test]
    fn list_edges_arc_returns_shared_instance() {
        let state = arc_cache_test_state();
        let mut store = state.tenant_graph_store("tenant-arc").unwrap();
        store
            .upsert_node(NodeRecord::new("a", ["Doc"], json!({})))
            .unwrap();
        store
            .upsert_node(NodeRecord::new("b", ["Doc"], json!({})))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new("e1", "a", "T", "b", json!({})))
            .unwrap();
        let first = store.list_edges_arc().unwrap();
        let second = store.list_edges_arc().unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "successive list_edges_arc calls at the same graph_version must share the allocation",
        );
        assert!(Arc::strong_count(&first) >= 2);
    }

    #[test]
    fn list_edges_arc_rebuilds_after_mutation() {
        let state = arc_cache_test_state();
        let mut store = state.tenant_graph_store("tenant-arc-bump").unwrap();
        store
            .upsert_node(NodeRecord::new("a", ["Doc"], json!({})))
            .unwrap();
        let first = store.list_edges_arc().unwrap();
        store
            .upsert_node(NodeRecord::new("b", ["Doc"], json!({})))
            .unwrap();
        let second = store.list_edges_arc().unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "mutation should bump graph_version and invalidate the arc cache",
        );
    }
}
