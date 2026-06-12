//! Operation contract for graph algorithms: execution modes (stream / stats /
//! mutate) plus a pre-run memory estimate, and the storage-agnostic graph view
//! the algorithm operations run against.
//!
//! This is the seam the algorithms plugin registers through. The pure algorithm
//! math lives in [`crate::algorithms`]; the operation wrappers that pick a mode,
//! read the graph, and (for `mutate`) write results back live in
//! [`crate::algorithm_ops`] and run against the [`AlgorithmGraph`] trait so one
//! operation serves the in-core executor, the MCP backend, and the HTTP tenant
//! store without per-surface algorithm code.
//!
//! Modes mirror the Neo4j GDS operation contract by name only (`stream`,
//! `stats`, `mutate`, plus an `estimate` companion). The contract is mined from
//! the GDS *API map*; no GDS code is used. Implementations come from the
//! published papers (see [`crate::algorithms`]).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::errors::RustyredError;
use crate::graph_store::{EdgeRecord, GraphStoreError, GraphStoreResult, NodeRecord};

/// Execution mode for an algorithm operation.
///
/// - `Stream` returns per-node (or per-pair) rows.
/// - `Stats` returns aggregate summaries only.
/// - `Mutate` writes results back onto the graph (node properties such as
///   `pagerank` / `community_id` / `betweenness`, or typed edges such as
///   `SIMILAR_TO`) so workflows compose.
/// - `Estimate` is the pre-run companion that returns memory bounds without
///   touching the graph. RAM-first storage makes estimate-before-run a gate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationMode {
    #[default]
    Stream,
    Stats,
    Mutate,
    Estimate,
}

impl OperationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationMode::Stream => "stream",
            OperationMode::Stats => "stats",
            OperationMode::Mutate => "mutate",
            OperationMode::Estimate => "estimate",
        }
    }

    /// Parse the requested mode from a request's `mode` argument, defaulting to
    /// `stream`. An unrecognized value is an error rather than a silent
    /// fallback, so a typo never quietly writes to (or skips) the graph.
    pub fn from_args(args: &Value) -> Result<OperationMode, OperationError> {
        match args.get("mode") {
            None => Ok(OperationMode::Stream),
            Some(Value::Null) => Ok(OperationMode::Stream),
            Some(Value::String(raw)) => OperationMode::parse(raw).ok_or_else(|| {
                OperationError::invalid_params(format!(
                    "unknown mode {raw:?}; expected stream | stats | mutate | estimate"
                ))
            }),
            Some(other) => Err(OperationError::invalid_params(format!(
                "mode must be a string, got {other}"
            ))),
        }
    }

    pub fn parse(raw: &str) -> Option<OperationMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "stream" => Some(OperationMode::Stream),
            "stats" => Some(OperationMode::Stats),
            "mutate" => Some(OperationMode::Mutate),
            "estimate" => Some(OperationMode::Estimate),
            _ => None,
        }
    }
}

/// Node and relationship counts an estimate is computed from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct GraphCounts {
    pub node_count: usize,
    pub relationship_count: usize,
}

/// Memory bounds for an operation over a graph of a given size.
///
/// Bytes are an analytic bound on the working set each algorithm holds
/// (adjacency, score vectors, DFS stacks), not a measurement. `required_memory`
/// and `tree_view` are human summaries; `bytes_min`/`bytes_max` are the machine
/// bound a caller gates on.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct MemoryEstimate {
    pub node_count: usize,
    pub relationship_count: usize,
    pub bytes_min: u64,
    pub bytes_max: u64,
    pub required_memory: String,
    pub tree_view: String,
}

impl MemoryEstimate {
    pub fn new(
        counts: GraphCounts,
        bytes_min: u64,
        bytes_max: u64,
        tree_view: impl Into<String>,
    ) -> Self {
        Self {
            node_count: counts.node_count,
            relationship_count: counts.relationship_count,
            required_memory: fmt_byte_range(bytes_min, bytes_max),
            bytes_min,
            bytes_max,
            tree_view: tree_view.into(),
        }
    }
}

/// Bytes-per-node and bytes-per-relationship working-set coefficients, plus a
/// fixed overhead. Each operation declares its own coefficients; the estimate
/// is `overhead + per_node*N + per_rel*M`. `min` uses the lower coefficient
/// pair (stream), `max` the higher (mutate also materializes a write set).
pub fn estimate_from_coefficients(
    counts: GraphCounts,
    overhead: u64,
    per_node_min: u64,
    per_node_max: u64,
    per_rel: u64,
    label: &str,
) -> MemoryEstimate {
    let n = counts.node_count as u64;
    let m = counts.relationship_count as u64;
    let bytes_min = overhead
        .saturating_add(per_node_min.saturating_mul(n))
        .saturating_add(per_rel.saturating_mul(m));
    let bytes_max = overhead
        .saturating_add(per_node_max.saturating_mul(n))
        .saturating_add(per_rel.saturating_mul(m));
    let tree = format!(
        "{label}: {} .. {} ({} nodes, {} rels)",
        fmt_bytes(bytes_min),
        fmt_bytes(bytes_max),
        n,
        m
    );
    MemoryEstimate::new(counts, bytes_min, bytes_max, tree)
}

