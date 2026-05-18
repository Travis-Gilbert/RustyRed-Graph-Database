use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use instant_distance::{Builder, HnswMap, Search};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::stable_hash;

#[derive(Clone, Debug)]
pub struct VectorPoint(Vec<f32>);

impl VectorPoint {
    pub fn new(raw: &[f32]) -> Self {
        let norm = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm < 1e-10 {
            Self(raw.to_vec())
        } else {
            Self(raw.iter().map(|x| x / norm).collect())
        }
    }
}

impl instant_distance::Point for VectorPoint {
    fn distance(&self, other: &Self) -> f32 {
        let dot: f32 = self.0.iter().zip(other.0.iter()).map(|(a, b)| a * b).sum();
        1.0 - dot
    }
}

pub struct VectorIndex {
    points: Vec<VectorPoint>,
    node_ids: Vec<String>,
    hnsw: Option<HnswMap<VectorPoint, String>>,
    pub dimension: usize,
}

impl Clone for VectorIndex {
    fn clone(&self) -> Self {
        let mut cloned = Self {
            points: self.points.clone(),
            node_ids: self.node_ids.clone(),
            hnsw: None,
            dimension: self.dimension,
        };
        cloned.rebuild();
        cloned
    }
}

impl std::fmt::Debug for VectorIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorIndex")
            .field("dimension", &self.dimension)
            .field("count", &self.points.len())
            .finish()
    }
}

impl VectorIndex {
    fn new(dimension: usize) -> Self {
        Self {
            points: Vec::new(),
            node_ids: Vec::new(),
            hnsw: None,
            dimension,
        }
    }

    fn insert(&mut self, node_id: &str, vector: &[f32]) {
        if let Some(pos) = self.node_ids.iter().position(|id| id == node_id) {
            self.points[pos] = VectorPoint::new(vector);
        } else {
            self.points.push(VectorPoint::new(vector));
            self.node_ids.push(node_id.to_string());
        }
        self.rebuild();
    }

