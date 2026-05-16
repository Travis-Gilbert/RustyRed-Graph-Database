use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use thg_core::store::RedisThgStore;
use thg_core::{
    sanitize_tenant_segment, EdgeRecord, EpistemicType, GraphMutation, GraphMutationBatch,
    GraphRebuildReport, GraphStats, GraphStoreError, GraphStoreResult, GraphTransaction,
    GraphWriteResult, InMemoryGraphStore, NeighborHit, NeighborQuery, NodeQuery, NodeRecord,
    RedCoreGraphStore, RedCoreOptions, RedisGraphStore, SpatialDesignation, SpatialIndex,
    VectorDesignation, VerifyReport,
};
use thg_mcp::{McpError, McpGraphBackend, McpGraphProvider, McpServerConfig};

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
type SpatialIndexes = BTreeMap<String, BTreeMap<(String, String, String), SpatialIndex>>;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub observability: Observability,
    redcore_stores: Arc<Mutex<BTreeMap<String, Arc<RedCoreTenantExecutor>>>>,
    graph_caches: Arc<Mutex<BTreeMap<String, Arc<GraphCacheTenant>>>>,
    graph_transactions: Arc<Mutex<BTreeMap<String, GraphTransactionContext>>>,
    next_graph_txn_id: Arc<AtomicU64>,
    spatial_indexes: Arc<Mutex<SpatialIndexes>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            observability: Observability::default(),
            redcore_stores: Arc::new(Mutex::new(BTreeMap::new())),
            graph_caches: Arc::new(Mutex::new(BTreeMap::new())),
            graph_transactions: Arc::new(Mutex::new(BTreeMap::new())),
            next_graph_txn_id: Arc::new(AtomicU64::new(1)),
            spatial_indexes: Arc::new(Mutex::new(BTreeMap::new())),
        }
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
        let mut index = SpatialIndex::for_designation(designation.clone());
        // Bulk-index any existing nodes for the label.
        let nodes = store
            .query_nodes(NodeQuery {
                label: Some(label.to_string()),
                ..NodeQuery::default()
            })
            .map_err(StoreAccessError::from)?;
        for node in nodes {
            if let (Some(lat), Some(lon)) = (
                node.properties
                    .get(lat_property)
                    .and_then(|v| v.as_f64()),
                node.properties
                    .get(lon_property)
                    .and_then(|v| v.as_f64()),
            ) {
                let _ = index.upsert(&node.id, lat, lon);
            }
        }
        let mut indexes = self
            .spatial_indexes
            .lock()
            .map_err(|_| StoreAccessError::internal("spatial index lock poisoned"))?;
        indexes
            .entry(tenant_id.to_string())
            .or_default()
            .insert(
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
                let _ = index.upsert(&node.id, lat, lon);
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
        context
            .mutations
            .mutations
            .extend(batch.mutations.into_iter());
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
        let transaction = store
            .commit_batch(context.mutations)
            .map_err(StoreAccessError::from)?;
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

    pub fn tenant_store(&self, tenant_id: &str) -> Result<RedisThgStore, StoreAccessError> {
        self.config.validate().map_err(StoreAccessError::internal)?;
        if self.config.storage_mode != StorageMode::Redis {
            return Err(StoreAccessError::unsupported(
                "run/context state commands are available only in RUSTY_RED_MODE=redis in this slice",
            ));
        }
        RedisThgStore::new(&self.config.redis_url, self.tenant_state_key(tenant_id))
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
                RedisThgStore::new(&self.config.redis_url, key)
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
            version: "0.1.0".to_string(),
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
        let options = RedCoreOptions {
            durability: self.config.durability,
            snapshot_interval_writes: self.config.snapshot_interval_writes,
            strict_acid: self.config.strict_acid,
        };
        let store = Arc::new(RedCoreTenantExecutor::new(
            RedCoreGraphStore::open(data_dir, options)?,
            self.config.tenant_memory_quota_bytes,
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
        let store = Arc::new(RedCoreTenantExecutor::new(
            RedCoreGraphStore::memory(),
            self.config.tenant_memory_quota_bytes,
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
}

impl RedCoreTenantExecutor {
    fn new(store: RedCoreGraphStore, tenant_memory_quota_bytes: usize) -> GraphStoreResult<Self> {
        let committed_snapshot = InMemoryGraphStore::from_snapshot(store.graph_snapshot())?;
        Ok(Self {
            writer: Mutex::new(store),
            committed_snapshot: RwLock::new(committed_snapshot),
            tenant_memory_quota_bytes,
        })
    }

    pub fn commit_batch(&self, batch: GraphMutationBatch) -> GraphStoreResult<GraphTransaction> {
        let mut writer = self.lock_writer()?;
        self.enforce_tenant_memory_quota(&writer, &batch)?;
        let transaction = writer.commit_batch(batch)?;
        let committed_snapshot = InMemoryGraphStore::from_snapshot(writer.graph_snapshot())?;
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
        let committed_snapshot = InMemoryGraphStore::from_snapshot(writer.graph_snapshot())?;
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
        let committed_snapshot = InMemoryGraphStore::from_snapshot(writer.graph_snapshot())?;
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
}

impl McpGraphProvider for AppState {
    type Backend = TenantGraphStore;

    fn backend_for_tenant(&self, tenant: &str) -> Result<Self::Backend, McpError> {
        self.tenant_graph_store(tenant)
            .map_err(|error| McpError::internal(error.message))
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, StorageMode};

    use super::{AppState, RedCoreTenantExecutor};
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use thg_core::{
        EdgeRecord, GraphMutation, GraphMutationBatch, NeighborQuery, NodeRecord,
        RedCoreDurability, RedCoreGraphStore,
    };

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
        });

        assert_eq!(
            state.tenant_state_key("Tenant.One!"),
            "rusty-red:TenantOne:state:v1"
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
            redis_url: "not-a-redis-url".to_string(),
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
            redis_url: "not-a-redis-url".to_string(),
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
            redis_url: "not-a-redis-url".to_string(),
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
        assert_eq!(executor.verify().unwrap().ok, true);
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
            redis_url: "not-a-redis-url".to_string(),
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

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{label}-{unique}"))
    }
}