fn fmt_byte_range(min: u64, max: u64) -> String {
    if min == max {
        fmt_bytes(min)
    } else {
        format!("[{} ... {}]", fmt_bytes(min), fmt_bytes(max))
    }
}

fn fmt_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} Bytes")
    }
}

/// Error from an algorithm operation. Carries a stable `code` so adapters can
/// map it onto their own error envelopes (`RustyredError`, MCP `invalid_params`,
/// HTTP status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationError {
    pub code: String,
    pub message: String,
}

impl OperationError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new("invalid_params", message)
    }

    pub fn unsupported_mode(command: &str, mode: OperationMode) -> Self {
        Self::new(
            "unsupported_mode",
            format!(
                "operation {command} does not support mode {}",
                mode.as_str()
            ),
        )
    }
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for OperationError {}

impl From<GraphStoreError> for OperationError {
    fn from(error: GraphStoreError) -> Self {
        Self::new(error.code, error.message)
    }
}

impl From<OperationError> for RustyredError {
    fn from(error: OperationError) -> Self {
        RustyredError::new(error.code, error.message)
    }
}

/// A storage-agnostic view of the graph an algorithm operation runs against.
///
/// Implemented by [`crate::graph_store::InMemoryGraphStore`] here in core; the
/// MCP and HTTP surfaces implement it over their own backends so the same
/// operation runs everywhere. Read methods feed the pure algorithms; write
/// methods back `mutate` mode.
pub trait AlgorithmGraph {
    /// Total live node and relationship counts (for the estimate).
    fn graph_counts(&self) -> GraphStoreResult<GraphCounts>;

    /// Every live (non-tombstoned) edge. Algorithms build their own adjacency.
    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>>;

    /// Live nodes carrying `label` (label-scoped operations: KNN, node
    /// similarity by label).
    fn nodes_with_label(&self, label: &str) -> GraphStoreResult<Vec<NodeRecord>>;

    /// A single node by id, owned.
    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>>;

    /// Top-`k` neighbors of `query` under the designated vector index, returned
    /// as `(node_id, cosine_similarity)` highest-first. Similarity is
    /// `1 - distance` (the index stores L2-normalized vectors, so cosine).
    fn vector_top_k(
        &self,
        label: &str,
        property: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>>;

    /// Read-modify-write a single property onto a node (mutate mode).
    fn write_node_property(&mut self, id: &str, key: &str, value: Value) -> GraphStoreResult<()>;

    /// Upsert a typed edge (mutate mode). Setting `tombstone` retracts one.
    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()>;
}

/// A registered algorithm operation: its command name, the modes it supports,
/// an input schema for tool/route generation, a memory estimate, and the run
/// body.
pub trait AlgorithmOperation: Send + Sync + std::fmt::Debug {
    /// Fully-qualified command, e.g. `rustyred.algorithm.leiden`. This is the
    /// MCP tool name and the dispatch key.
    fn command(&self) -> &'static str;

    /// Short segment used for the HTTP route and as a human handle, e.g.
    /// `leiden`. Must be URL-safe and unique.
    fn name(&self) -> &'static str;

    /// One-line description for tool/route discovery.
    fn summary(&self) -> &'static str;

    /// Modes this operation supports (always includes the ones it implements;
    /// `Estimate` is handled by the contract and need not be listed).
    fn modes(&self) -> &'static [OperationMode];

    /// JSON-schema-ish description of accepted arguments, for MCP `inputSchema`
    /// and OpenAPI generation.
    fn input_schema(&self) -> Value;

    /// Memory bounds for this operation over a graph of size `counts`.
    fn estimate(&self, counts: GraphCounts, args: &Value) -> MemoryEstimate;