    fn rebuild(&mut self) {
        if self.points.is_empty() {
            self.hnsw = None;
            return;
        }
        let hnsw = Builder::default().build(self.points.clone(), self.node_ids.clone());
        self.hnsw = Some(hnsw);
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<(String, f32)> {
        let Some(ref hnsw) = self.hnsw else {
            return Vec::new();
        };
        let point = VectorPoint::new(query);
        let mut search = Search::default();
        hnsw.search(&point, &mut search)
            .take(k)
            .map(|item| (item.value.clone(), item.distance))
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VectorDesignation {
    pub label: String,
    pub property: String,
    pub dimension: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HybridScoringConfig {
    pub alpha: f32,
    #[serde(default = "default_confidence_weighted_graph_distance")]
    pub confidence_weighted_graph_distance: bool,
    #[serde(default = "default_hybrid_edge_type_weights")]
    pub edge_type_weights: BTreeMap<String, f32>,
}

impl Default for HybridScoringConfig {
    fn default() -> Self {
        Self {
            alpha: 0.5,
            confidence_weighted_graph_distance: true,
            edge_type_weights: default_hybrid_edge_type_weights(),
        }
    }
}

impl HybridScoringConfig {
    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    fn edge_type_weight(&self, edge_type: &str) -> f32 {
        self.edge_type_weights
            .get(edge_type)
            .or_else(|| self.edge_type_weights.get(&edge_type.to_ascii_lowercase()))
            .copied()
            .unwrap_or(1.0)
    }
}

fn default_confidence_weighted_graph_distance() -> bool {
    true
}

pub fn default_hybrid_edge_type_weights() -> BTreeMap<String, f32> {
    BTreeMap::from([
        ("contradicts".to_string(), -1.0),
        ("CONTRADICTS".to_string(), -1.0),
        ("tension".to_string(), -0.5),
        ("TENSION".to_string(), -0.5),
    ])
}

pub type GraphStoreResult<T> = Result<T, GraphStoreError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicType {
    Supports,
    Contradicts,
    Tension,
    Derives,
    Cites,
}

impl std::fmt::Display for EpistemicType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Supports => write!(f, "supports"),
            Self::Contradicts => write!(f, "contradicts"),
            Self::Tension => write!(f, "tension"),
            Self::Derives => write!(f, "derives"),
            Self::Cites => write!(f, "cites"),
        }
    }
}

impl std::str::FromStr for EpistemicType {
    type Err = GraphStoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "supports" => Ok(Self::Supports),
            "contradicts" => Ok(Self::Contradicts),
            "tension" => Ok(Self::Tension),
            "derives" => Ok(Self::Derives),
            "cites" => Ok(Self::Cites),
            _ => Err(GraphStoreError::new(
                "invalid_epistemic_type",
                format!("unknown epistemic type: {s}"),
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

pub trait GraphStore {
    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<GraphWriteResult>;
    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult>;
    fn get_node(&self, id: &str) -> Option<&NodeRecord>;
    fn get_edge(&self, id: &str) -> Option<&EdgeRecord>;
    fn query_nodes(&self, query: NodeQuery) -> Vec<NodeRecord>;
    fn neighbors(&self, query: NeighborQuery) -> Vec<NeighborHit>;
    fn stats(&self) -> GraphStats;
    fn verify(&self) -> VerifyReport;
    fn rebuild_indexes(&mut self) -> GraphStoreResult<GraphRebuildReport>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphStoreError {
    pub code: String,
    pub message: String,
}

impl GraphStoreError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn empty_field(field: &str) -> Self {
        Self::new("empty_graph_field", format!("{field} is required"))
    }

    fn missing_endpoint(edge_id: &str, endpoint: &str, node_id: &str) -> Self {
        Self::new(
            "missing_graph_endpoint",
            format!("edge {edge_id} {endpoint} endpoint {node_id} does not exist"),
        )
    }

    fn empty_transaction() -> Self {
        Self::new(
            "empty_graph_transaction",
            "graph transaction requires at least one mutation",
        )
    }

    fn io(action: impl AsRef<str>, err: impl std::fmt::Display) -> Self {
        Self::new("redcore_io_error", format!("{}: {err}", action.as_ref()))
    }

    fn tombstoned_endpoint(edge_id: &str, endpoint: &str, node_id: &str) -> Self {
        Self::new(
            "tombstoned_graph_endpoint",
            format!("edge {edge_id} {endpoint} endpoint {node_id} is tombstoned"),
        )
    }

    #[cfg(feature = "redis-store")]
    fn invalid_record(record_type: &str, id: &str, err: impl std::fmt::Display) -> Self {
        Self::new(
            "invalid_graph_record",
            format!("{record_type} {id} could not be decoded: {err}"),
        )
    }
}

#[cfg(feature = "redis-store")]
impl From<redis::RedisError> for GraphStoreError {
    fn from(err: redis::RedisError) -> Self {
        Self::new("redis_graph_store_error", err.to_string())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NodeRecord {
    pub id: String,
    pub labels: Vec<String>,
    pub properties: Value,
    pub version: u64,
    pub tombstone: bool,
}

impl NodeRecord {
    pub fn new(
        id: impl Into<String>,
        labels: impl IntoIterator<Item = impl Into<String>>,
        properties: Value,
    ) -> Self {
        Self {
            id: id.into(),
            labels: normalize_labels(labels),
            properties,
            version: 0,
            tombstone: false,
        }
    }

    pub fn checksum(&self) -> String {
        stable_hash(self)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EdgeRecord {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub properties: Value,
    pub version: u64,
    pub tombstone: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epistemic_type: Option<EpistemicType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

impl EdgeRecord {
    pub fn new(
        id: impl Into<String>,
        from_id: impl Into<String>,
        edge_type: impl Into<String>,
        to_id: impl Into<String>,
        properties: Value,
    ) -> Self {
        Self {
            id: id.into(),
            from_id: from_id.into(),
            to_id: to_id.into(),
            edge_type: edge_type.into(),
            properties,
            version: 0,
            tombstone: false,
            confidence: None,
            epistemic_type: None,
            provenance: None,
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence.clamp(0.0, 1.0));
        self
    }

    pub fn with_epistemic_type(mut self, epistemic_type: EpistemicType) -> Self {
        self.epistemic_type = Some(epistemic_type);
        self
    }

    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = Some(provenance);
        self
    }

    pub fn effective_confidence(&self) -> f64 {
        self.confidence.unwrap_or(1.0)
    }

    pub fn checksum(&self) -> String {
        stable_hash(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Out,
    In,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NeighborQuery {
    pub node_id: String,
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_type: Option<String>,
}

impl NeighborQuery {
    pub fn out(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            direction: Direction::Out,
            edge_type: None,
        }
    }

    pub fn in_(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            direction: Direction::In,
            edge_type: None,
        }
    }

    pub fn with_edge_type(mut self, edge_type: impl Into<String>) -> Self {
        let edge_type = edge_type.into();
        if !edge_type.trim().is_empty() {
            self.edge_type = Some(edge_type);
        }
        self
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NeighborHit {
    pub edge_id: String,
    pub node_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epistemic_type: Option<EpistemicType>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct NodeQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl NodeQuery {
    pub fn label(label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..Self::default()
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        if limit > 0 {
            self.limit = Some(limit);
        }
        self
    }

    fn normalized_label(&self) -> Option<String> {
        self.label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_string)
    }

    fn bounded_limit(&self) -> usize {
        self.limit.filter(|limit| *limit > 0).unwrap_or(100)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphWriteResult {
    pub id: String,
    pub version: u64,
    pub checksum: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "op", content = "record", rename_all = "snake_case")]
pub enum GraphMutation {
    NodeUpsert(NodeRecord),
    EdgeUpsert(EdgeRecord),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GraphMutationBatch {
    pub mutations: Vec<GraphMutation>,
}

impl GraphMutationBatch {
    pub fn new(mutations: impl IntoIterator<Item = GraphMutation>) -> Self {
        Self {
            mutations: mutations.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphTransaction {
    pub txn_id: u64,
    pub graph_version: u64,
    pub writes: Vec<GraphWriteResult>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphStats {
    pub version: u64,
    pub nodes_total: usize,
    pub edges_total: usize,
    pub labels_total: usize,
    pub edge_types_total: usize,
    pub property_keys_total: usize,
    pub property_indexes_total: usize,
    /// Estimated serialized footprint for live graph records and derived indexes.
    pub memory_bytes: usize,
    pub memory_quota_bytes: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GraphSnapshot {
    pub version: u64,
    pub nodes: Vec<NodeRecord>,
    pub edges: Vec<EdgeRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifyProblem {
    pub kind: String,
    pub id: String,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub stats: GraphStats,
    pub problems: Vec<VerifyProblem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphRebuildReport {
    pub repaired: bool,
    pub before: VerifyReport,
    pub after: VerifyReport,
}

#[derive(Clone, Debug)]
pub struct InMemoryGraphStore {
    version: u64,
    nodes: BTreeMap<String, NodeRecord>,
    edges: BTreeMap<String, EdgeRecord>,
    out_adjacency: BTreeMap<(String, String), BTreeSet<String>>,
    in_adjacency: BTreeMap<(String, String), BTreeSet<String>>,
    label_index: BTreeMap<String, BTreeSet<String>>,
    edge_type_index: BTreeMap<String, BTreeSet<String>>,
    property_index: BTreeMap<(String, String), BTreeSet<String>>,
    vector_designations: HashMap<(String, String), usize>,
    vector_indexes: HashMap<(String, String), VectorIndex>,
}

impl Default for InMemoryGraphStore {
    fn default() -> Self {
        Self {
            version: 0,
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            out_adjacency: BTreeMap::new(),
            in_adjacency: BTreeMap::new(),
            label_index: BTreeMap::new(),
            edge_type_index: BTreeMap::new(),
            property_index: BTreeMap::new(),
            vector_designations: HashMap::new(),
            vector_indexes: HashMap::new(),
        }
    }
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> GraphSnapshot {
        GraphSnapshot {
            version: self.version,
            nodes: self.nodes.values().cloned().collect(),
            edges: self.edges.values().cloned().collect(),
        }
    }

    pub fn from_snapshot(snapshot: GraphSnapshot) -> GraphStoreResult<Self> {
        let mut store = Self::new();
        store.version = snapshot.version;
        for mut node in snapshot.nodes {
            node.labels = normalize_labels(node.labels);
            store.apply_recovered_node(node)?;
        }
        for edge in snapshot.edges {
            store.apply_recovered_edge(edge)?;
        }
        store.version = store.version.max(snapshot.version);
        Ok(store)
    }

    pub fn upsert_node(&mut self, mut node: NodeRecord) -> GraphStoreResult<GraphWriteResult> {
        if node.id.trim().is_empty() {
            return Err(GraphStoreError::empty_field("node.id"));
        }

        node.labels = normalize_labels(node.labels);
        if let Some(existing) = self.nodes.get(&node.id).cloned() {
            self.remove_node_indexes(&existing);
        }

        self.version += 1;
        node.version = self.version;
        let checksum = node.checksum();
        let id = node.id.clone();
        if !node.tombstone {
            self.add_node_indexes(&node);
            self.auto_index_vectors(&node);
        }
        self.nodes.insert(id.clone(), node);

        Ok(GraphWriteResult {
            id,
            version: self.version,
            checksum,
        })
    }

    fn apply_recovered_node(&mut self, mut node: NodeRecord) -> GraphStoreResult<()> {
        if node.id.trim().is_empty() {
            return Err(GraphStoreError::empty_field("node.id"));
        }
        node.labels = normalize_labels(node.labels);
        if let Some(existing) = self.nodes.get(&node.id).cloned() {
            self.remove_node_indexes(&existing);
        }
        if !node.tombstone {
            self.add_node_indexes(&node);
            self.auto_index_vectors(&node);
        }
        self.version = self.version.max(node.version);
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn upsert_edge(&mut self, mut edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        validate_edge_shape(&edge)?;
        self.require_live_endpoint(&edge, "from", &edge.from_id)?;
        self.require_live_endpoint(&edge, "to", &edge.to_id)?;

        if let Some(existing) = self.edges.get(&edge.id).cloned() {
            self.remove_edge_indexes(&existing);
        }

        self.version += 1;
        edge.version = self.version;
        let checksum = edge.checksum();
        let id = edge.id.clone();
        if !edge.tombstone {
            self.add_edge_indexes(&edge);
        }
        self.edges.insert(id.clone(), edge);

        Ok(GraphWriteResult {
            id,
            version: self.version,
            checksum,
        })
    }

    fn apply_recovered_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        validate_edge_shape(&edge)?;
        if !edge.tombstone {
            self.require_live_endpoint(&edge, "from", &edge.from_id)?;
            self.require_live_endpoint(&edge, "to", &edge.to_id)?;
        }
        if let Some(existing) = self.edges.get(&edge.id).cloned() {
            self.remove_edge_indexes(&existing);
        }
        if !edge.tombstone {
            self.add_edge_indexes(&edge);
        }
        self.version = self.version.max(edge.version);
        self.edges.insert(edge.id.clone(), edge);
        Ok(())
    }

    pub fn get_node(&self, id: &str) -> Option<&NodeRecord> {
        self.nodes.get(id).filter(|node| !node.tombstone)
    }

    pub fn get_edge(&self, id: &str) -> Option<&EdgeRecord> {
        self.edges.get(id).filter(|edge| !edge.tombstone)
    }

    pub fn node_ids_for_label(&self, label: &str) -> Vec<String> {
        sorted_values(self.label_index.get(label))
    }

    pub fn edge_ids_for_type(&self, edge_type: &str) -> Vec<String> {
        sorted_values(self.edge_type_index.get(edge_type))
    }

    pub fn node_ids_for_property(&self, key: &str, value: &Value) -> Vec<String> {
        let Some(token) = property_index_token(value) else {
            return Vec::new();
        };
        sorted_values(self.property_index.get(&(key.to_string(), token)))
    }

    pub fn labels(&self) -> Vec<String> {
        self.label_index.keys().cloned().collect()
    }

    pub fn edge_types(&self) -> Vec<String> {
        self.edge_type_index.keys().cloned().collect()
    }

    pub fn property_keys(&self) -> Vec<String> {
        self.property_index
            .keys()
            .map(|(key, _)| key.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn query_nodes(&self, query: NodeQuery) -> Vec<NodeRecord> {
        let mut candidate_ids: Option<BTreeSet<String>> = None;
        if let Some(label) = query.normalized_label() {
            merge_candidates(&mut candidate_ids, self.label_index.get(&label).cloned());
        }
        for (key, value) in &query.properties {
            let key = key.trim();
            if key.is_empty() {
                return Vec::new();
            }
            let Some(token) = property_index_token(value) else {
                return Vec::new();
            };
            merge_candidates(
                &mut candidate_ids,
                self.property_index.get(&(key.to_string(), token)).cloned(),
            );
        }

        let ids = candidate_ids.unwrap_or_else(|| {
            self.nodes
                .values()
                .filter(|node| !node.tombstone)
                .map(|node| node.id.clone())
                .collect()
        });
        ids.into_iter()
            .filter_map(|id| self.get_node(&id).cloned())
            .take(query.bounded_limit())
            .collect()
    }

    pub fn neighbors(&self, query: NeighborQuery) -> Vec<NeighborHit> {
        let mut edge_ids = BTreeSet::new();
        match query.edge_type {
            Some(edge_type) => {
                let key = (query.node_id.clone(), edge_type);
                let index = match query.direction {
                    Direction::Out => &self.out_adjacency,
                    Direction::In => &self.in_adjacency,
                };
                if let Some(index_edge_ids) = index.get(&key) {
                    edge_ids.extend(index_edge_ids.iter().cloned());
                }
            }
            None => {
                let index = match query.direction {
                    Direction::Out => &self.out_adjacency,
                    Direction::In => &self.in_adjacency,
                };
                for ((node_id, _edge_type), index_edge_ids) in index {
                    if node_id == &query.node_id {
                        edge_ids.extend(index_edge_ids.iter().cloned());
                    }
                }
            }
        }

        let mut hits = Vec::new();
        for edge_id in edge_ids {
            let Some(edge) = self.get_edge(&edge_id) else {
                continue;
            };
            let node_id = match query.direction {
                Direction::Out => edge.to_id.clone(),
                Direction::In => edge.from_id.clone(),
            };
            if self.get_node(&node_id).is_none() {
                continue;
            }
            hits.push(NeighborHit {
                edge_id: edge.id.clone(),
                node_id,
                edge_type: edge.edge_type.clone(),
                confidence: edge.confidence,
                epistemic_type: edge.epistemic_type.clone(),
            });
        }
        hits
    }

    pub fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> Vec<(EdgeRecord, NodeRecord)> {
        let max_depth = max_depth.unwrap_or(1);
        let min_conf = min_confidence.unwrap_or(0.0);
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<(String, usize)> =
            std::collections::VecDeque::new();
        let mut results: Vec<(EdgeRecord, NodeRecord)> = Vec::new();

        visited.insert(node_id.to_string());
        queue.push_back((node_id.to_string(), 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for hits in [
                self.neighbors(NeighborQuery::out(&current_id)),
                self.neighbors(NeighborQuery::in_(&current_id)),
            ] {
                for hit in hits {
                    if visited.contains(&hit.node_id) {
                        continue;
                    }
                    let Some(edge) = self.get_edge(&hit.edge_id) else {
                        continue;
                    };
                    if edge.effective_confidence() < min_conf {
                        continue;
                    }
                    if let Some(types) = epistemic_types {
                        match &edge.epistemic_type {
                            Some(et) if types.contains(et) => {}
                            _ => continue,
                        }
                    }
                    let Some(node) = self.get_node(&hit.node_id) else {
                        continue;
                    };
                    visited.insert(hit.node_id.clone());
                    results.push((edge.clone(), node.clone()));
                    queue.push_back((hit.node_id, depth + 1));
                }
            }
        }
        results
    }

    pub fn stats(&self) -> GraphStats {
        GraphStats {
            version: self.version,
            nodes_total: self.nodes.values().filter(|node| !node.tombstone).count(),
            edges_total: self.edges.values().filter(|edge| !edge.tombstone).count(),
            labels_total: self.label_index.len(),
            edge_types_total: self.edge_type_index.len(),
            property_keys_total: self.property_keys().len(),
            property_indexes_total: self.property_index.len(),
            memory_bytes: self.estimated_memory_bytes(),
            memory_quota_bytes: 0,
        }
    }

    pub fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        if label.trim().is_empty() || property_name.trim().is_empty() {
            return Err(GraphStoreError::new(
                "invalid_vector_designation",
                "label and property_name must be non-empty".to_string(),
            ));
        }
        if dimension == 0 {
            return Err(GraphStoreError::new(
                "invalid_vector_designation",
                "dimension must be > 0".to_string(),
            ));
        }
        let key = (label.to_string(), property_name.to_string());
        self.vector_designations.insert(key.clone(), dimension);
        self.vector_indexes
            .entry(key)
            .or_insert_with(|| VectorIndex::new(dimension));
        for node in self.nodes.values() {
            if node.tombstone {
                continue;
            }
            if !node.labels.iter().any(|l| l == label) {
                continue;
            }
            if let Some(arr) = extract_float_array(&node.properties, property_name) {
                if arr.len() == dimension {
                    let idx_key = (label.to_string(), property_name.to_string());
                    if let Some(idx) = self.vector_indexes.get_mut(&idx_key) {
                        idx.insert(&node.id, &arr);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn vector_designations(&self) -> Vec<VectorDesignation> {
        self.vector_designations
            .iter()
            .map(|((label, property), &dimension)| VectorDesignation {
                label: label.clone(),
                property: property.clone(),
                dimension,
            })
            .collect()
    }

    pub fn index_vector(
        &mut self,
        node_id: &str,
        property_name: &str,
        vector: &[f32],
    ) -> GraphStoreResult<()> {
        let node = self.nodes.get(node_id).ok_or_else(|| {
            GraphStoreError::new("node_not_found", format!("no node with id {node_id}"))
        })?;
        let matching_label = node.labels.iter().find(|label| {
            self.vector_designations
                .contains_key(&(label.to_string(), property_name.to_string()))
        });
        let label = matching_label
            .ok_or_else(|| {
                GraphStoreError::new(
                    "no_vector_designation",
                    format!(
                        "no vector designation covers property {property_name} for node {node_id}"
                    ),
                )
            })?
            .clone();
        let key = (label, property_name.to_string());
        let expected_dim = self.vector_designations[&key];
        if vector.len() != expected_dim {
            return Err(GraphStoreError::new(
                "dimension_mismatch",
                format!("expected {expected_dim} dimensions, got {}", vector.len()),
            ));
        }
        let idx = self
            .vector_indexes
            .entry(key)
            .or_insert_with(|| VectorIndex::new(expected_dim));
        idx.insert(node_id, vector);
        Ok(())
    }

    pub fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        for ((l, p), idx) in &self.vector_indexes {
            if p != property_name {
                continue;
            }
            if let Some(lbl) = label {
                if l != lbl {
                    continue;
                }
            }
            if query.len() != idx.dimension {
                return Err(GraphStoreError::new(
                    "dimension_mismatch",
                    format!(
                        "query dimension {} does not match index dimension {}",
                        query.len(),
                        idx.dimension
                    ),
                ));
            }
        }
        let mut results = Vec::new();
        for ((l, p), idx) in &self.vector_indexes {
            if p != property_name {
                continue;
            }
            if let Some(label) = label {
                if l != label {
                    continue;
                }
            }
            results.extend(idx.search(query, k));
        }
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        Ok(results)
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
        self.hybrid_search_with_config(
            label,
            property_name,
            query,
            k,
            graph_seeds,
            max_hops,
            &HybridScoringConfig::default().with_alpha(alpha),
        )
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
        let vector_results = self.vector_search(label, property_name, query, k * 2)?;
        if vector_results.is_empty() {
            return Ok(Vec::new());
        }
        let graph_scores = self.hybrid_graph_scores(graph_seeds, max_hops, config);
        let max_vec_dist = vector_results
            .iter()
            .map(|(_, d)| *d)
            .fold(0.0_f32, f32::max)
            .max(1e-10);
        let alpha = config.alpha.clamp(0.0, 1.0);
        let mut scored: Vec<(String, f32)> = vector_results
            .into_iter()
            .map(|(node_id, vec_dist)| {
                let vector_score = 1.0 - (vec_dist / max_vec_dist);
                let graph_score = graph_scores.get(&node_id).copied().unwrap_or(0.0);
                let final_score = (1.0 - alpha) * vector_score + alpha * graph_score;
                (node_id, final_score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        Ok(scored)
    }

    fn hybrid_graph_scores(
        &self,
        graph_seeds: &[String],
        max_hops: usize,
        config: &HybridScoringConfig,
    ) -> HashMap<String, f32> {
        let mut adjacency: HashMap<String, Vec<&EdgeRecord>> = HashMap::new();
        for edge in self.edges.values().filter(|edge| !edge.tombstone) {
            adjacency
                .entry(edge.from_id.clone())
                .or_default()
                .push(edge);
        }
        let mut best: HashMap<String, f32> = HashMap::new();
        let mut queue: VecDeque<(String, usize, f32)> = VecDeque::new();
        for seed in graph_seeds {
            best.insert(seed.clone(), 1.0);
            queue.push_back((seed.clone(), 0, 1.0));
        }
        while let Some((node_id, depth, path_score)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            let Some(edges) = adjacency.get(&node_id) else {
                continue;
            };
            for edge in edges {
                let confidence = if config.confidence_weighted_graph_distance {
                    edge.effective_confidence() as f32
                } else {
                    1.0
                };
                let edge_weight = config.edge_type_weight(&edge.edge_type);
                let next_depth = depth + 1;
                let next_score = path_score * edge_weight * confidence / (1.0 + next_depth as f32);
                let should_update = best
                    .get(&edge.to_id)
                    .map(|current| next_score.abs() > current.abs())
                    .unwrap_or(true);
                if should_update {
                    best.insert(edge.to_id.clone(), next_score.clamp(-1.0, 1.0));
                    queue.push_back((edge.to_id.clone(), next_depth, next_score));
                }
            }
        }
        best
    }

    fn auto_index_vectors(&mut self, node: &NodeRecord) {
        let designations: Vec<((String, String), usize)> = self
            .vector_designations
            .iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        for ((label, property), dimension) in designations {
            if !node.labels.iter().any(|l| l == &label) {
                continue;
            }
            if let Some(arr) = extract_float_array(&node.properties, &property) {
                if arr.len() == dimension {
                    let key = (label, property);
                    let idx = self
                        .vector_indexes
                        .entry(key)
                        .or_insert_with(|| VectorIndex::new(dimension));
                    idx.insert(&node.id, &arr);
                }
            }
        }
    }

    fn estimated_memory_bytes(&self) -> usize {
        let snapshot_bytes = serde_json::to_vec(&self.snapshot())
            .map(|raw| raw.len())
            .unwrap_or_default();
        snapshot_bytes
            + string_index_bytes(&self.label_index)
            + string_index_bytes(&self.edge_type_index)
            + tuple_index_bytes(&self.out_adjacency)
            + tuple_index_bytes(&self.in_adjacency)
            + tuple_index_bytes(&self.property_index)
    }

    pub fn verify(&self) -> VerifyReport {
        let mut expected = ExpectedIndexes::default();
        let mut problems = Vec::new();

        for node in self.nodes.values().filter(|node| !node.tombstone) {
            for label in &node.labels {
                expected
                    .label_index
                    .entry(label.clone())
                    .or_default()
                    .insert(node.id.clone());
            }
            for (key, token) in indexed_properties(&node.properties) {
                expected
                    .property_index
                    .entry((key, token))
                    .or_default()
                    .insert(node.id.clone());
            }
        }

        for edge in self.edges.values().filter(|edge| !edge.tombstone) {
            if self.get_node(&edge.from_id).is_none() {
                problems.push(VerifyProblem {
                    kind: "missing_from_endpoint".to_string(),
                    id: edge.id.clone(),
                    detail: format!("from endpoint {} is not a live node", edge.from_id),
                });
            }
            if self.get_node(&edge.to_id).is_none() {
                problems.push(VerifyProblem {
                    kind: "missing_to_endpoint".to_string(),
                    id: edge.id.clone(),
                    detail: format!("to endpoint {} is not a live node", edge.to_id),
                });
            }
            expected
                .edge_type_index
                .entry(edge.edge_type.clone())
                .or_default()
                .insert(edge.id.clone());
            expected
                .out_adjacency
                .entry((edge.from_id.clone(), edge.edge_type.clone()))
                .or_default()
                .insert(edge.id.clone());
            expected
                .in_adjacency
                .entry((edge.to_id.clone(), edge.edge_type.clone()))
                .or_default()
                .insert(edge.id.clone());
        }

        if self.label_index != expected.label_index {
            problems.push(VerifyProblem {
                kind: "label_index_drift".to_string(),
                id: "label_index".to_string(),
                detail: "label index does not match live node labels".to_string(),
            });
        }
        if self.edge_type_index != expected.edge_type_index {
            problems.push(VerifyProblem {
                kind: "edge_type_index_drift".to_string(),
                id: "edge_type_index".to_string(),
                detail: "edge type index does not match live edge types".to_string(),
            });
        }
        if self.property_index != expected.property_index {
            problems.push(VerifyProblem {
                kind: "property_index_drift".to_string(),
                id: "property_index".to_string(),
                detail: "property index does not match live scalar node properties".to_string(),
            });
        }
        if self.out_adjacency != expected.out_adjacency {
            problems.push(VerifyProblem {
                kind: "out_adjacency_drift".to_string(),
                id: "out_adjacency".to_string(),
                detail: "out adjacency index does not match live edges".to_string(),
            });
        }
        if self.in_adjacency != expected.in_adjacency {
            problems.push(VerifyProblem {
                kind: "in_adjacency_drift".to_string(),
                id: "in_adjacency".to_string(),
                detail: "in adjacency index does not match live edges".to_string(),
            });
        }

        VerifyReport {
            ok: problems.is_empty(),
            stats: self.stats(),
            problems,
        }
    }

    pub fn rebuild_indexes(&mut self) -> GraphStoreResult<GraphRebuildReport> {
        let before = self.verify();
        self.rebuild_indexes_from_records();
        let after = self.verify();
        Ok(GraphRebuildReport {
            repaired: !before.ok && after.ok,
            before,
            after,
        })
    }

    fn rebuild_indexes_from_records(&mut self) {
        self.out_adjacency.clear();
        self.in_adjacency.clear();
        self.label_index.clear();
        self.edge_type_index.clear();
        self.property_index.clear();
        let nodes = self.nodes.values().cloned().collect::<Vec<_>>();
        let edges = self.edges.values().cloned().collect::<Vec<_>>();
        for node in nodes {
            if !node.tombstone {
                self.add_node_indexes(&node);
            }
        }
        for edge in edges {
            if !edge.tombstone {
                self.add_edge_indexes(&edge);
            }
        }
    }

    fn require_live_endpoint(
        &self,
        edge: &EdgeRecord,
        endpoint: &str,
        node_id: &str,
    ) -> GraphStoreResult<()> {
        let Some(node) = self.nodes.get(node_id) else {
            return Err(GraphStoreError::missing_endpoint(
                &edge.id, endpoint, node_id,
            ));
        };
        if node.tombstone {
            return Err(GraphStoreError::tombstoned_endpoint(
                &edge.id, endpoint, node_id,
            ));
        }
        Ok(())
    }

    fn add_node_indexes(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.label_index
                .entry(label.clone())
                .or_default()
                .insert(node.id.clone());
        }
        for (key, token) in indexed_properties(&node.properties) {
            self.property_index
                .entry((key, token))
                .or_default()
                .insert(node.id.clone());
        }
    }

    fn remove_node_indexes(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            remove_index_value(&mut self.label_index, label, &node.id);
        }
        for key in indexed_properties(&node.properties).into_keys() {
            let entries = self
                .property_index
                .keys()
                .filter(|(property_key, _)| property_key == &key)
                .cloned()
                .collect::<Vec<_>>();
            for entry in entries {
                remove_index_value(&mut self.property_index, &entry, &node.id);
            }
        }
    }

    fn add_edge_indexes(&mut self, edge: &EdgeRecord) {
        self.edge_type_index
            .entry(edge.edge_type.clone())
            .or_default()
            .insert(edge.id.clone());
        self.out_adjacency
            .entry((edge.from_id.clone(), edge.edge_type.clone()))
            .or_default()
            .insert(edge.id.clone());
        self.in_adjacency
            .entry((edge.to_id.clone(), edge.edge_type.clone()))
            .or_default()
            .insert(edge.id.clone());
    }

    fn remove_edge_indexes(&mut self, edge: &EdgeRecord) {
        remove_index_value(&mut self.edge_type_index, &edge.edge_type, &edge.id);
        remove_index_value(
            &mut self.out_adjacency,
            &(edge.from_id.clone(), edge.edge_type.clone()),
            &edge.id,
        );
        remove_index_value(
            &mut self.in_adjacency,
            &(edge.to_id.clone(), edge.edge_type.clone()),
            &edge.id,
        );
    }
}

const REDCORE_AOF_MAGIC: &str = "RRGDB_AOF";
const REDCORE_MANIFEST_VERSION: u32 = 1;
const REDCORE_SNAPSHOT_FILE: &str = "graph.snapshot.current";
const REDCORE_PREVIOUS_SNAPSHOT_FILE: &str = "graph.snapshot.previous";
const REDCORE_AOF_FILE: &str = "graph.aof.current";
const REDCORE_MANIFEST_FILE: &str = "manifest.json";
const REDCORE_LOCK_FILE: &str = ".redcore.lock";
const REDCORE_CURRENT_SNAPSHOT_TMP_FILE: &str = "graph.snapshot.current.tmp";
const REDCORE_PREVIOUS_SNAPSHOT_TMP_FILE: &str = "graph.snapshot.previous.tmp";
const REDCORE_MANIFEST_TMP_FILE: &str = "manifest.json.tmp";

static REDCORE_PROCESS_LOCKS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedCoreDurability {
    None,
    AofEverysec,
    AofAlways,
    SnapshotOnly,
}

impl RedCoreDurability {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "memory" => Self::None,
            "aof_always" | "always" => Self::AofAlways,
            "snapshot_only" | "snapshot" => Self::SnapshotOnly,
            _ => Self::AofEverysec,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AofEverysec => "aof_everysec",
            Self::AofAlways => "aof_always",
            Self::SnapshotOnly => "snapshot_only",
        }
    }

    fn uses_aof(self) -> bool {
        matches!(self, Self::AofEverysec | Self::AofAlways)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RedCoreOptions {
    pub durability: RedCoreDurability,
    pub snapshot_interval_writes: u64,
    pub strict_acid: bool,
}

impl Default for RedCoreOptions {
    fn default() -> Self {
        Self {
            durability: RedCoreDurability::AofEverysec,
            snapshot_interval_writes: 1_000,
            strict_acid: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RedCoreStatus {
    pub mode: String,
    pub durability: String,
    pub data_dir: Option<String>,
    pub graph_version: u64,
    pub last_txn_id: u64,
    pub snapshot_txn_id: u64,
    pub recovered_frames: u64,
    pub last_recovery_ok: bool,
    pub strict_acid: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RedCoreManifest {
    pub version: u32,
    pub graph_version: u64,
    pub last_txn_id: u64,
    pub snapshot_txn_id: u64,
    pub durability: RedCoreDurability,
    pub snapshot_file: String,
    pub aof_file: String,
    pub updated_at_unix_ms: u128,
    #[serde(default = "default_format_kind")]
    pub format_kind: String,
    #[serde(default)]
    pub crate_version: String,
    #[serde(default)]
    pub created_at_unix_ms: u128,
}

fn default_format_kind() -> String {
    "redcore".to_string()
}

/// Maximum manifest format version this build is willing to load.
/// Bump when introducing a breaking on-disk format change.
pub const CURRENT_FORMAT_VERSION: u32 = REDCORE_MANIFEST_VERSION;

/// Returns true if a snapshot at `version` can be loaded directly without migration.
pub fn manifest_version_compatible(version: u32) -> bool {
    version <= CURRENT_FORMAT_VERSION
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct RedCoreSnapshotEnvelope {
    version: u32,
    txn_id: u64,
    graph: GraphSnapshot,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct RedCoreAofFrame {
    magic: String,
    version: u32,
    txn_id: u64,
    graph_version: u64,
    timestamp_unix_ms: u128,
    payload_checksum: String,
    mutation: RedCoreMutation,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "op", content = "record", rename_all = "snake_case")]
enum RedCoreMutation {
    NodeUpsert(NodeRecord),
    EdgeUpsert(EdgeRecord),
    Batch(Vec<RedCoreMutation>),
    VectorDesignation(VectorDesignation),
}

impl From<GraphMutation> for RedCoreMutation {
    fn from(mutation: GraphMutation) -> Self {
        match mutation {
            GraphMutation::NodeUpsert(node) => Self::NodeUpsert(node),
            GraphMutation::EdgeUpsert(edge) => Self::EdgeUpsert(edge),
        }
    }
}

#[derive(Debug)]
pub struct RedCoreGraphStore {
    store: InMemoryGraphStore,
    data_dir: Option<PathBuf>,
    _directory_lock: Option<RedCoreDirectoryLock>,
    options: RedCoreOptions,
    last_txn_id: u64,
    snapshot_txn_id: u64,
    recovered_frames: u64,
    last_recovery_ok: bool,
    last_fsync: Option<SystemTime>,
}

#[derive(Debug)]
struct RedCoreDirectoryLock {
    file: File,
    process_key: PathBuf,
}

impl RedCoreGraphStore {
    pub fn memory() -> Self {
        Self {
            store: InMemoryGraphStore::new(),
            data_dir: None,
            _directory_lock: None,
            options: RedCoreOptions {
                durability: RedCoreDurability::None,
                snapshot_interval_writes: 0,
                strict_acid: false,
            },
            last_txn_id: 0,
            snapshot_txn_id: 0,
            recovered_frames: 0,
            last_recovery_ok: true,
            last_fsync: None,
        }
    }

    pub fn open(data_dir: impl Into<PathBuf>, options: RedCoreOptions) -> GraphStoreResult<Self> {
        validate_redcore_options(&options)?;
        let data_dir = data_dir.into();
        fs::create_dir_all(&data_dir)
            .map_err(|err| GraphStoreError::io("create RedCore data directory", err))?;
        let directory_lock = acquire_redcore_directory_lock(&data_dir)?;
        let mut engine = Self {
            store: InMemoryGraphStore::new(),
            data_dir: Some(data_dir),
            _directory_lock: Some(directory_lock),
            options,
            last_txn_id: 0,
            snapshot_txn_id: 0,
            recovered_frames: 0,
            last_recovery_ok: false,
            last_fsync: None,
        };
        engine.recover()?;
        engine.last_recovery_ok = true;
        engine.write_manifest()?;
        Ok(engine)
    }

    pub fn readiness_check(
        data_dir: &Path,
        durability: RedCoreDurability,
        strict_acid: bool,
    ) -> GraphStoreResult<()> {
        validate_redcore_options(&RedCoreOptions {
            durability,
            snapshot_interval_writes: 1,
            strict_acid,
        })?;
        fs::create_dir_all(data_dir)
            .map_err(|err| GraphStoreError::io("create RedCore data directory", err))?;
        if durability.uses_aof() {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(data_dir.join(REDCORE_AOF_FILE))
                .map_err(|err| GraphStoreError::io("open RedCore AOF for readiness", err))?;
        }
        let probe_path = data_dir.join(".ready-probe");
        fs::write(&probe_path, b"ok")
            .map_err(|err| GraphStoreError::io("write RedCore readiness probe", err))?;
        sync_file_path(&probe_path, "fsync RedCore readiness probe")?;
        fs::remove_file(&probe_path)
            .map_err(|err| GraphStoreError::io("remove RedCore readiness probe", err))?;
        sync_directory(
            data_dir,
            "fsync RedCore data directory after readiness probe",
        )?;
        Ok(())
    }

    pub fn status(&self) -> RedCoreStatus {
        RedCoreStatus {
            mode: if self.data_dir.is_some() {
                "embedded".to_string()
            } else {
                "memory".to_string()
            },
            durability: self.options.durability.as_str().to_string(),
            data_dir: self
                .data_dir
                .as_ref()
                .map(|path| path.display().to_string()),
            graph_version: self.store.stats().version,
            last_txn_id: self.last_txn_id,
            snapshot_txn_id: self.snapshot_txn_id,
            recovered_frames: self.recovered_frames,
            last_recovery_ok: self.last_recovery_ok,
            strict_acid: self.options.strict_acid,
        }
    }

    pub fn graph_snapshot(&self) -> GraphSnapshot {
        self.store.snapshot()
    }

    pub fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<GraphWriteResult> {
        self.commit_batch(GraphMutationBatch::new([GraphMutation::NodeUpsert(node)]))?
            .writes
            .into_iter()
            .next()
            .ok_or_else(|| GraphStoreError::new("redcore_missing_write", "node write vanished"))
    }

    pub fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        self.commit_batch(GraphMutationBatch::new([GraphMutation::EdgeUpsert(edge)]))?
            .writes
            .into_iter()
            .next()
            .ok_or_else(|| GraphStoreError::new("redcore_missing_write", "edge write vanished"))
    }

    pub fn commit_batch(
        &mut self,
        batch: GraphMutationBatch,
    ) -> GraphStoreResult<GraphTransaction> {
        if batch.mutations.is_empty() {
            return Err(GraphStoreError::empty_transaction());
        }

        let mut staged = self.store.clone();
        let mut writes = Vec::with_capacity(batch.mutations.len());
        let mut durable_mutations = Vec::with_capacity(batch.mutations.len());

        for mutation in batch.mutations {
            match mutation {
                GraphMutation::NodeUpsert(node) => {
                    let write = staged.upsert_node(node)?;
                    let record = staged.nodes.get(&write.id).cloned().ok_or_else(|| {
                        GraphStoreError::new("redcore_missing_write", "node write vanished")
                    })?;
                    durable_mutations.push(RedCoreMutation::NodeUpsert(record));
                    writes.push(write);
                }
                GraphMutation::EdgeUpsert(edge) => {
                    let write = staged.upsert_edge(edge)?;
                    let record = staged.edges.get(&write.id).cloned().ok_or_else(|| {
                        GraphStoreError::new("redcore_missing_write", "edge write vanished")
                    })?;
                    durable_mutations.push(RedCoreMutation::EdgeUpsert(record));
                    writes.push(write);
                }
            }
        }

        let txn_id = self.last_txn_id + 1;
        let graph_version = staged.stats().version;
        let durable_mutation = if durable_mutations.len() == 1 {
            durable_mutations
                .pop()
                .expect("single durable mutation exists")
        } else {
            RedCoreMutation::Batch(durable_mutations)
        };
        let prepublished_snapshot_txn_id =
            self.persist_before_publish(txn_id, graph_version, &staged, durable_mutation)?;

        self.store = staged;
        self.last_txn_id = txn_id;
        if let Some(snapshot_txn_id) = prepublished_snapshot_txn_id {
            self.snapshot_txn_id = snapshot_txn_id;
        }

        Ok(GraphTransaction {
            txn_id,
            graph_version,
            writes,
        })
    }

    pub fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        Ok(self.store.get_node(id).cloned())
    }

    pub fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        Ok(self.store.get_edge(id).cloned())
    }

    pub fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        Ok(self.store.query_nodes(query))
    }

    pub fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        Ok(self.store.neighbors(query))
    }

    pub fn stats(&self) -> GraphStoreResult<GraphStats> {
        Ok(self.store.stats())
    }

    pub fn verify(&self) -> GraphStoreResult<VerifyReport> {
        Ok(self.store.verify())
    }

    pub fn rebuild_indexes(&mut self) -> GraphStoreResult<GraphRebuildReport> {
        self.store.rebuild_indexes()
    }

    pub fn labels(&self) -> GraphStoreResult<Vec<String>> {
        Ok(self.store.labels())
    }

    pub fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        Ok(self.store.edge_types())
    }

    pub fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        Ok(self.store.property_keys())
    }

    pub fn designate_vector_property(
        &mut self,
        label: &str,
        property_name: &str,
        dimension: usize,
    ) -> GraphStoreResult<()> {
        self.store
            .designate_vector_property(label, property_name, dimension)?;
        let designation = VectorDesignation {
            label: label.to_string(),
            property: property_name.to_string(),
            dimension,
        };
        let txn_id = self.last_txn_id + 1;
        let staged = self.store.clone();
        let graph_version = staged.stats().version;
        self.persist_before_publish(
            txn_id,
            graph_version,
            &staged,
            RedCoreMutation::VectorDesignation(designation),
        )?;
        self.last_txn_id = txn_id;
        Ok(())
    }

    pub fn vector_search(
        &self,
        label: Option<&str>,
        property_name: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        self.store.vector_search(label, property_name, query, k)
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
        self.store
            .hybrid_search(label, property_name, query, k, graph_seeds, max_hops, alpha)
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

    pub fn vector_designations(&self) -> Vec<VectorDesignation> {
        self.store.vector_designations()
    }

    pub fn epistemic_neighbors(
        &self,
        node_id: &str,
        epistemic_types: Option<&[EpistemicType]>,
        min_confidence: Option<f64>,
        max_depth: Option<usize>,
    ) -> Vec<(EdgeRecord, NodeRecord)> {
        self.store
            .epistemic_neighbors(node_id, epistemic_types, min_confidence, max_depth)
    }

    pub fn snapshot_now(&mut self) -> GraphStoreResult<()> {
        self.write_snapshot()?;
        self.write_manifest()
    }

    fn recover(&mut self) -> GraphStoreResult<()> {
        let Some(data_dir) = self.data_dir.clone() else {
            return Ok(());
        };
        if let Some(manifest) = read_manifest(&data_dir)? {
            if !manifest_version_compatible(manifest.version) {
                return Err(GraphStoreError::new(
                    "redcore_format_too_new",
                    format!(
                        "On-disk RedCore manifest version is {}, this build supports up to {}. \
                         Upgrade the binary or run rustyred-upgrade-format.",
                        manifest.version, CURRENT_FORMAT_VERSION
                    ),
                ));
            }
        }
        if let Some(envelope) = read_latest_valid_snapshot(&data_dir)? {
            self.snapshot_txn_id = envelope.txn_id;
            self.last_txn_id = self.last_txn_id.max(envelope.txn_id);
            self.store = InMemoryGraphStore::from_snapshot(envelope.graph)?;
        }
        self.replay_aof(&data_dir)?;
        Ok(())
    }

    fn replay_aof(&mut self, data_dir: &Path) -> GraphStoreResult<()> {
        let path = data_dir.join(REDCORE_AOF_FILE);
        if !path.exists() {
            return Ok(());
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|err| GraphStoreError::io("open RedCore AOF", err))?;
        let read_file = file
            .try_clone()
            .map_err(|err| GraphStoreError::io("clone RedCore AOF reader", err))?;
        let mut reader = BufReader::new(read_file);
        let mut frame_start = 0_u64;
        loop {
            let mut line = String::new();
            let bytes_read = reader
                .read_line(&mut line)
                .map_err(|err| GraphStoreError::io("read RedCore AOF", err))?;
            if bytes_read == 0 {
                break;
            }
            let frame_end = frame_start + bytes_read as u64;
            if line.trim().is_empty() {
                frame_start = frame_end;
                continue;
            }
            if !line.ends_with('\n') {
                truncate_aof_tail(&file, data_dir, frame_start)?;
                break;
            }
            let raw = line.trim_end_matches(['\r', '\n']);
            let frame = match decode_aof_frame(raw) {
                Ok(frame) => frame,
                Err(_) => {
                    truncate_aof_tail(&file, data_dir, frame_start)?;
                    break;
                }
            };
            if frame.txn_id <= self.snapshot_txn_id {
                frame_start = frame_end;
                continue;
            }
            self.apply_recovered_mutation(frame.mutation)?;
            self.last_txn_id = self.last_txn_id.max(frame.txn_id);
            self.recovered_frames += 1;
            frame_start = frame_end;
        }
        Ok(())
    }

    fn apply_recovered_mutation(&mut self, mutation: RedCoreMutation) -> GraphStoreResult<()> {
        match mutation {
            RedCoreMutation::NodeUpsert(node) => self.store.apply_recovered_node(node),
            RedCoreMutation::EdgeUpsert(edge) => self.store.apply_recovered_edge(edge),
            RedCoreMutation::Batch(mutations) => {
                for mutation in mutations {
                    self.apply_recovered_mutation(mutation)?;
                }
                Ok(())
            }
            RedCoreMutation::VectorDesignation(d) => {
                self.store
                    .designate_vector_property(&d.label, &d.property, d.dimension)
            }
        }
    }

    fn persist_before_publish(
        &mut self,
        txn_id: u64,
        graph_version: u64,
        staged: &InMemoryGraphStore,
        mutation: RedCoreMutation,
    ) -> GraphStoreResult<Option<u64>> {
        let mut snapshot_txn_id = None;
        match self.options.durability {
            RedCoreDurability::None => {}
            RedCoreDurability::SnapshotOnly => {
                self.write_snapshot_for(txn_id, staged)?;
                snapshot_txn_id = Some(txn_id);
            }
            RedCoreDurability::AofEverysec | RedCoreDurability::AofAlways => {
                self.append_aof(txn_id, graph_version, mutation)?;
                if self.should_snapshot_for(txn_id) {
                    self.write_snapshot_for(txn_id, staged)?;
                    snapshot_txn_id = Some(txn_id);
                }
            }
        }
        self.write_manifest_for(
            graph_version,
            txn_id,
            snapshot_txn_id.unwrap_or(self.snapshot_txn_id),
        )?;
        Ok(snapshot_txn_id)
    }

    fn append_aof(
        &mut self,
        txn_id: u64,
        graph_version: u64,
        mutation: RedCoreMutation,
    ) -> GraphStoreResult<()> {
        let Some(data_dir) = self.data_dir.as_ref() else {
            return Ok(());
        };
        fs::create_dir_all(data_dir)
            .map_err(|err| GraphStoreError::io("create RedCore AOF directory", err))?;
        let frame = RedCoreAofFrame {
            magic: REDCORE_AOF_MAGIC.to_string(),
            version: REDCORE_MANIFEST_VERSION,
            txn_id,
            graph_version,
            timestamp_unix_ms: unix_ms(),
            payload_checksum: stable_hash(&mutation),
            mutation,
        };
        let raw = serde_json::to_string(&frame)
            .map_err(|err| GraphStoreError::io("encode RedCore AOF frame", err))?;
        let path = data_dir.join(REDCORE_AOF_FILE);
        let created = !path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| GraphStoreError::io("open RedCore AOF for append", err))?;
        file.write_all(raw.as_bytes())
            .map_err(|err| GraphStoreError::io("append RedCore AOF", err))?;
        file.write_all(b"\n")
            .map_err(|err| GraphStoreError::io("append RedCore AOF frame delimiter", err))?;
        match self.options.durability {
            RedCoreDurability::AofAlways => {
                file.sync_all()
                    .map_err(|err| GraphStoreError::io("fsync RedCore AOF", err))?;
                if created {
                    sync_directory(data_dir, "fsync RedCore data directory after AOF create")?;
                }
                self.last_fsync = Some(SystemTime::now());
            }
            RedCoreDurability::AofEverysec => {
                let should_sync = self
                    .last_fsync
                    .and_then(|last| last.elapsed().ok())
                    .map(|elapsed| elapsed >= Duration::from_secs(1))
                    .unwrap_or(true);
                if should_sync {
                    file.sync_data()
                        .map_err(|err| GraphStoreError::io("fsync RedCore AOF", err))?;
                    if created {
                        sync_directory(data_dir, "fsync RedCore data directory after AOF create")?;
                    }
                    self.last_fsync = Some(SystemTime::now());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn should_snapshot_for(&self, txn_id: u64) -> bool {
        let interval = self.options.snapshot_interval_writes;
        interval > 0 && txn_id > self.snapshot_txn_id && txn_id % interval == 0
    }

    fn write_snapshot(&mut self) -> GraphStoreResult<()> {
        let txn_id = self.last_txn_id;
        self.write_snapshot_for(txn_id, &self.store)?;
        self.snapshot_txn_id = txn_id;
        Ok(())
    }

    fn write_snapshot_for(&self, txn_id: u64, store: &InMemoryGraphStore) -> GraphStoreResult<()> {
        let Some(data_dir) = self.data_dir.as_ref() else {
            return Ok(());
        };
        fs::create_dir_all(data_dir)
            .map_err(|err| GraphStoreError::io("create RedCore snapshot directory", err))?;
        let envelope = RedCoreSnapshotEnvelope {
            version: REDCORE_MANIFEST_VERSION,
            txn_id,
            graph: store.snapshot(),
        };
        let raw = serde_json::to_vec_pretty(&envelope)
            .map_err(|err| GraphStoreError::io("encode RedCore snapshot", err))?;
        let current = data_dir.join(REDCORE_SNAPSHOT_FILE);
        if current.exists() {
            preserve_previous_snapshot(data_dir, &current)?;
        }
        write_atomic_file(
            data_dir,
            REDCORE_CURRENT_SNAPSHOT_TMP_FILE,
            REDCORE_SNAPSHOT_FILE,
            &raw,
            "RedCore snapshot",
        )?;
        Ok(())
    }

    fn write_manifest(&self) -> GraphStoreResult<()> {
        self.write_manifest_for(
            self.store.stats().version,
            self.last_txn_id,
            self.snapshot_txn_id,
        )
    }

    fn write_manifest_for(
        &self,
        graph_version: u64,
        last_txn_id: u64,
        snapshot_txn_id: u64,
    ) -> GraphStoreResult<()> {
        let Some(data_dir) = self.data_dir.as_ref() else {
            return Ok(());
        };
        let now = unix_ms();
        let manifest = RedCoreManifest {
            version: REDCORE_MANIFEST_VERSION,
            graph_version,
            last_txn_id,
            snapshot_txn_id,
            durability: self.options.durability,
            snapshot_file: REDCORE_SNAPSHOT_FILE.to_string(),
            aof_file: REDCORE_AOF_FILE.to_string(),
            updated_at_unix_ms: now,
            format_kind: default_format_kind(),
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at_unix_ms: now,
        };
        let raw = serde_json::to_vec_pretty(&manifest)
            .map_err(|err| GraphStoreError::io("encode RedCore manifest", err))?;
        write_atomic_file(
            data_dir,
            REDCORE_MANIFEST_TMP_FILE,
            REDCORE_MANIFEST_FILE,
            &raw,
            "RedCore manifest",
        )?;
        Ok(())
    }
}

impl Drop for RedCoreDirectoryLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        if let Some(locks) = REDCORE_PROCESS_LOCKS.get() {
            if let Ok(mut locks) = locks.lock() {
                locks.remove(&self.process_key);
            }
        }
    }
}

fn validate_redcore_options(options: &RedCoreOptions) -> GraphStoreResult<()> {
    if options.strict_acid && options.durability != RedCoreDurability::AofAlways {
        return Err(GraphStoreError::new(
            "redcore_strict_mode_invalid",
            "strict ACID mode requires RUSTY_RED_DURABILITY=aof_always",
        ));
    }
    Ok(())
}

fn acquire_redcore_directory_lock(data_dir: &Path) -> GraphStoreResult<RedCoreDirectoryLock> {
    let process_key = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    let locks = REDCORE_PROCESS_LOCKS.get_or_init(|| Mutex::new(BTreeSet::new()));
    {
        let mut locks = locks.lock().map_err(|_| {
            GraphStoreError::new(
                "redcore_lock_poisoned",
                "RedCore process lock registry is poisoned",
            )
        })?;
        if !locks.insert(process_key.clone()) {
            return Err(GraphStoreError::new(
                "redcore_lock_unavailable",
                format!(
                    "RedCore data directory {} is already open in this process",
                    data_dir.display()
                ),
            ));
        }
    }

    let lock_path = data_dir.join(REDCORE_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|err| {
            release_redcore_process_lock(&process_key);
            GraphStoreError::io("open RedCore directory lock", err)
        })?;
    if let Err(err) = file.try_lock_exclusive() {
        release_redcore_process_lock(&process_key);
        let message = if err.kind() == ErrorKind::WouldBlock {
            format!(
                "RedCore data directory {} is locked by another process",
                data_dir.display()
            )
        } else {
            format!("lock RedCore data directory: {err}")
        };
        return Err(GraphStoreError::new("redcore_lock_unavailable", message));
    }

    Ok(RedCoreDirectoryLock { file, process_key })
}

fn release_redcore_process_lock(process_key: &Path) {
    if let Some(locks) = REDCORE_PROCESS_LOCKS.get() {
        if let Ok(mut locks) = locks.lock() {
            locks.remove(process_key);
        }
    }
}

fn decode_aof_frame(raw: &str) -> GraphStoreResult<RedCoreAofFrame> {
    let frame: RedCoreAofFrame = serde_json::from_str(raw)
        .map_err(|err| GraphStoreError::io("decode RedCore AOF frame", err))?;
    if frame.magic != REDCORE_AOF_MAGIC || frame.version != REDCORE_MANIFEST_VERSION {
        return Err(GraphStoreError::new(
            "redcore_aof_frame_invalid",
            "RedCore AOF frame magic or version is invalid",
        ));
    }
    let checksum = stable_hash(&frame.mutation);
    if checksum != frame.payload_checksum {
        return Err(GraphStoreError::new(
            "redcore_aof_checksum_mismatch",
            format!("AOF frame {} checksum mismatch", frame.txn_id),
        ));
    }
    Ok(frame)
}

fn truncate_aof_tail(file: &File, data_dir: &Path, offset: u64) -> GraphStoreResult<()> {
    file.set_len(offset)
        .map_err(|err| GraphStoreError::io("truncate torn RedCore AOF tail", err))?;
    file.sync_all()
        .map_err(|err| GraphStoreError::io("fsync truncated RedCore AOF", err))?;
    sync_directory(
        data_dir,
        "fsync RedCore data directory after AOF truncation",
    )
}

fn preserve_previous_snapshot(data_dir: &Path, current: &Path) -> GraphStoreResult<()> {
    let raw =
        fs::read(current).map_err(|err| GraphStoreError::io("read previous snapshot", err))?;
    write_atomic_file(
        data_dir,
        REDCORE_PREVIOUS_SNAPSHOT_TMP_FILE,
        REDCORE_PREVIOUS_SNAPSHOT_FILE,
        &raw,
        "RedCore previous snapshot",
    )
}

fn write_atomic_file(
    data_dir: &Path,
    tmp_name: &str,
    final_name: &str,
    raw: &[u8],
    label: &str,
) -> GraphStoreResult<()> {
    fs::create_dir_all(data_dir)
        .map_err(|err| GraphStoreError::io(format!("create {label} directory"), err))?;
    let tmp_path = data_dir.join(tmp_name);
    match fs::remove_file(&tmp_path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(GraphStoreError::io(
                format!("remove stale {label} temp file"),
                err,
            ))
        }
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp_path)
        .map_err(|err| GraphStoreError::io(format!("create {label} temp file"), err))?;
    file.write_all(raw)
        .map_err(|err| GraphStoreError::io(format!("write {label} temp file"), err))?;
    file.sync_all()
        .map_err(|err| GraphStoreError::io(format!("fsync {label} temp file"), err))?;
    drop(file);
    fs::rename(&tmp_path, data_dir.join(final_name))
        .map_err(|err| GraphStoreError::io(format!("install {label}"), err))?;
    sync_directory(
        data_dir,
        format!("fsync RedCore directory after {label} install"),
    )
}

fn sync_file_path(path: &Path, action: &str) -> GraphStoreResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|err| GraphStoreError::io(action, err))
}

fn sync_directory(data_dir: &Path, action: impl AsRef<str>) -> GraphStoreResult<()> {
    File::open(data_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| GraphStoreError::io(action.as_ref(), err))
}

pub fn read_manifest(data_dir: &Path) -> GraphStoreResult<Option<RedCoreManifest>> {
    let path = data_dir.join(REDCORE_MANIFEST_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|err| GraphStoreError::io("read manifest", err))?;
    serde_json::from_str::<RedCoreManifest>(&raw)
        .map(Some)
        .map_err(|err| GraphStoreError::io("decode manifest", err))
}

fn read_latest_valid_snapshot(
    data_dir: &Path,
) -> GraphStoreResult<Option<RedCoreSnapshotEnvelope>> {
    match read_snapshot_file(&data_dir.join(REDCORE_SNAPSHOT_FILE), "current snapshot") {
        Ok(Some(snapshot)) => Ok(Some(snapshot)),
        Ok(None) => read_snapshot_file(
            &data_dir.join(REDCORE_PREVIOUS_SNAPSHOT_FILE),
            "previous snapshot",
        ),
        Err(current_error) => match read_snapshot_file(
            &data_dir.join(REDCORE_PREVIOUS_SNAPSHOT_FILE),
            "previous snapshot",
        ) {
            Ok(Some(snapshot)) => Ok(Some(snapshot)),
            Ok(None) => Err(current_error),
            Err(_) => Err(current_error),
        },
    }
}

fn read_snapshot_file(
    path: &Path,
    label: &str,
) -> GraphStoreResult<Option<RedCoreSnapshotEnvelope>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|err| GraphStoreError::io(format!("read RedCore {label}"), err))?;
    serde_json::from_str::<RedCoreSnapshotEnvelope>(&raw)
        .map(Some)
        .map_err(|err| GraphStoreError::io(format!("decode RedCore {label}"), err))
}

pub fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(feature = "redis-store")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedisGraphKeyspace {
    prefix: String,
}

#[cfg(feature = "redis-store")]
impl RedisGraphKeyspace {
    pub fn new(prefix: impl Into<String>) -> Self {
        let prefix = prefix.into().trim().trim_end_matches(':').to_string();
        Self {
            prefix: if prefix.is_empty() {
                "rrgdb:{tenant:default}:graph:v1".to_string()
            } else {
                prefix
            },
        }
    }

    pub fn tenant_prefix(base_prefix: &str, tenant_id: &str) -> String {
        let base_prefix = base_prefix.trim().trim_end_matches(':');
        let safe_tenant = sanitize_tenant_segment(tenant_id);
        if base_prefix.is_empty() {
            format!("rrgdb:{{tenant:{safe_tenant}}}:graph:v1")
        } else {
            format!("{base_prefix}:{{tenant:{safe_tenant}}}:graph:v1")
        }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn version(&self) -> String {
        self.key("version")
    }

    pub fn nodes(&self) -> String {
        self.key("nodes")
    }

    pub fn edges(&self) -> String {
        self.key("edges")
    }

    pub fn labels(&self) -> String {
        self.key("labels")
    }

    pub fn edge_types(&self) -> String {
        self.key("edge_types")
    }

    pub fn property_index_entries(&self) -> String {
        self.key("property_index_entries")
    }

    pub fn out_adjacency_pairs(&self) -> String {
        self.key("out_adjacency_pairs")
    }

    pub fn in_adjacency_pairs(&self) -> String {
        self.key("in_adjacency_pairs")
    }

    pub fn events(&self) -> String {
        self.key("events")
    }

    pub fn node(&self, id: &str) -> String {
        self.key(&format!("node:{}", encode_key_segment(id)))
    }

    pub fn edge(&self, id: &str) -> String {
        self.key(&format!("edge:{}", encode_key_segment(id)))
    }

    pub fn label(&self, label: &str) -> String {
        self.key(&format!("label:{}", encode_key_segment(label)))
    }

    pub fn edge_type(&self, edge_type: &str) -> String {
        self.key(&format!("edge_type:{}", encode_key_segment(edge_type)))
    }

    pub fn property_value(&self, key: &str, token: &str) -> String {
        self.key(&format!(
            "property:{}:{}",
            encode_key_segment(key),
            encode_key_segment(token)
        ))
    }

    pub fn out_adjacency(&self, node_id: &str, edge_type: &str) -> String {
        self.key(&format!(
            "adj:out:{}:{}",
            encode_key_segment(node_id),
            encode_key_segment(edge_type)
        ))
    }

    pub fn in_adjacency(&self, node_id: &str, edge_type: &str) -> String {
        self.key(&format!(
            "adj:in:{}:{}",
            encode_key_segment(node_id),
            encode_key_segment(edge_type)
        ))
    }

    fn key(&self, suffix: &str) -> String {
        format!("{}:{suffix}", self.prefix)
    }
}

#[cfg(feature = "redis-store")]
#[derive(Clone, Debug)]
pub struct RedisGraphStore {
    client: redis::Client,
    keyspace: RedisGraphKeyspace,
}

#[cfg(feature = "redis-store")]
impl RedisGraphStore {
    pub fn new(redis_url: &str, key_prefix: impl Into<String>) -> redis::RedisResult<Self> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
            keyspace: RedisGraphKeyspace::new(key_prefix),
        })
    }

    pub fn tenant(redis_url: &str, base_prefix: &str, tenant_id: &str) -> redis::RedisResult<Self> {
        Self::new(
            redis_url,
            RedisGraphKeyspace::tenant_prefix(base_prefix, tenant_id),
        )
    }

    pub fn keyspace(&self) -> &RedisGraphKeyspace {
        &self.keyspace
    }

    pub fn ping(&self) -> GraphStoreResult<()> {
        let mut connection = self.connection()?;
        redis::cmd("PING").query::<String>(&mut connection)?;
        Ok(())
    }

    pub fn upsert_node(&mut self, mut node: NodeRecord) -> GraphStoreResult<GraphWriteResult> {
        if node.id.trim().is_empty() {
            return Err(GraphStoreError::empty_field("node.id"));
        }

        node.labels = normalize_labels(node.labels);
        let mut connection = self.connection()?;
        let keyspace = self.keyspace.clone();
        let watch_keys = vec![keyspace.version(), keyspace.node(&node.id)];
        let mutation: GraphStoreResult<(GraphWriteResult, Option<NodeRecord>)> =
            redis::transaction(&mut connection, &watch_keys, |connection, pipe| {
                let existing = match load_node_raw_from_connection(connection, &keyspace, &node.id)
                {
                    Ok(existing) => existing,
                    Err(error) => return Ok(Some(Err(error))),
                };
                let current_version = redis::cmd("GET")
                    .arg(keyspace.version())
                    .query::<Option<u64>>(connection)?
                    .unwrap_or_default();
                let version = current_version + 1;
                let mut next_node = node.clone();
                next_node.version = version;
                let checksum = next_node.checksum();
                let raw = match serde_json::to_string(&next_node) {
                    Ok(raw) => raw,
                    Err(error) => {
                        return Ok(Some(Err(GraphStoreError::invalid_record(
                            "node",
                            &next_node.id,
                            error,
                        ))))
                    }
                };
                let event = match graph_event("node.upsert", &next_node.id, version, &checksum) {
                    Ok(event) => event,
                    Err(error) => return Ok(Some(Err(error))),
                };

                pipe.cmd("SET")
                    .arg(keyspace.version())
                    .arg(version)
                    .ignore()
                    .cmd("SET")
                    .arg(keyspace.node(&next_node.id))
                    .arg(raw)
                    .ignore()
                    .cmd("SADD")
                    .arg(keyspace.nodes())
                    .arg(&next_node.id)
                    .ignore()
                    .cmd("RPUSH")
                    .arg(keyspace.events())
                    .arg(event)
                    .ignore();
                if let Some(existing) = existing.as_ref() {
                    for label in &existing.labels {
                        pipe.cmd("SREM")
                            .arg(keyspace.label(label))
                            .arg(&existing.id)
                            .ignore();
                    }
                    remove_node_from_redis_property_indexes(pipe, &keyspace, existing);
                }
                if !next_node.tombstone {
                    for label in &next_node.labels {
                        pipe.cmd("SADD")
                            .arg(keyspace.labels())
                            .arg(label)
                            .ignore()
                            .cmd("SADD")
                            .arg(keyspace.label(label))
                            .arg(&next_node.id)
                            .ignore();
                    }
                    add_node_to_redis_property_indexes(pipe, &keyspace, &next_node);
                }
                let write = GraphWriteResult {
                    id: next_node.id.clone(),
                    version,
                    checksum,
                };
                match pipe.query::<Option<()>>(connection)? {
                    Some(()) => Ok(Some(Ok((write, existing)))),
                    None => Ok(None),
                }
            })?;
        let (write, existing) = mutation?;

        if let Some(existing) = existing {
            self.cleanup_empty_labels(&mut connection, &existing.labels)?;
            self.cleanup_empty_properties(
                &mut connection,
                &indexed_properties(&existing.properties),
            )?;
        }

        Ok(write)
    }

    pub fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        validate_edge_shape(&edge)?;
        let mut connection = self.connection()?;
        let keyspace = self.keyspace.clone();
        let watch_keys = vec![
            keyspace.version(),
            keyspace.edge(&edge.id),
            keyspace.node(&edge.from_id),
            keyspace.node(&edge.to_id),
        ];
        let mutation: GraphStoreResult<(GraphWriteResult, Option<EdgeRecord>)> =
            redis::transaction(&mut connection, &watch_keys, |connection, pipe| {
                let from_node =
                    match load_node_raw_from_connection(connection, &keyspace, &edge.from_id) {
                        Ok(node) => node,
                        Err(error) => return Ok(Some(Err(error))),
                    };
                if let Err(error) =
                    require_live_endpoint_record(&edge, "from", &edge.from_id, from_node.as_ref())
                {
                    return Ok(Some(Err(error)));
                }
                let to_node =
                    match load_node_raw_from_connection(connection, &keyspace, &edge.to_id) {
                        Ok(node) => node,
                        Err(error) => return Ok(Some(Err(error))),
                    };
                if let Err(error) =
                    require_live_endpoint_record(&edge, "to", &edge.to_id, to_node.as_ref())
                {
                    return Ok(Some(Err(error)));
                }
                let existing = match load_edge_raw_from_connection(connection, &keyspace, &edge.id)
                {
                    Ok(existing) => existing,
                    Err(error) => return Ok(Some(Err(error))),
                };
                let current_version = redis::cmd("GET")
                    .arg(keyspace.version())
                    .query::<Option<u64>>(connection)?
                    .unwrap_or_default();
                let version = current_version + 1;
                let mut next_edge = edge.clone();
                next_edge.version = version;
                let checksum = next_edge.checksum();
                let raw = match serde_json::to_string(&next_edge) {
                    Ok(raw) => raw,
                    Err(error) => {
                        return Ok(Some(Err(GraphStoreError::invalid_record(
                            "edge",
                            &next_edge.id,
                            error,
                        ))))
                    }
                };
                let event = match graph_event("edge.upsert", &next_edge.id, version, &checksum) {
                    Ok(event) => event,
                    Err(error) => return Ok(Some(Err(error))),
                };

                pipe.cmd("SET")
                    .arg(keyspace.version())
                    .arg(version)
                    .ignore()
                    .cmd("SET")
                    .arg(keyspace.edge(&next_edge.id))
                    .arg(raw)
                    .ignore()
                    .cmd("SADD")
                    .arg(keyspace.edges())
                    .arg(&next_edge.id)
                    .ignore()
                    .cmd("RPUSH")
                    .arg(keyspace.events())
                    .arg(event)
                    .ignore();
                if let Some(existing) = existing.as_ref() {
                    remove_edge_from_redis_indexes(pipe, &keyspace, existing);
                }
                if !next_edge.tombstone {
                    add_edge_to_redis_indexes(pipe, &keyspace, &next_edge);
                }
                let write = GraphWriteResult {
                    id: next_edge.id.clone(),
                    version,
                    checksum,
                };
                match pipe.query::<Option<()>>(connection)? {
                    Some(()) => Ok(Some(Ok((write, existing)))),
                    None => Ok(None),
                }
            })?;
        let (write, existing) = mutation?;

        if let Some(existing) = existing {
            self.cleanup_empty_edge_type(&mut connection, &existing.edge_type)?;
            self.cleanup_empty_adjacency_pair(
                &mut connection,
                Direction::Out,
                &existing.from_id,
                &existing.edge_type,
            )?;
            self.cleanup_empty_adjacency_pair(
                &mut connection,
                Direction::In,
                &existing.to_id,
                &existing.edge_type,
            )?;
        }

        Ok(write)
    }

    pub fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        Ok(self.load_node_raw(id)?.filter(|node| !node.tombstone))
    }

    pub fn get_edge(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        Ok(self.load_edge_raw(id)?.filter(|edge| !edge.tombstone))
    }

    pub fn node_ids_for_label(&self, label: &str) -> GraphStoreResult<Vec<String>> {
        let mut connection = self.connection()?;
        Ok(
            redis_string_set(&mut connection, self.keyspace.label(label))?
                .into_iter()
                .collect(),
        )
    }

    pub fn edge_ids_for_type(&self, edge_type: &str) -> GraphStoreResult<Vec<String>> {
        let mut connection = self.connection()?;
        Ok(
            redis_string_set(&mut connection, self.keyspace.edge_type(edge_type))?
                .into_iter()
                .collect(),
        )
    }

    pub fn node_ids_for_property(&self, key: &str, value: &Value) -> GraphStoreResult<Vec<String>> {
        let Some(token) = property_index_token(value) else {
            return Ok(Vec::new());
        };
        let mut connection = self.connection()?;
        Ok(
            redis_string_set(&mut connection, self.keyspace.property_value(key, &token))?
                .into_iter()
                .collect(),
        )
    }

    pub fn labels(&self) -> GraphStoreResult<Vec<String>> {
        let mut connection = self.connection()?;
        Ok(redis_string_set(&mut connection, self.keyspace.labels())?
            .into_iter()
            .filter(|label| {
                redis_string_set(&mut connection, self.keyspace.label(label))
                    .map(|ids| !ids.is_empty())
                    .unwrap_or(false)
            })
            .collect())
    }

    pub fn edge_types(&self) -> GraphStoreResult<Vec<String>> {
        let mut connection = self.connection()?;
        Ok(
            redis_string_set(&mut connection, self.keyspace.edge_types())?
                .into_iter()
                .filter(|edge_type| {
                    redis_string_set(&mut connection, self.keyspace.edge_type(edge_type))
                        .map(|ids| !ids.is_empty())
                        .unwrap_or(false)
                })
                .collect(),
        )
    }

    pub fn property_keys(&self) -> GraphStoreResult<Vec<String>> {
        let mut connection = self.connection()?;
        let mut keys = BTreeSet::new();
        for entry in redis_string_set(&mut connection, self.keyspace.property_index_entries())? {
            let Some((key, token)) = decode_property_pair(&entry) else {
                continue;
            };
            if !redis_string_set(&mut connection, self.keyspace.property_value(&key, &token))?
                .is_empty()
            {
                keys.insert(key);
            }
        }
        Ok(keys.into_iter().collect())
    }

    pub fn query_nodes(&self, query: NodeQuery) -> GraphStoreResult<Vec<NodeRecord>> {
        let mut candidate_ids: Option<BTreeSet<String>> = None;
        if let Some(label) = query.normalized_label() {
            merge_candidates(
                &mut candidate_ids,
                Some(self.node_ids_for_label(&label)?.into_iter().collect()),
            );
        }
        for (key, value) in &query.properties {
            let key = key.trim();
            if key.is_empty() {
                return Ok(Vec::new());
            }
            let Some(token) = property_index_token(value) else {
                return Ok(Vec::new());
            };
            let mut connection = self.connection()?;
            merge_candidates(
                &mut candidate_ids,
                Some(
                    redis_string_set(&mut connection, self.keyspace.property_value(key, &token))?
                        .into_iter()
                        .collect(),
                ),
            );
        }

        let ids = match candidate_ids {
            Some(ids) => ids,
            None => self.live_nodes()?.into_keys().collect(),
        };
        let mut nodes = Vec::new();
        for id in ids.into_iter().take(query.bounded_limit()) {
            if let Some(node) = self.get_node(&id)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    pub fn neighbors(&self, query: NeighborQuery) -> GraphStoreResult<Vec<NeighborHit>> {
        let mut connection = self.connection()?;
        let mut edge_ids = BTreeSet::new();
        match query.edge_type {
            Some(edge_type) => {
                let key = match query.direction {
                    Direction::Out => self.keyspace.out_adjacency(&query.node_id, &edge_type),
                    Direction::In => self.keyspace.in_adjacency(&query.node_id, &edge_type),
                };
                edge_ids.extend(redis_string_set(&mut connection, key)?);
            }
            None => {
                let edge_types = redis_string_set(&mut connection, self.keyspace.edge_types())?;
                for edge_type in edge_types {
                    let key = match query.direction {
                        Direction::Out => self.keyspace.out_adjacency(&query.node_id, &edge_type),
                        Direction::In => self.keyspace.in_adjacency(&query.node_id, &edge_type),
                    };
                    edge_ids.extend(redis_string_set(&mut connection, key)?);
                }
            }
        }

        let mut hits = Vec::new();
        for edge_id in edge_ids {
            let Some(edge) = self.get_edge(&edge_id)? else {
                continue;
            };
            let node_id = match query.direction {
                Direction::Out => edge.to_id.clone(),
                Direction::In => edge.from_id.clone(),
            };
            if self.get_node(&node_id)?.is_none() {
                continue;
            }
            hits.push(NeighborHit {
                edge_id: edge.id.clone(),
                node_id,
                edge_type: edge.edge_type.clone(),
                confidence: edge.confidence,
                epistemic_type: edge.epistemic_type.clone(),
            });
        }
        Ok(hits)
    }

    pub fn stats(&self) -> GraphStoreResult<GraphStats> {
        let live_nodes = self.live_nodes()?;
        let live_edges = self.live_edges()?;
        let mut connection = self.connection()?;
        let version = redis::cmd("GET")
            .arg(self.keyspace.version())
            .query::<Option<u64>>(&mut connection)?
            .unwrap_or_default();
        Ok(GraphStats {
            version,
            nodes_total: live_nodes.len(),
            edges_total: live_edges.len(),
            labels_total: self.labels()?.len(),
            edge_types_total: self.edge_types()?.len(),
            property_keys_total: self.property_keys()?.len(),
            property_indexes_total: self.redis_indexes()?.property_index.len(),
            memory_bytes: 0,
            memory_quota_bytes: 0,
        })
    }

    pub fn verify(&self) -> GraphStoreResult<VerifyReport> {
        let live_nodes = self.live_nodes()?;
        let live_edges = self.live_edges()?;
        let mut expected = ExpectedIndexes::default();
        let mut problems = Vec::new();

        for node in live_nodes.values() {
            for label in &node.labels {
                expected
                    .label_index
                    .entry(label.clone())
                    .or_default()
                    .insert(node.id.clone());
            }
            for (key, token) in indexed_properties(&node.properties) {
                expected
                    .property_index
                    .entry((key, token))
                    .or_default()
                    .insert(node.id.clone());
            }
        }

        for edge in live_edges.values() {
            if !live_nodes.contains_key(&edge.from_id) {
                problems.push(VerifyProblem {
                    kind: "missing_from_endpoint".to_string(),
                    id: edge.id.clone(),
                    detail: format!("from endpoint {} is not a live node", edge.from_id),
                });
            }
            if !live_nodes.contains_key(&edge.to_id) {
                problems.push(VerifyProblem {
                    kind: "missing_to_endpoint".to_string(),
                    id: edge.id.clone(),
                    detail: format!("to endpoint {} is not a live node", edge.to_id),
                });
            }
            expected
                .edge_type_index
                .entry(edge.edge_type.clone())
                .or_default()
                .insert(edge.id.clone());
            expected
                .out_adjacency
                .entry((edge.from_id.clone(), edge.edge_type.clone()))
                .or_default()
                .insert(edge.id.clone());
            expected
                .in_adjacency
                .entry((edge.to_id.clone(), edge.edge_type.clone()))
                .or_default()
                .insert(edge.id.clone());
        }

        let actual = self.redis_indexes()?;
        if actual.label_index != expected.label_index {
            problems.push(VerifyProblem {
                kind: "label_index_drift".to_string(),
                id: "label_index".to_string(),
                detail: "Redis label index does not match live node labels".to_string(),
            });
        }
        if actual.edge_type_index != expected.edge_type_index {
            problems.push(VerifyProblem {
                kind: "edge_type_index_drift".to_string(),
                id: "edge_type_index".to_string(),
                detail: "Redis edge type index does not match live edge types".to_string(),
            });
        }
        if actual.property_index != expected.property_index {
            problems.push(VerifyProblem {
                kind: "property_index_drift".to_string(),
                id: "property_index".to_string(),
                detail: "Redis property index does not match live scalar node properties"
                    .to_string(),
            });
        }
        if actual.out_adjacency != expected.out_adjacency {
            problems.push(VerifyProblem {
                kind: "out_adjacency_drift".to_string(),
                id: "out_adjacency".to_string(),
                detail: "Redis out adjacency index does not match live edges".to_string(),
            });
        }
        if actual.in_adjacency != expected.in_adjacency {
            problems.push(VerifyProblem {
                kind: "in_adjacency_drift".to_string(),
                id: "in_adjacency".to_string(),
                detail: "Redis in adjacency index does not match live edges".to_string(),
            });
        }

        Ok(VerifyReport {
            ok: problems.is_empty(),
            stats: self.stats()?,
            problems,
        })
    }

    pub fn rebuild_indexes(&mut self) -> GraphStoreResult<GraphRebuildReport> {
        let before = self.verify()?;
        let mut connection = self.connection()?;
        let keyspace = self.keyspace.clone();
        let watch_keys = vec![keyspace.version()];
        let rebuild: GraphStoreResult<()> =
            redis::transaction(&mut connection, &watch_keys, |connection, pipe| {
                let live_nodes = match live_nodes_from_connection(connection, &keyspace) {
                    Ok(nodes) => nodes,
                    Err(error) => return Ok(Some(Err(error))),
                };
                let live_edges = match live_edges_from_connection(connection, &keyspace) {
                    Ok(edges) => edges,
                    Err(error) => return Ok(Some(Err(error))),
                };
                let actual = match redis_indexes_from_connection(connection, &keyspace) {
                    Ok(indexes) => indexes,
                    Err(error) => return Ok(Some(Err(error))),
                };

                pipe.cmd("DEL")
                    .arg(keyspace.labels())
                    .arg(keyspace.edge_types())
                    .arg(keyspace.property_index_entries())
                    .arg(keyspace.out_adjacency_pairs())
                    .arg(keyspace.in_adjacency_pairs())
                    .ignore();
                for label in actual.label_index.keys() {
                    pipe.cmd("DEL").arg(keyspace.label(label)).ignore();
                }
                for edge_type in actual.edge_type_index.keys() {
                    pipe.cmd("DEL").arg(keyspace.edge_type(edge_type)).ignore();
                }
                for (key, token) in actual.property_index.keys() {
                    pipe.cmd("DEL")
                        .arg(keyspace.property_value(key, token))
                        .ignore();
                }
                for (node_id, edge_type) in actual.out_adjacency.keys() {
                    pipe.cmd("DEL")
                        .arg(keyspace.out_adjacency(node_id, edge_type))
                        .ignore();
                }
                for (node_id, edge_type) in actual.in_adjacency.keys() {
                    pipe.cmd("DEL")
                        .arg(keyspace.in_adjacency(node_id, edge_type))
                        .ignore();
                }

                for node in live_nodes.values() {
                    for label in &node.labels {
                        pipe.cmd("SADD")
                            .arg(keyspace.labels())
                            .arg(label)
                            .ignore()
                            .cmd("SADD")
                            .arg(keyspace.label(label))
                            .arg(&node.id)
                            .ignore();
                    }
                    add_node_to_redis_property_indexes(pipe, &keyspace, node);
                }
                for edge in live_edges.values() {
                    add_edge_to_redis_indexes(pipe, &keyspace, edge);
                }

                match pipe.query::<Option<()>>(connection)? {
                    Some(()) => Ok(Some(Ok(()))),
                    None => Ok(None),
                }
            })?;
        rebuild?;
        let after = self.verify()?;
        Ok(GraphRebuildReport {
            repaired: !before.ok && after.ok,
            before,
            after,
        })
    }

    fn connection(&self) -> GraphStoreResult<redis::Connection> {
        Ok(self.client.get_connection()?)
    }

    fn load_node_raw(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        let mut connection = self.connection()?;
        load_node_raw_from_connection(&mut connection, &self.keyspace, id)
    }

    fn load_edge_raw(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        let mut connection = self.connection()?;
        load_edge_raw_from_connection(&mut connection, &self.keyspace, id)
    }

    fn live_nodes(&self) -> GraphStoreResult<BTreeMap<String, NodeRecord>> {
        let mut connection = self.connection()?;
        live_nodes_from_connection(&mut connection, &self.keyspace)
    }

    fn live_edges(&self) -> GraphStoreResult<BTreeMap<String, EdgeRecord>> {
        let mut connection = self.connection()?;
        live_edges_from_connection(&mut connection, &self.keyspace)
    }

    fn redis_indexes(&self) -> GraphStoreResult<ExpectedIndexes> {
        let mut connection = self.connection()?;
        redis_indexes_from_connection(&mut connection, &self.keyspace)
    }

    fn cleanup_empty_labels(
        &self,
        connection: &mut redis::Connection,
        labels: &[String],
    ) -> GraphStoreResult<()> {
        for label in labels {
            cleanup_empty_redis_set(
                connection,
                self.keyspace.label(label),
                self.keyspace.labels(),
                label,
            )?;
        }
        Ok(())
    }

    fn cleanup_empty_properties(
        &self,
        connection: &mut redis::Connection,
        properties: &BTreeMap<String, String>,
    ) -> GraphStoreResult<()> {
        for (key, token) in properties {
            cleanup_empty_redis_set(
                connection,
                self.keyspace.property_value(key, token),
                self.keyspace.property_index_entries(),
                &property_pair(key, token),
            )?;
        }
        Ok(())
    }

    fn cleanup_empty_edge_type(
        &self,
        connection: &mut redis::Connection,
        edge_type: &str,
    ) -> GraphStoreResult<()> {
        cleanup_empty_redis_set(
            connection,
            self.keyspace.edge_type(edge_type),
            self.keyspace.edge_types(),
            edge_type,
        )
    }

    fn cleanup_empty_adjacency_pair(
        &self,
        connection: &mut redis::Connection,
        direction: Direction,
        node_id: &str,
        edge_type: &str,
    ) -> GraphStoreResult<()> {
        let pair = adjacency_pair(node_id, edge_type);
        let (index_key, catalog_key) = match direction {
            Direction::Out => (
                self.keyspace.out_adjacency(node_id, edge_type),
                self.keyspace.out_adjacency_pairs(),
            ),
            Direction::In => (
                self.keyspace.in_adjacency(node_id, edge_type),
                self.keyspace.in_adjacency_pairs(),
            ),
        };
        cleanup_empty_redis_set(connection, index_key, catalog_key, &pair)
    }
}

impl GraphStore for InMemoryGraphStore {
    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<GraphWriteResult> {
        InMemoryGraphStore::upsert_node(self, node)
    }

    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        InMemoryGraphStore::upsert_edge(self, edge)
    }

    fn get_node(&self, id: &str) -> Option<&NodeRecord> {
        InMemoryGraphStore::get_node(self, id)
    }

    fn get_edge(&self, id: &str) -> Option<&EdgeRecord> {
        InMemoryGraphStore::get_edge(self, id)
    }

    fn query_nodes(&self, query: NodeQuery) -> Vec<NodeRecord> {
        InMemoryGraphStore::query_nodes(self, query)
    }

    fn neighbors(&self, query: NeighborQuery) -> Vec<NeighborHit> {
        InMemoryGraphStore::neighbors(self, query)
    }

    fn stats(&self) -> GraphStats {
        InMemoryGraphStore::stats(self)
    }

    fn verify(&self) -> VerifyReport {
        InMemoryGraphStore::verify(self)
    }

    fn rebuild_indexes(&mut self) -> GraphStoreResult<GraphRebuildReport> {
        InMemoryGraphStore::rebuild_indexes(self)
    }
}

#[derive(Default)]
struct ExpectedIndexes {
    out_adjacency: BTreeMap<(String, String), BTreeSet<String>>,
    in_adjacency: BTreeMap<(String, String), BTreeSet<String>>,
    label_index: BTreeMap<String, BTreeSet<String>>,
    edge_type_index: BTreeMap<String, BTreeSet<String>>,
    property_index: BTreeMap<(String, String), BTreeSet<String>>,
}

fn validate_edge_shape(edge: &EdgeRecord) -> GraphStoreResult<()> {
    if edge.id.trim().is_empty() {
        return Err(GraphStoreError::empty_field("edge.id"));
    }
    if edge.from_id.trim().is_empty() {
        return Err(GraphStoreError::empty_field("edge.from_id"));
    }
    if edge.to_id.trim().is_empty() {
        return Err(GraphStoreError::empty_field("edge.to_id"));
    }
    if edge.edge_type.trim().is_empty() {
        return Err(GraphStoreError::empty_field("edge.type"));
    }
    Ok(())
}

fn extract_float_array(properties: &Value, key: &str) -> Option<Vec<f32>> {
    let arr = properties.get(key)?.as_array()?;
    let floats: Vec<f32> = arr
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();
    if floats.len() == arr.len() {
        Some(floats)
    } else {
        None
    }
}

fn normalize_labels(labels: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
    let mut labels = labels
        .into_iter()
        .map(Into::into)
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    labels
}

fn remove_index_value<K: Ord + Clone>(
    index: &mut BTreeMap<K, BTreeSet<String>>,
    key: &K,
    value: &str,
) {
    let should_remove = match index.get_mut(key) {
        Some(values) => {
            values.remove(value);
            values.is_empty()
        }
        None => false,
    };
    if should_remove {
        index.remove(key);
    }
}

fn sorted_values(values: Option<&BTreeSet<String>>) -> Vec<String> {
    values
        .map(|values| values.iter().cloned().collect())
        .unwrap_or_default()
}

fn string_index_bytes(index: &BTreeMap<String, BTreeSet<String>>) -> usize {
    index
        .iter()
        .map(|(key, values)| key.len() + values.iter().map(String::len).sum::<usize>())
        .sum()
}

fn tuple_index_bytes(index: &BTreeMap<(String, String), BTreeSet<String>>) -> usize {
    index
        .iter()
        .map(|((left, right), values)| {
            left.len() + right.len() + values.iter().map(String::len).sum::<usize>()
        })
        .sum()
}

fn merge_candidates(candidates: &mut Option<BTreeSet<String>>, next: Option<BTreeSet<String>>) {
    let next = next.unwrap_or_default();
    match candidates {
        Some(existing) => {
            *existing = existing.intersection(&next).cloned().collect();
        }
        None => *candidates = Some(next),
    }
}

fn indexed_properties(properties: &Value) -> BTreeMap<String, String> {
    let Some(properties) = properties.as_object() else {
        return BTreeMap::new();
    };
    properties
        .iter()
        .filter_map(|(key, value)| {
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            property_index_token(value).map(|token| (key.to_string(), token))
        })
        .collect()
}

fn property_index_token(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).ok()
        }
        Value::Array(_) | Value::Object(_) => None,
    }
}

#[cfg(feature = "redis-store")]
fn redis_string_set(
    connection: &mut redis::Connection,
    key: String,
) -> GraphStoreResult<BTreeSet<String>> {
    let values = redis::cmd("SMEMBERS")
        .arg(key)
        .query::<Vec<String>>(connection)?;
    Ok(values.into_iter().collect())
}

#[cfg(feature = "redis-store")]
fn live_nodes_from_connection(
    connection: &mut redis::Connection,
    keyspace: &RedisGraphKeyspace,
) -> GraphStoreResult<BTreeMap<String, NodeRecord>> {
    let node_ids = redis_string_set(connection, keyspace.nodes())?;
    let mut nodes = BTreeMap::new();
    for node_id in node_ids {
        if let Some(node) = load_node_raw_from_connection(connection, keyspace, &node_id)?
            .filter(|node| !node.tombstone)
        {
            nodes.insert(node_id, node);
        }
    }
    Ok(nodes)
}

#[cfg(feature = "redis-store")]
fn live_edges_from_connection(
    connection: &mut redis::Connection,
    keyspace: &RedisGraphKeyspace,
) -> GraphStoreResult<BTreeMap<String, EdgeRecord>> {
    let edge_ids = redis_string_set(connection, keyspace.edges())?;
    let mut edges = BTreeMap::new();
    for edge_id in edge_ids {
        if let Some(edge) = load_edge_raw_from_connection(connection, keyspace, &edge_id)?
            .filter(|edge| !edge.tombstone)
        {
            edges.insert(edge_id, edge);
        }
    }
    Ok(edges)
}

#[cfg(feature = "redis-store")]
fn redis_indexes_from_connection(
    connection: &mut redis::Connection,
    keyspace: &RedisGraphKeyspace,
) -> GraphStoreResult<ExpectedIndexes> {
    let mut indexes = ExpectedIndexes::default();
    for label in redis_string_set(connection, keyspace.labels())? {
        let node_ids = redis_string_set(connection, keyspace.label(&label))?;
        if !node_ids.is_empty() {
            indexes.label_index.insert(label, node_ids);
        }
    }
    for edge_type in redis_string_set(connection, keyspace.edge_types())? {
        let edge_ids = redis_string_set(connection, keyspace.edge_type(&edge_type))?;
        if !edge_ids.is_empty() {
            indexes.edge_type_index.insert(edge_type, edge_ids);
        }
    }
    for entry in redis_string_set(connection, keyspace.property_index_entries())? {
        let Some((key, token)) = decode_property_pair(&entry) else {
            continue;
        };
        let node_ids = redis_string_set(connection, keyspace.property_value(&key, &token))?;
        if !node_ids.is_empty() {
            indexes.property_index.insert((key, token), node_ids);
        }
    }
    for pair in redis_string_set(connection, keyspace.out_adjacency_pairs())? {
        let Some((node_id, edge_type)) = decode_adjacency_pair(&pair) else {
            continue;
        };
        let edge_ids = redis_string_set(connection, keyspace.out_adjacency(&node_id, &edge_type))?;
        if !edge_ids.is_empty() {
            indexes.out_adjacency.insert((node_id, edge_type), edge_ids);
        }
    }
    for pair in redis_string_set(connection, keyspace.in_adjacency_pairs())? {
        let Some((node_id, edge_type)) = decode_adjacency_pair(&pair) else {
            continue;
        };
        let edge_ids = redis_string_set(connection, keyspace.in_adjacency(&node_id, &edge_type))?;
        if !edge_ids.is_empty() {
            indexes.in_adjacency.insert((node_id, edge_type), edge_ids);
        }
    }
    Ok(indexes)
}

#[cfg(feature = "redis-store")]
fn load_node_raw_from_connection(
    connection: &mut redis::Connection,
    keyspace: &RedisGraphKeyspace,
    id: &str,
) -> GraphStoreResult<Option<NodeRecord>> {
    let raw = redis::cmd("GET")
        .arg(keyspace.node(id))
        .query::<Option<String>>(connection)?;
    raw.map(|value| {
        serde_json::from_str::<NodeRecord>(&value)
            .map_err(|err| GraphStoreError::invalid_record("node", id, err))
    })
    .transpose()
}

#[cfg(feature = "redis-store")]
fn load_edge_raw_from_connection(
    connection: &mut redis::Connection,
    keyspace: &RedisGraphKeyspace,
    id: &str,
) -> GraphStoreResult<Option<EdgeRecord>> {
    let raw = redis::cmd("GET")
        .arg(keyspace.edge(id))
        .query::<Option<String>>(connection)?;
    raw.map(|value| {
        serde_json::from_str::<EdgeRecord>(&value)
            .map_err(|err| GraphStoreError::invalid_record("edge", id, err))
    })
    .transpose()
}

#[cfg(feature = "redis-store")]
fn require_live_endpoint_record(
    edge: &EdgeRecord,
    endpoint: &str,
    node_id: &str,
    node: Option<&NodeRecord>,
) -> GraphStoreResult<()> {
    let Some(node) = node else {
        return Err(GraphStoreError::missing_endpoint(
            &edge.id, endpoint, node_id,
        ));
    };
    if node.tombstone {
        return Err(GraphStoreError::tombstoned_endpoint(
            &edge.id, endpoint, node_id,
        ));
    }
    Ok(())
}

#[cfg(feature = "redis-store")]
fn add_edge_to_redis_indexes(
    pipe: &mut redis::Pipeline,
    keyspace: &RedisGraphKeyspace,
    edge: &EdgeRecord,
) {
    let out_pair = adjacency_pair(&edge.from_id, &edge.edge_type);
    let in_pair = adjacency_pair(&edge.to_id, &edge.edge_type);
    pipe.cmd("SADD")
        .arg(keyspace.edge_types())
        .arg(&edge.edge_type)
        .ignore()
        .cmd("SADD")
        .arg(keyspace.edge_type(&edge.edge_type))
        .arg(&edge.id)
        .ignore()
        .cmd("SADD")
        .arg(keyspace.out_adjacency_pairs())
        .arg(out_pair)
        .ignore()
        .cmd("SADD")
        .arg(keyspace.out_adjacency(&edge.from_id, &edge.edge_type))
        .arg(&edge.id)
        .ignore()
        .cmd("SADD")
        .arg(keyspace.in_adjacency_pairs())
        .arg(in_pair)
        .ignore()
        .cmd("SADD")
        .arg(keyspace.in_adjacency(&edge.to_id, &edge.edge_type))
        .arg(&edge.id)
        .ignore();
}

#[cfg(feature = "redis-store")]
fn remove_edge_from_redis_indexes(
    pipe: &mut redis::Pipeline,
    keyspace: &RedisGraphKeyspace,
    edge: &EdgeRecord,
) {
    pipe.cmd("SREM")
        .arg(keyspace.edge_type(&edge.edge_type))
        .arg(&edge.id)
        .ignore()
        .cmd("SREM")
        .arg(keyspace.out_adjacency(&edge.from_id, &edge.edge_type))
        .arg(&edge.id)
        .ignore()
        .cmd("SREM")
        .arg(keyspace.in_adjacency(&edge.to_id, &edge.edge_type))
        .arg(&edge.id)
        .ignore();
}

#[cfg(feature = "redis-store")]
fn add_node_to_redis_property_indexes(
    pipe: &mut redis::Pipeline,
    keyspace: &RedisGraphKeyspace,
    node: &NodeRecord,
) {
    for (key, token) in indexed_properties(&node.properties) {
        pipe.cmd("SADD")
            .arg(keyspace.property_index_entries())
            .arg(property_pair(&key, &token))
            .ignore()
            .cmd("SADD")
            .arg(keyspace.property_value(&key, &token))
            .arg(&node.id)
            .ignore();
    }
}

#[cfg(feature = "redis-store")]
fn remove_node_from_redis_property_indexes(
    pipe: &mut redis::Pipeline,
    keyspace: &RedisGraphKeyspace,
    node: &NodeRecord,
) {
    for (key, token) in indexed_properties(&node.properties) {
        pipe.cmd("SREM")
            .arg(keyspace.property_value(&key, &token))
            .arg(&node.id)
            .ignore();
    }
}

#[cfg(feature = "redis-store")]
fn cleanup_empty_redis_set(
    connection: &mut redis::Connection,
    set_key: String,
    catalog_key: String,
    catalog_member: &str,
) -> GraphStoreResult<()> {
    let count = redis::cmd("SCARD")
        .arg(&set_key)
        .query::<usize>(connection)?;
    if count == 0 {
        redis::pipe()
            .atomic()
            .cmd("DEL")
            .arg(set_key)
            .ignore()
            .cmd("SREM")
            .arg(catalog_key)
            .arg(catalog_member)
            .ignore()
            .query::<()>(connection)?;
    }
    Ok(())
}

#[cfg(feature = "redis-store")]
fn graph_event(
    event_type: &str,
    id: &str,
    version: u64,
    checksum: &str,
) -> GraphStoreResult<String> {
    serde_json::to_string(&serde_json::json!({
        "type": event_type,
        "id": id,
        "version": version,
        "checksum": checksum,
    }))
    .map_err(|err| GraphStoreError::invalid_record("event", id, err))
}

#[cfg(feature = "redis-store")]
fn adjacency_pair(node_id: &str, edge_type: &str) -> String {
    serde_json::to_string(&(node_id, edge_type))
        .unwrap_or_else(|_| format!("{node_id}\t{edge_type}"))
}

#[cfg(feature = "redis-store")]
fn decode_adjacency_pair(raw: &str) -> Option<(String, String)> {
    serde_json::from_str::<(String, String)>(raw).ok()
}

#[cfg(feature = "redis-store")]
fn property_pair(key: &str, token: &str) -> String {
    serde_json::to_string(&(key, token)).unwrap_or_else(|_| format!("{key}\t{token}"))
}

#[cfg(feature = "redis-store")]
fn decode_property_pair(raw: &str) -> Option<(String, String)> {
    serde_json::from_str::<(String, String)>(raw).ok()
}

pub fn sanitize_tenant_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect::<String>();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}

#[cfg(feature = "redis-store")]
fn encode_key_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(1 + value.len() * 2);
    encoded.push('h');
    for byte in value.as_bytes() {
        encoded.push(hex_digit(byte >> 4));
        encoded.push(hex_digit(byte & 0x0f));
    }
    encoded
}

#[cfg(feature = "redis-store")]
fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => unreachable!("hex digit nibble is always <= 15"),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;

    use super::{
        Direction, EdgeRecord, GraphMutation, GraphMutationBatch, GraphStore, InMemoryGraphStore,
        NeighborQuery, NodeQuery, NodeRecord, RedCoreDurability, RedCoreGraphStore, RedCoreOptions,
    };

    #[test]
    fn records_have_stable_hashes_and_metadata() {
        let mut node = NodeRecord::new(
            "node:1",
            ["Person", "Person", " User "],
            json!({ "name": "Ada" }),
        );
        node.version = 7;

        assert_eq!(node.labels, vec!["Person".to_string(), "User".to_string()]);
        assert!(node.checksum().starts_with("sha256:"));

        let mut edge = EdgeRecord::new(
            "edge:1",
            "node:1",
            "KNOWS",
            "node:2",
            json!({ "confidence": 0.9 }),
        );
        edge.version = 8;

        assert_eq!(edge.from_id, "node:1");
        assert_eq!(edge.to_id, "node:2");
        assert_eq!(edge.edge_type, "KNOWS");
        assert!(edge.checksum().starts_with("sha256:"));
    }

    #[test]
    fn memory_store_upserts_nodes_edges_and_adjacency() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["Person"],
                json!({ "name": "Ada" }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:b",
                ["Person", "Engineer"],
                json!({ "name": "Grace" }),
            ))
            .unwrap();

        let write = store
            .upsert_edge(EdgeRecord::new(
                "edge:ab",
                "node:a",
                "KNOWS",
                "node:b",
                json!({ "since": 1952 }),
            ))
            .unwrap();

        assert_eq!(write.id, "edge:ab");
        assert_eq!(store.get_node("node:a").unwrap().version, 1);
        assert_eq!(store.get_edge("edge:ab").unwrap().version, 3);
        assert_eq!(
            store.neighbors(NeighborQuery::out("node:a")),
            vec![super::NeighborHit {
                edge_id: "edge:ab".to_string(),
                node_id: "node:b".to_string(),
                edge_type: "KNOWS".to_string(),
                confidence: None,
                epistemic_type: None,
            }]
        );
        assert_eq!(
            store.neighbors(NeighborQuery::in_("node:b").with_edge_type("KNOWS"))[0].node_id,
            "node:a"
        );
        assert_eq!(store.verify().ok, true);
    }

    #[test]
    fn label_and_edge_type_indexes_track_updates() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["Person"],
                json!({"name": "Ada", "kind": "scientist"}),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:b",
                ["Person"],
                json!({"name": "Grace", "kind": "engineer"}),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:ab",
                "node:a",
                "KNOWS",
                "node:b",
                json!({}),
            ))
            .unwrap();

        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["System"],
                json!({"name": "Ada", "kind": "engine"}),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:ab",
                "node:a",
                "CALLS",
                "node:b",
                json!({}),
            ))
            .unwrap();

        assert_eq!(
            store.node_ids_for_label("Person"),
            vec!["node:b".to_string()]
        );
        assert_eq!(
            store.node_ids_for_label("System"),
            vec!["node:a".to_string()]
        );
        assert!(store.edge_ids_for_type("KNOWS").is_empty());
        assert_eq!(
            store.edge_ids_for_type("CALLS"),
            vec!["edge:ab".to_string()]
        );
        assert_eq!(
            store
                .neighbors(NeighborQuery {
                    node_id: "node:a".to_string(),
                    direction: Direction::Out,
                    edge_type: Some("CALLS".to_string()),
                })
                .len(),
            1
        );
        assert_eq!(
            store.node_ids_for_property("kind", &json!("engine")),
            vec!["node:a".to_string()]
        );
        assert!(store
            .node_ids_for_property("kind", &json!("scientist"))
            .is_empty());
        assert_eq!(store.verify().ok, true);
    }

    #[test]
    fn property_indexes_support_exact_node_seek() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["File"],
                json!({"path": "src/lib.rs", "repo": "rusty-red", "rank": 1}),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:b",
                ["File"],
                json!({"path": "src/main.rs", "repo": "rusty-red", "rank": 2}),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:c",
                ["Symbol"],
                json!({"path": "src/lib.rs", "repo": "rusty-red"}),
            ))
            .unwrap();

        let hits = store.query_nodes(
            NodeQuery::label("File")
                .with_property("repo", json!("rusty-red"))
                .with_property("path", json!("src/lib.rs")),
        );

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "node:a");
        assert_eq!(
            store.property_keys(),
            vec!["path".to_string(), "rank".to_string(), "repo".to_string()]
        );
        assert_eq!(store.stats().property_indexes_total, 5);
        assert_eq!(store.verify().ok, true);
    }

    #[test]
    fn upserting_edge_requires_live_endpoints() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new("node:a", ["Person"], json!({})))
            .unwrap();

