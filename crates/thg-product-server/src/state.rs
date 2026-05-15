use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use serde_json::json;
use thg_core::store::RedisThgStore;
use thg_core::{
    sanitize_tenant_segment, EdgeRecord, GraphMutation, GraphMutationBatch, GraphRebuildReport,
    GraphStats, GraphStoreError, GraphStoreResult, GraphTransaction, GraphWriteResult,
    InMemoryGraphStore, NeighborHit, NeighborQuery, NodeQuery, NodeRecord, RedCoreGraphStore,
    RedCoreOptions, RedisGraphStore, VerifyReport,
};
use thg_mcp::{McpError, McpGraphBackend, McpGraphProvider, McpServerConfig};

use crate::config::{Config, StorageMode};
use crate::graph_cache::GraphCacheTenant;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    redcore_stores: Arc<Mutex<BTreeMap<String, Arc<RedCoreTenantExecutor>>>>,
    graph_caches: Arc<Mutex<BTreeMap<String, Arc<GraphCacheTenant>>>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            redcore_stores: Arc::new(Mutex::new(BTreeMap::new())),
            graph_caches: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn tenant_store(&self, tenant_id: &str) -> Result<RedisThgStore, StoreAccessError> {
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
        match self.config.storage_mode {
            StorageMode::Embedded => {
                self.config.validate().map_err(StoreAccessError::internal)?;
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
        let store = Arc::new(RedCoreTenantExecutor::new(RedCoreGraphStore::open(
            data_dir, options,
        )?)?);
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
        let store = Arc::new(RedCoreTenantExecutor::new(RedCoreGraphStore::memory())?);
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
}

impl RedCoreTenantExecutor {
    fn new(store: RedCoreGraphStore) -> GraphStoreResult<Self> {
        let committed_snapshot = InMemoryGraphStore::from_snapshot(store.graph_snapshot())?;
        Ok(Self {
            writer: Mutex::new(store),
            committed_snapshot: RwLock::new(committed_snapshot),
        })
    }

    pub fn commit_batch(&self, batch: GraphMutationBatch) -> GraphStoreResult<GraphTransaction> {
        let mut writer = self.lock_writer()?;
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
        self.with_snapshot(|snapshot| snapshot.stats())
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
        let executor = Arc::new(RedCoreTenantExecutor::new(RedCoreGraphStore::memory()).unwrap());
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
        let executor = RedCoreTenantExecutor::new(RedCoreGraphStore::memory()).unwrap();
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

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{label}-{unique}"))
    }
}