    /// Run the operation in `mode` against `graph`, returning the result
    /// payload. For `Mutate`, perform writes via `graph` and return a summary.
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError>;
}

/// Run an operation end to end from a JSON argument object: parse the mode,
/// short-circuit `estimate`, reject unsupported modes, and dispatch. This is the
/// single entry point every adapter (executor, MCP, HTTP) calls, so mode and
/// estimate semantics are identical across surfaces.
pub fn dispatch_operation(
    op: &dyn AlgorithmOperation,
    graph: &mut dyn AlgorithmGraph,
    args: &Value,
) -> Result<Value, OperationError> {
    let mode = OperationMode::from_args(args)?;
    if mode == OperationMode::Estimate {
        let counts = graph.graph_counts()?;
        let estimate = op.estimate(counts, args);
        return Ok(json!({
            "operation": op.command(),
            "mode": "estimate",
            "estimate": estimate,
        }));
    }
    if !op.modes().contains(&mode) {
        return Err(OperationError::unsupported_mode(op.command(), mode));
    }
    op.run(graph, mode, args)
}

// ===== Argument helpers shared by the operation wrappers =====

pub fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub fn arg_f64(args: &Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(Value::as_f64).unwrap_or(default)
}

pub fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

pub fn arg_u64(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

pub fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Required string argument, erroring with a clear message when absent.
pub fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, OperationError> {
    arg_str(args, key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| OperationError::invalid_params(format!("missing required argument {key:?}")))
}

// ===== In-core graph view: InMemoryGraphStore =====

impl AlgorithmGraph for crate::graph_store::InMemoryGraphStore {
    fn graph_counts(&self) -> GraphStoreResult<GraphCounts> {
        let stats = crate::graph_store::GraphStore::stats(self);
        Ok(GraphCounts {
            node_count: stats.nodes_total,
            relationship_count: stats.edges_total,
        })
    }

    fn list_edges(&self) -> GraphStoreResult<Vec<EdgeRecord>> {
        Ok(self
            .snapshot()
            .edges
            .into_iter()
            .filter(|edge| !edge.tombstone)
            .collect())
    }

    fn nodes_with_label(&self, label: &str) -> GraphStoreResult<Vec<NodeRecord>> {
        Ok(crate::graph_store::GraphStore::query_nodes(
            self,
            crate::graph_store::NodeQuery::label(label),
        ))
    }

    fn get_node(&self, id: &str) -> GraphStoreResult<Option<NodeRecord>> {
        Ok(crate::graph_store::GraphStore::get_node(self, id).cloned())
    }

    fn vector_top_k(
        &self,
        label: &str,
        property: &str,
        query: &[f32],
        k: usize,
    ) -> GraphStoreResult<Vec<(String, f32)>> {
        let raw = self.vector_search(Some(label), property, query, k)?;
        Ok(raw
            .into_iter()
            .map(|(id, distance)| (id, 1.0 - distance))
            .collect())
    }

    fn write_node_property(&mut self, id: &str, key: &str, value: Value) -> GraphStoreResult<()> {
        let mut node = crate::graph_store::GraphStore::get_node(self, id)
            .cloned()
            .ok_or_else(|| {
                GraphStoreError::new("node_not_found", format!("no node with id {id}"))
            })?;
        set_property(&mut node.properties, key, value);
        crate::graph_store::GraphStore::upsert_node(self, node).map(|_| ())
    }

    fn upsert_edge(&mut self, edge: EdgeRecord) -> GraphStoreResult<()> {
        crate::graph_store::GraphStore::upsert_edge(self, edge).map(|_| ())
    }
}

/// Insert/overwrite a single property key on a node's property object, creating
/// the object if the node had none.
pub fn set_property(properties: &mut Value, key: &str, value: Value) {
    if !properties.is_object() {
        *properties = Value::Object(serde_json::Map::new());
    }
    if let Some(object) = properties.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mode_parses_and_defaults_to_stream() {
        assert_eq!(
            OperationMode::from_args(&json!({})).unwrap(),
            OperationMode::Stream
        );
        assert_eq!(
            OperationMode::from_args(&json!({"mode": "mutate"})).unwrap(),
            OperationMode::Mutate
        );
        assert_eq!(
            OperationMode::from_args(&json!({"mode": "ESTIMATE"})).unwrap(),
            OperationMode::Estimate
        );
        assert!(OperationMode::from_args(&json!({"mode": "destroy"})).is_err());
    }

    #[test]
    fn estimate_scales_with_counts() {
        let counts = GraphCounts {
            node_count: 1_000,
            relationship_count: 4_000,
        };
        let estimate = estimate_from_coefficients(counts, 1024, 16, 48, 24, "test");
        assert_eq!(estimate.node_count, 1_000);
        assert_eq!(estimate.relationship_count, 4_000);
        assert!(estimate.bytes_min < estimate.bytes_max);
        // min = 1024 + 16*1000 + 24*4000 = 113_024
        assert_eq!(estimate.bytes_min, 1024 + 16 * 1000 + 24 * 4000);
        assert_eq!(estimate.bytes_max, 1024 + 48 * 1000 + 24 * 4000);
    }

    #[test]
    fn set_property_creates_object_when_missing() {
        let mut props = Value::Null;
        set_property(&mut props, "pagerank", json!(0.25));
        assert_eq!(props["pagerank"], json!(0.25));
    }
}