        let error = store
            .upsert_edge(EdgeRecord::new(
                "edge:missing",
                "node:a",
                "KNOWS",
                "node:missing",
                json!({}),
            ))
            .unwrap_err();

        assert_eq!(error.code, "missing_graph_endpoint");
        assert!(store.edge_ids_for_type("KNOWS").is_empty());
    }

    #[test]
    fn verify_detects_index_drift() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new("node:a", ["Person"], json!({})))
            .unwrap();
        store
            .upsert_node(NodeRecord::new("node:b", ["Person"], json!({})))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:ab",
                "node:a",
                "KNOWS",
                "node:b",
                json!({}),
            ))
            .unwrap();

        store
            .out_adjacency
            .get_mut(&("node:a".to_string(), "KNOWS".to_string()))
            .unwrap()
            .remove("edge:ab");
        store
            .property_index
            .entry(("name".to_string(), "\"Ada\"".to_string()))
            .or_default()
            .insert("node:a".to_string());

        let report = store.verify();

        assert_eq!(report.ok, false);
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.kind == "out_adjacency_drift"));
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.kind == "property_index_drift"));
    }

    #[test]
    fn rebuild_indexes_repairs_derived_index_drift() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new(
                "node:a",
                ["Person"],
                json!({ "name": "Ada" }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "node:b",
                ["Person"],
                json!({ "name": "Grace" }),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:ab",
                "node:a",
                "KNOWS",
                "node:b",
                json!({}),
            ))
            .unwrap();

        store.label_index.clear();
        store
            .out_adjacency
            .get_mut(&("node:a".to_string(), "KNOWS".to_string()))
            .unwrap()
            .remove("edge:ab");
        store
            .property_index
            .entry(("name".to_string(), "\"Wrong\"".to_string()))
            .or_default()
            .insert("node:a".to_string());

        assert!(store.query_nodes(NodeQuery::label("Person")).is_empty());
        let report = store.rebuild_indexes().unwrap();

        assert_eq!(report.repaired, true);
        assert_eq!(report.before.ok, false);
        assert_eq!(report.after.ok, true);
        assert_eq!(store.query_nodes(NodeQuery::label("Person")).len(), 2);
        assert_eq!(
            store.neighbors(NeighborQuery::out("node:a"))[0].node_id,
            "node:b"
        );
        assert_eq!(
            store.node_ids_for_property("name", &json!("Ada")),
            vec!["node:a".to_string()]
        );
    }

    #[test]
    fn rebuild_indexes_does_not_hide_canonical_edge_corruption() {
        let mut store = InMemoryGraphStore::new();
        store
            .upsert_node(NodeRecord::new("node:a", ["Person"], json!({})))
            .unwrap();
        store.edges.insert(
            "edge:missing".to_string(),
            EdgeRecord {
                id: "edge:missing".to_string(),
                from_id: "node:a".to_string(),
                to_id: "node:missing".to_string(),
                edge_type: "KNOWS".to_string(),
                properties: json!({}),
                version: 9,
                tombstone: false,
                confidence: None,
                epistemic_type: None,
                provenance: None,
            },
        );

        let report = store.rebuild_indexes().unwrap();

        assert_eq!(report.repaired, false);
        assert_eq!(report.after.ok, false);
        assert!(report
            .after
            .problems
            .iter()
            .any(|problem| problem.kind == "missing_to_endpoint"));
    }

    #[test]
    fn graph_store_trait_covers_memory_oracle_contract() {
        fn write_fixture(store: &mut dyn GraphStore) {
            store
                .upsert_node(NodeRecord::new("node:a", ["Fixture"], json!({})))
                .unwrap();
            store
                .upsert_node(NodeRecord::new("node:b", ["Fixture"], json!({})))
                .unwrap();
            store
                .upsert_edge(EdgeRecord::new(
                    "edge:ab",
                    "node:a",
                    "LINKS",
                    "node:b",
                    json!({}),
                ))
                .unwrap();
        }

        let mut store = InMemoryGraphStore::new();
        write_fixture(&mut store);

        assert_eq!(store.stats().nodes_total, 2);
        assert_eq!(store.verify().ok, true);
        assert_eq!(
            store.neighbors(NeighborQuery::out("node:a"))[0].node_id,
            "node:b"
        );
    }

    #[test]
    fn redcore_embedded_store_recovers_nodes_edges_and_indexes_from_aof() {
        let data_dir = unique_test_dir("redcore-aof-recovery");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: false,
        };
        {
            let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:a",
                    ["File"],
                    json!({ "path": "src/lib.rs", "repo": "rusty-red" }),
                ))
                .unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:b",
                    ["File"],
                    json!({ "path": "src/main.rs", "repo": "rusty-red" }),
                ))
                .unwrap();
            store
                .upsert_edge(EdgeRecord::new(
                    "edge:ab",
                    "node:a",
                    "IMPORTS",
                    "node:b",
                    json!({ "rank": 1 }),
                ))
                .unwrap();
            assert_eq!(store.verify().unwrap().ok, true);
        }

        let store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        let hits = store
            .query_nodes(
                NodeQuery::label("File")
                    .with_property("repo", json!("rusty-red"))
                    .with_property("path", json!("src/lib.rs")),
            )
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "node:a");
        assert_eq!(
            store.neighbors(NeighborQuery::out("node:a")).unwrap()[0].node_id,
            "node:b"
        );
        assert_eq!(store.status().recovered_frames, 3);
        assert_eq!(store.verify().unwrap().ok, true);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_batch_commit_is_all_or_nothing() {
        let mut store = RedCoreGraphStore::memory();
        let error = store
            .commit_batch(GraphMutationBatch::new([
                GraphMutation::NodeUpsert(NodeRecord::new("node:a", ["File"], json!({}))),
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
        assert!(store.get_node("node:a").unwrap().is_none());
        assert_eq!(store.status().graph_version, 0);
        assert_eq!(store.status().last_txn_id, 0);

        let transaction = store
            .commit_batch(GraphMutationBatch::new([
                GraphMutation::NodeUpsert(NodeRecord::new("node:a", ["File"], json!({}))),
                GraphMutation::NodeUpsert(NodeRecord::new("node:b", ["File"], json!({}))),
                GraphMutation::EdgeUpsert(EdgeRecord::new(
                    "edge:ab",
                    "node:a",
                    "IMPORTS",
                    "node:b",
                    json!({}),
                )),
            ]))
            .unwrap();

        assert_eq!(transaction.txn_id, 1);
        assert_eq!(transaction.graph_version, 3);
        assert_eq!(transaction.writes.len(), 3);
        assert_eq!(store.status().last_txn_id, 1);
        assert_eq!(store.verify().unwrap().ok, true);
    }

    #[test]
    fn redcore_recovers_batch_commit_from_aof_as_one_transaction() {
        let data_dir = unique_test_dir("redcore-batch-aof-recovery");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: false,
        };
        {
            let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
            let transaction = store
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

            assert_eq!(transaction.txn_id, 1);
            assert_eq!(transaction.graph_version, 3);
            assert_eq!(store.status().last_txn_id, 1);
        }

        let store = RedCoreGraphStore::open(&data_dir, options).unwrap();

        assert_eq!(store.status().recovered_frames, 1);
        assert_eq!(store.status().last_txn_id, 1);
        assert_eq!(store.status().graph_version, 3);
        assert_eq!(
            store.neighbors(NeighborQuery::out("node:a")).unwrap()[0].node_id,
            "node:b"
        );
        assert_eq!(store.verify().unwrap().ok, true);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_failed_aof_append_does_not_publish_staged_mutation() {
        let data_dir = unique_test_dir("redcore-aof-publish-gate");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: false,
        };
        let mut store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        std::fs::create_dir(data_dir.join(super::REDCORE_AOF_FILE)).unwrap();

        let error = store
            .upsert_node(NodeRecord::new(
                "node:blocked",
                ["File"],
                json!({ "path": "blocked.rs" }),
            ))
            .unwrap_err();

        assert_eq!(error.code, "redcore_io_error");
        assert!(store.get_node("node:blocked").unwrap().is_none());
        assert_eq!(store.status().graph_version, 0);
        assert_eq!(store.status().last_txn_id, 0);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_failed_snapshot_write_does_not_publish_staged_mutation() {
        let data_dir = unique_test_dir("redcore-snapshot-publish-gate");
        let options = RedCoreOptions {
            durability: RedCoreDurability::SnapshotOnly,
            snapshot_interval_writes: 1,
            strict_acid: false,
        };
        let mut store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        std::fs::create_dir(data_dir.join("graph.snapshot.current.tmp")).unwrap();

        let error = store
            .upsert_node(NodeRecord::new(
                "node:blocked",
                ["File"],
                json!({ "path": "blocked.rs" }),
            ))
            .unwrap_err();

        assert_eq!(error.code, "redcore_io_error");
        assert!(store.get_node("node:blocked").unwrap().is_none());
        assert_eq!(store.status().graph_version, 0);
        assert_eq!(store.status().last_txn_id, 0);
        assert_eq!(store.status().snapshot_txn_id, 0);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_failed_manifest_write_does_not_publish_staged_mutation() {
        let data_dir = unique_test_dir("redcore-manifest-publish-gate");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: false,
        };
        let mut store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        std::fs::create_dir(data_dir.join("manifest.json.tmp")).unwrap();

        let error = store
            .upsert_node(NodeRecord::new(
                "node:blocked",
                ["File"],
                json!({ "path": "blocked.rs" }),
            ))
            .unwrap_err();

        assert_eq!(error.code, "redcore_io_error");
        assert!(store.get_node("node:blocked").unwrap().is_none());
        assert_eq!(store.status().graph_version, 0);
        assert_eq!(store.status().last_txn_id, 0);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_snapshot_only_recovers_without_redis_or_aof() {
        let data_dir = unique_test_dir("redcore-snapshot-recovery");
        let options = RedCoreOptions {
            durability: RedCoreDurability::SnapshotOnly,
            snapshot_interval_writes: 1,
            strict_acid: false,
        };
        {
            let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:snapshot",
                    ["Snapshot"],
                    json!({ "mode": "snapshot_only" }),
                ))
                .unwrap();
        }

        let store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        assert_eq!(
            store.get_node("node:snapshot").unwrap().unwrap().labels,
            vec!["Snapshot".to_string()]
        );
        assert_eq!(store.status().snapshot_txn_id, 1);
        assert_eq!(store.verify().unwrap().ok, true);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_checked_in_format_fixture_loads_current_read_paths() {
        let data_dir = unique_test_dir("redcore-format-fixture");
        let fixture_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("redcore-v1");
        std::fs::create_dir_all(&data_dir).unwrap();
        for file in ["manifest.json", "graph.snapshot.current"] {
            std::fs::copy(fixture_dir.join(file), data_dir.join(file)).unwrap();
        }

        let options = RedCoreOptions {
            durability: RedCoreDurability::SnapshotOnly,
            snapshot_interval_writes: 10,
            strict_acid: false,
        };
        let mut store = RedCoreGraphStore::open(&data_dir, options).unwrap();

        assert_eq!(super::read_manifest(&data_dir).unwrap().unwrap().version, 1);
        assert_eq!(
            store.get_node("doc:a").unwrap().unwrap().labels,
            vec!["Doc"]
        );
        assert_eq!(
            store
                .query_nodes(NodeQuery {
                    label: Some("Doc".to_string()),
                    properties: std::collections::BTreeMap::new(),
                    limit: Some(10),
                })
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.neighbors(NeighborQuery::out("doc:a")).unwrap()[0].node_id,
            "doc:b"
        );
        assert!(store.verify().unwrap().ok);
        assert!(store.rebuild_indexes().unwrap().after.ok);
        store
            .designate_vector_property("Doc", "embedding", 2)
            .unwrap();
        let vector_hits = store
            .vector_search(Some("Doc"), "embedding", &[1.0, 0.0], 2)
            .unwrap();
        assert_eq!(vector_hits[0].0, "doc:a");

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_strict_acid_requires_aof_always() {
        let data_dir = unique_test_dir("redcore-strict-requires-aof-always");
        let error = RedCoreGraphStore::open(
            &data_dir,
            RedCoreOptions {
                durability: RedCoreDurability::AofEverysec,
                snapshot_interval_writes: 100,
                strict_acid: true,
            },
        )
        .unwrap_err();

        assert_eq!(error.code, "redcore_strict_mode_invalid");
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_open_holds_exclusive_data_directory_lock() {
        let data_dir = unique_test_dir("redcore-file-lock");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: true,
        };
        let _store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
        let error = RedCoreGraphStore::open(&data_dir, options).unwrap_err();

        assert_eq!(error.code, "redcore_lock_unavailable");
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_recovery_truncates_torn_aof_tail() {
        let data_dir = unique_test_dir("redcore-torn-aof-tail");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: true,
        };
        {
            let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:stable",
                    ["File"],
                    json!({ "path": "stable.rs" }),
                ))
                .unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(data_dir.join(super::REDCORE_AOF_FILE))
            .unwrap()
            .write_all(br#"{"magic":"RRGDB_AOF","txn_id":2"#)
            .unwrap();

        let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
        assert!(store.get_node("node:stable").unwrap().is_some());
        assert_eq!(store.status().recovered_frames, 1);
        let aof = std::fs::read_to_string(data_dir.join(super::REDCORE_AOF_FILE)).unwrap();
        assert!(!aof.contains(r#""txn_id":2"#));

        store
            .upsert_node(NodeRecord::new(
                "node:after-recovery",
                ["File"],
                json!({ "path": "after.rs" }),
            ))
            .unwrap();
        drop(store);
        let store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        assert!(store.get_node("node:after-recovery").unwrap().is_some());
        assert_eq!(store.verify().unwrap().ok, true);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_falls_back_to_previous_snapshot_and_replays_aof() {
        let data_dir = unique_test_dir("redcore-previous-snapshot-fallback");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 1,
            strict_acid: true,
        };
        {
            let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:first",
                    ["Snapshot"],
                    json!({ "step": 1 }),
                ))
                .unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "node:second",
                    ["Snapshot"],
                    json!({ "step": 2 }),
                ))
                .unwrap();
        }
        std::fs::write(
            data_dir.join(super::REDCORE_SNAPSHOT_FILE),
            b"{corrupt snapshot",
        )
        .unwrap();

        let store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        assert!(store.get_node("node:first").unwrap().is_some());
        assert!(store.get_node("node:second").unwrap().is_some());
        assert_eq!(store.status().last_txn_id, 2);
        assert_eq!(store.verify().unwrap().ok, true);

        std::fs::remove_dir_all(data_dir).ok();
    }

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{label}-{unique}"))
    }

    #[test]
    fn vector_designate_and_search_returns_nearest() {
        let mut store = InMemoryGraphStore::default();
        store
            .designate_vector_property("Doc", "embedding", 3)
            .unwrap();

        for (id, vec) in [
            ("doc:1", vec![1.0_f32, 0.0, 0.0]),
            ("doc:2", vec![0.0, 1.0, 0.0]),
            ("doc:3", vec![0.7, 0.7, 0.0]),
        ] {
            store
                .upsert_node(NodeRecord::new(id, ["Doc"], json!({ "embedding": vec })))
                .unwrap();
        }

        let results = store
            .vector_search(Some("Doc"), "embedding", &[1.0, 0.0, 0.0], 2)
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "doc:1");
        assert!(
            results[0].1 < 0.01,
            "exact match should have near-zero distance"
        );
        assert_eq!(results[1].0, "doc:3");
    }

    #[test]
    fn vector_search_dimension_mismatch_errors() {
        let mut store = InMemoryGraphStore::default();
        store
            .designate_vector_property("Doc", "embedding", 3)
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "doc:1",
                ["Doc"],
                json!({ "embedding": [1.0, 0.0, 0.0] }),
            ))
            .unwrap();

        let err = store
            .vector_search(Some("Doc"), "embedding", &[1.0, 0.0], 2)
            .unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("dimension"),
            "error should mention dimension: {msg}"
        );
    }

    #[test]
    fn vector_auto_index_on_upsert() {
        let mut store = InMemoryGraphStore::default();
        store
            .designate_vector_property("Doc", "embedding", 2)
            .unwrap();

        store
            .upsert_node(NodeRecord::new(
                "doc:a",
                ["Doc"],
                json!({ "embedding": [1.0, 0.0] }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "doc:b",
                ["Doc"],
                json!({ "embedding": [0.0, 1.0] }),
            ))
            .unwrap();

        let results = store
            .vector_search(Some("Doc"), "embedding", &[0.0, 1.0], 1)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "doc:b");
    }

    #[test]
    fn hybrid_search_blends_vector_and_graph() {
        let mut store = InMemoryGraphStore::default();
        store
            .designate_vector_property("Doc", "embedding", 2)
            .unwrap();

        store
            .upsert_node(NodeRecord::new(
                "doc:a",
                ["Doc"],
                json!({ "embedding": [1.0, 0.0] }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "doc:b",
                ["Doc"],
                json!({ "embedding": [0.9, 0.1] }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "doc:c",
                ["Doc"],
                json!({ "embedding": [0.0, 1.0] }),
            ))
            .unwrap();

        store
            .upsert_edge(EdgeRecord::new("e1", "doc:c", "LINKS", "doc:a", json!({})))
            .unwrap();

        let results = store
            .hybrid_search(
                Some("Doc"),
                "embedding",
                &[1.0, 0.0],
                3,
                &["doc:c".to_string()],
                2,
                0.8,
            )
            .unwrap();

        assert!(!results.is_empty());
        let ids: Vec<&str> = results.iter().map(|r| r.0.as_str()).collect();
        assert!(
            ids.contains(&"doc:a"),
            "doc:a should appear (near vector + reachable from seed)"
        );
    }

    #[test]
    fn hybrid_search_can_penalize_contradicting_edges() {
        let mut store = InMemoryGraphStore::default();
        store
            .designate_vector_property("Claim", "embedding", 2)
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "claim:seed",
                ["Claim"],
                json!({ "embedding": [1.0, 0.0] }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "claim:support",
                ["Claim"],
                json!({ "embedding": [0.95, 0.05] }),
            ))
            .unwrap();
        store
            .upsert_node(NodeRecord::new(
                "claim:against",
                ["Claim"],
                json!({ "embedding": [0.95, 0.05] }),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "edge:support",
                "claim:seed",
                "SUPPORTS",
                "claim:support",
                json!({}),
            ))
            .unwrap();
        store
            .upsert_edge(
                EdgeRecord::new(
                    "edge:against",
                    "claim:seed",
                    "CONTRADICTS",
                    "claim:against",
                    json!({}),
                )
                .with_confidence(1.0),
            )
            .unwrap();

        let results = store
            .hybrid_search(
                Some("Claim"),
                "embedding",
                &[1.0, 0.0],
                3,
                &["claim:seed".to_string()],
                1,
                0.9,
            )
            .unwrap();
        let support = results
            .iter()
            .find(|(id, _)| id == "claim:support")
            .map(|(_, score)| *score)
            .unwrap();
        let against = results
            .iter()
            .find(|(id, _)| id == "claim:against")
            .map(|(_, score)| *score)
            .unwrap();

        assert!(support > against);
    }

    #[test]
    fn vector_designations_list() {
        let mut store = InMemoryGraphStore::default();
        store
            .designate_vector_property("Doc", "embedding", 384)
            .unwrap();
        store
            .designate_vector_property("Claim", "vector", 128)
            .unwrap();

        let desigs = store.vector_designations();
        assert_eq!(desigs.len(), 2);
        let labels: Vec<&str> = desigs.iter().map(|d| d.label.as_str()).collect();
        assert!(labels.contains(&"Doc"));
        assert!(labels.contains(&"Claim"));
    }

    #[test]
    fn redcore_persists_vector_designation_through_reopen() {
        let data_dir = unique_test_dir("vec-aof");
        let options = RedCoreOptions {
            durability: RedCoreDurability::AofAlways,
            snapshot_interval_writes: 100,
            strict_acid: false,
        };

        {
            let mut store = RedCoreGraphStore::open(&data_dir, options.clone()).unwrap();
            store
                .designate_vector_property("Doc", "embedding", 3)
                .unwrap();
            store
                .upsert_node(NodeRecord::new(
                    "doc:1",
                    ["Doc"],
                    json!({ "embedding": [1.0, 0.0, 0.0] }),
                ))
                .unwrap();
        }

        {
            let store = RedCoreGraphStore::open(&data_dir, options).unwrap();
            let desigs = store.vector_designations();
            assert_eq!(desigs.len(), 1);
            assert_eq!(desigs[0].label, "Doc");
            assert_eq!(desigs[0].dimension, 3);

            let results = store
                .vector_search(Some("Doc"), "embedding", &[1.0, 0.0, 0.0], 1)
                .unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].0, "doc:1");
        }

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[cfg(feature = "redis-store")]
    #[test]
    fn redis_keyspace_uses_tenant_hash_tagged_graph_keys() {
        let prefix = super::RedisGraphKeyspace::tenant_prefix("rrgdb", "Tenant One!");
        let keyspace = super::RedisGraphKeyspace::new(prefix);

        assert_eq!(keyspace.prefix(), "rrgdb:{tenant:TenantOne}:graph:v1");
        assert_eq!(
            keyspace.node("node:a"),
            "rrgdb:{tenant:TenantOne}:graph:v1:node:h6e6f64653a61"
        );
        assert_eq!(
            keyspace.out_adjacency("node:a", "LINKS"),
            "rrgdb:{tenant:TenantOne}:graph:v1:adj:out:h6e6f64653a61:h4c494e4b53"
        );
        assert_eq!(
            keyspace.property_value("path", "\"src/lib.rs\""),
            "rrgdb:{tenant:TenantOne}:graph:v1:property:h70617468:h227372632f6c69622e727322"
        );
        assert_eq!(
            keyspace.events(),
            "rrgdb:{tenant:TenantOne}:graph:v1:events"
        );
    }

    #[test]
    fn redcore_manifest_records_format_kind_and_crate_version() {
        let data_dir = unique_test_dir("redcore-manifest-format");
        let options = RedCoreOptions {
            durability: RedCoreDurability::None,
            snapshot_interval_writes: 1,
            strict_acid: false,
        };
        let mut store = RedCoreGraphStore::open(&data_dir, options).unwrap();
        let node = NodeRecord::new("n1", ["Doc"], json!({"title": "x"}));
        store.upsert_node(node).unwrap();
        store.snapshot_now().unwrap();
        drop(store);

        let manifest = super::read_manifest(&data_dir).unwrap().unwrap();
        assert_eq!(manifest.version, super::CURRENT_FORMAT_VERSION);
        assert_eq!(manifest.format_kind, "redcore");
        assert!(!manifest.crate_version.is_empty());
        assert!(manifest.created_at_unix_ms > 0);

        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_refuses_too_new_manifest() {
        let data_dir = unique_test_dir("redcore-manifest-too-new");
        std::fs::create_dir_all(&data_dir).unwrap();
        let manifest_path = data_dir.join("manifest.json");
        std::fs::write(
            &manifest_path,
            r#"{
                "version": 9999,
                "graph_version": 0,
                "last_txn_id": 0,
                "snapshot_txn_id": 0,
                "durability": "none",
                "snapshot_file": "graph.snapshot.current",
                "aof_file": "graph.aof",
                "updated_at_unix_ms": 0
            }"#,
        )
        .unwrap();

        let result = RedCoreGraphStore::open(
            &data_dir,
            RedCoreOptions {
                durability: RedCoreDurability::None,
                snapshot_interval_writes: 0,
                strict_acid: false,
            },
        );

        let error = result.unwrap_err();
        assert_eq!(error.code, "redcore_format_too_new");
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn redcore_legacy_manifest_without_format_kind_loads() {
        let data_dir = unique_test_dir("redcore-manifest-legacy");
        std::fs::create_dir_all(&data_dir).unwrap();
        let manifest_path = data_dir.join("manifest.json");
        // Legacy: no format_kind, crate_version, created_at_unix_ms.
        std::fs::write(
            &manifest_path,
            r#"{
                "version": 1,
                "graph_version": 0,
                "last_txn_id": 0,
                "snapshot_txn_id": 0,
                "durability": "none",
                "snapshot_file": "graph.snapshot.current",
                "aof_file": "graph.aof",
                "updated_at_unix_ms": 0
            }"#,
        )
        .unwrap();

        let store = RedCoreGraphStore::open(
            &data_dir,
            RedCoreOptions {
                durability: RedCoreDurability::None,
                snapshot_interval_writes: 0,
                strict_acid: false,
            },
        )
        .unwrap();
        drop(store);
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[cfg(feature = "redis-store")]
    #[test]
    fn redis_keyspace_normalizes_tenants_and_encodes_dynamic_key_segments() {
        assert_eq!(
            super::RedisGraphKeyspace::tenant_prefix("rrgdb", "Tenant.One!"),
            "rrgdb:{tenant:TenantOne}:graph:v1"
        );

        let keyspace = super::RedisGraphKeyspace::new("rrgdb:{tenant:T}:graph:v1");
        assert_ne!(
            keyspace.out_adjacency("a:b", "c"),
            keyspace.out_adjacency("a", "b:c")
        );
        assert_ne!(
            keyspace.in_adjacency("a:b", "c"),
            keyspace.in_adjacency("a", "b:c")
        );
    }
}
