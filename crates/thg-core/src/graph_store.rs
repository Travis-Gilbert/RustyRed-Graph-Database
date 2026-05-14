use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::stable_hash;

pub type GraphStoreResult<T> = Result<T, GraphStoreError>;

pub trait GraphStore {
    fn upsert_node(&mut self, node: NodeRecord) -> GraphStoreResult<GraphWriteResult>;
    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult>;
    fn get_node(&self, id: &str) -> Option<&NodeRecord>;
    fn get_edge(&self, id: &str) -> Option<&EdgeRecord>;
    fn query_nodes(&self, query: NodeQuery) -> Vec<NodeRecord>;
    fn neighbors(&self, query: NeighborQuery) -> Vec<NeighborHit>;
    fn stats(&self) -> GraphStats;
    fn verify(&self) -> VerifyReport;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphStoreError {
    pub code: String,
    pub message: String,
}

impl GraphStoreError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EdgeRecord {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub properties: Value,
    pub version: u64,
    pub tombstone: bool,
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
        }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NeighborHit {
    pub edge_id: String,
    pub node_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphStats {
    pub version: u64,
    pub nodes_total: usize,
    pub edges_total: usize,
    pub labels_total: usize,
    pub edge_types_total: usize,
    pub property_keys_total: usize,
    pub property_indexes_total: usize,
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

#[derive(Clone, Debug, Default)]
pub struct InMemoryGraphStore {
    version: u64,
    nodes: BTreeMap<String, NodeRecord>,
    edges: BTreeMap<String, EdgeRecord>,
    out_adjacency: BTreeMap<(String, String), BTreeSet<String>>,
    in_adjacency: BTreeMap<(String, String), BTreeSet<String>>,
    label_index: BTreeMap<String, BTreeSet<String>>,
    edge_type_index: BTreeMap<String, BTreeSet<String>>,
    property_index: BTreeMap<(String, String), BTreeSet<String>>,
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self::default()
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
        }
        self.nodes.insert(id.clone(), node);

        Ok(GraphWriteResult {
            id,
            version: self.version,
            checksum,
        })
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
            });
        }
        hits
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
        }
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
        let existing = self.load_node_raw(&node.id)?;
        let mut connection = self.connection()?;
        let version = redis::cmd("INCR")
            .arg(self.keyspace.version())
            .query::<u64>(&mut connection)?;
        node.version = version;
        let checksum = node.checksum();
        let raw = serde_json::to_string(&node)
            .map_err(|err| GraphStoreError::invalid_record("node", &node.id, err))?;
        let event = graph_event("node.upsert", &node.id, version, &checksum)?;

        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SET")
            .arg(self.keyspace.node(&node.id))
            .arg(raw)
            .ignore()
            .cmd("SADD")
            .arg(self.keyspace.nodes())
            .arg(&node.id)
            .ignore()
            .cmd("RPUSH")
            .arg(self.keyspace.events())
            .arg(event)
            .ignore();
        if let Some(existing) = existing.as_ref() {
            for label in &existing.labels {
                pipe.cmd("SREM")
                    .arg(self.keyspace.label(label))
                    .arg(&existing.id)
                    .ignore();
            }
            remove_node_from_redis_property_indexes(&mut pipe, &self.keyspace, existing);
        }
        if !node.tombstone {
            for label in &node.labels {
                pipe.cmd("SADD")
                    .arg(self.keyspace.labels())
                    .arg(label)
                    .ignore()
                    .cmd("SADD")
                    .arg(self.keyspace.label(label))
                    .arg(&node.id)
                    .ignore();
            }
            add_node_to_redis_property_indexes(&mut pipe, &self.keyspace, &node);
        }
        pipe.query::<()>(&mut connection)?;

        if let Some(existing) = existing {
            self.cleanup_empty_labels(&mut connection, &existing.labels)?;
            self.cleanup_empty_properties(
                &mut connection,
                &indexed_properties(&existing.properties),
            )?;
        }

        Ok(GraphWriteResult {
            id: node.id,
            version,
            checksum,
        })
    }

    pub fn upsert_edge(&mut self, mut edge: EdgeRecord) -> GraphStoreResult<GraphWriteResult> {
        validate_edge_shape(&edge)?;
        self.require_live_endpoint(&edge, "from", &edge.from_id)?;
        self.require_live_endpoint(&edge, "to", &edge.to_id)?;

        let existing = self.load_edge_raw(&edge.id)?;
        let mut connection = self.connection()?;
        let version = redis::cmd("INCR")
            .arg(self.keyspace.version())
            .query::<u64>(&mut connection)?;
        edge.version = version;
        let checksum = edge.checksum();
        let raw = serde_json::to_string(&edge)
            .map_err(|err| GraphStoreError::invalid_record("edge", &edge.id, err))?;
        let event = graph_event("edge.upsert", &edge.id, version, &checksum)?;

        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SET")
            .arg(self.keyspace.edge(&edge.id))
            .arg(raw)
            .ignore()
            .cmd("SADD")
            .arg(self.keyspace.edges())
            .arg(&edge.id)
            .ignore()
            .cmd("RPUSH")
            .arg(self.keyspace.events())
            .arg(event)
            .ignore();
        if let Some(existing) = existing.as_ref() {
            remove_edge_from_redis_indexes(&mut pipe, &self.keyspace, existing);
        }
        if !edge.tombstone {
            add_edge_to_redis_indexes(&mut pipe, &self.keyspace, &edge);
        }
        pipe.query::<()>(&mut connection)?;

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

        Ok(GraphWriteResult {
            id: edge.id,
            version,
            checksum,
        })
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
                edge_id: edge.id,
                node_id,
                edge_type: edge.edge_type,
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

    fn connection(&self) -> GraphStoreResult<redis::Connection> {
        Ok(self.client.get_connection()?)
    }

    fn load_node_raw(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        let mut connection = self.connection()?;
        let raw = redis::cmd("GET")
            .arg(self.keyspace.node(id))
            .query::<Option<String>>(&mut connection)?;
        raw.map(|value| {
            serde_json::from_str::<NodeRecord>(&value)
                .map_err(|err| GraphStoreError::invalid_record("node", id, err))
        })
        .transpose()
    }

    fn load_edge_raw(&self, id: &str) -> GraphStoreResult<Option<EdgeRecord>> {
        let mut connection = self.connection()?;
        let raw = redis::cmd("GET")
            .arg(self.keyspace.edge(id))
            .query::<Option<String>>(&mut connection)?;
        raw.map(|value| {
            serde_json::from_str::<EdgeRecord>(&value)
                .map_err(|err| GraphStoreError::invalid_record("edge", id, err))
        })
        .transpose()
    }

    fn require_live_endpoint(
        &self,
        edge: &EdgeRecord,
        endpoint: &str,
        node_id: &str,
    ) -> GraphStoreResult<()> {
        let Some(node) = self.load_node_raw(node_id)? else {
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

    fn live_nodes(&self) -> GraphStoreResult<BTreeMap<String, NodeRecord>> {
        let mut connection = self.connection()?;
        let node_ids = redis_string_set(&mut connection, self.keyspace.nodes())?;
        let mut nodes = BTreeMap::new();
        for node_id in node_ids {
            if let Some(node) = self.get_node(&node_id)? {
                nodes.insert(node_id, node);
            }
        }
        Ok(nodes)
    }

    fn live_edges(&self) -> GraphStoreResult<BTreeMap<String, EdgeRecord>> {
        let mut connection = self.connection()?;
        let edge_ids = redis_string_set(&mut connection, self.keyspace.edges())?;
        let mut edges = BTreeMap::new();
        for edge_id in edge_ids {
            if let Some(edge) = self.get_edge(&edge_id)? {
                edges.insert(edge_id, edge);
            }
        }
        Ok(edges)
    }

    fn redis_indexes(&self) -> GraphStoreResult<ExpectedIndexes> {
        let mut connection = self.connection()?;
        let mut indexes = ExpectedIndexes::default();
        for label in redis_string_set(&mut connection, self.keyspace.labels())? {
            let node_ids = redis_string_set(&mut connection, self.keyspace.label(&label))?;
            if !node_ids.is_empty() {
                indexes.label_index.insert(label, node_ids);
            }
        }
        for edge_type in redis_string_set(&mut connection, self.keyspace.edge_types())? {
            let edge_ids = redis_string_set(&mut connection, self.keyspace.edge_type(&edge_type))?;
            if !edge_ids.is_empty() {
                indexes.edge_type_index.insert(edge_type, edge_ids);
            }
        }
        for entry in redis_string_set(&mut connection, self.keyspace.property_index_entries())? {
            let Some((key, token)) = decode_property_pair(&entry) else {
                continue;
            };
            let node_ids =
                redis_string_set(&mut connection, self.keyspace.property_value(&key, &token))?;
            if !node_ids.is_empty() {
                indexes.property_index.insert((key, token), node_ids);
            }
        }
        for pair in redis_string_set(&mut connection, self.keyspace.out_adjacency_pairs())? {
            let Some((node_id, edge_type)) = decode_adjacency_pair(&pair) else {
                continue;
            };
            let edge_ids = redis_string_set(
                &mut connection,
                self.keyspace.out_adjacency(&node_id, &edge_type),
            )?;
            if !edge_ids.is_empty() {
                indexes.out_adjacency.insert((node_id, edge_type), edge_ids);
            }
        }
        for pair in redis_string_set(&mut connection, self.keyspace.in_adjacency_pairs())? {
            let Some((node_id, edge_type)) = decode_adjacency_pair(&pair) else {
                continue;
            };
            let edge_ids = redis_string_set(
                &mut connection,
                self.keyspace.in_adjacency(&node_id, &edge_type),
            )?;
            if !edge_ids.is_empty() {
                indexes.in_adjacency.insert((node_id, edge_type), edge_ids);
            }
        }
        Ok(indexes)
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
    use serde_json::json;

    use super::{
        Direction, EdgeRecord, GraphStore, InMemoryGraphStore, NeighborQuery, NodeQuery, NodeRecord,
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
