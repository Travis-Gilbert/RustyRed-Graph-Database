//! Algorithm operations: the registered [`AlgorithmOperation`] wrappers that
//! adopt the mode-plus-estimate contract over the pure math in
//! [`crate::algorithms`] and the existing helpers in [`crate::graph`].
//!
//! Each operation declares its modes, an input schema, a memory estimate, and a
//! `run` body that reads the graph via [`AlgorithmGraph`] and, for `mutate`,
//! writes results back (node properties or typed edges). Registering one here
//! surfaces it through `execute_request_json`, the MCP tool generation, and the
//! HTTP route generation with no per-surface code.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::algorithms::{
    adamic_adar, articulation_points_and_bridges, betweenness_centrality,
    betweenness_centrality_sampled, common_neighbors, leiden, neighbor_sets, node_similarity,
    resource_allocation, strongly_connected_components, topological_sort,
    topological_sort_condensation, SimilarityMetric,
};
use crate::graph::{
    connected_components, expand_bounded, expand_bounded_weighted, label_propagation_communities,
    pagerank, paths_shortest, paths_shortest_weighted, personalized_pagerank, EdgeTuple,
};
use crate::graph_store::EdgeRecord;
use crate::morphology::{
    default_relation_weights, is_morphological_relation, morphological_edges_from_records,
    morphology_stats, MorphologicalEdge,
};
use crate::operation::{
    arg_bool, arg_f64, arg_str, arg_u64, arg_usize, estimate_from_coefficients, require_str,
    AlgorithmGraph, AlgorithmOperation, GraphCounts, MemoryEstimate, OperationError, OperationMode,
};
use crate::plugin::{PluginCapability, PluginCapabilityKind, RustyRedPlugin};

const STREAM_STATS_MUTATE: &[OperationMode] = &[
    OperationMode::Stream,
    OperationMode::Stats,
    OperationMode::Mutate,
];
const STREAM_MUTATE: &[OperationMode] = &[OperationMode::Stream, OperationMode::Mutate];
const STREAM_STATS: &[OperationMode] = &[OperationMode::Stream, OperationMode::Stats];
const STREAM_ONLY: &[OperationMode] = &[OperationMode::Stream];

/// Every builtin algorithm operation, in registration order. The first six are
/// the tier-1 deliverables; `link_prediction` completes deliverable 6; the rest
/// conform the pre-existing algorithms to the operation contract.
pub fn algorithm_operations() -> Vec<Arc<dyn AlgorithmOperation>> {
    vec![
        Arc::new(PageRankOp),
        Arc::new(SimilarityKnnOp),
        Arc::new(LeidenOp),
        Arc::new(BetweennessOp),
        Arc::new(SccOp),
        Arc::new(NodeSimilarityOp),
        Arc::new(LinkPredictionOp),
        Arc::new(MorphologicalMessagePassingOp),
        Arc::new(PersonalizedPageRankOp),
        Arc::new(ConnectedComponentsOp),
        Arc::new(LabelPropagationOp),
        Arc::new(ShortestPathOp),
        Arc::new(ExpandOp),
    ]
}

/// The builtin plugin that contributes all tier-1 graph algorithm operations.
/// Registered in `with_builtin_plugins`, it is the single seam new algorithms
/// register through.
#[derive(Clone, Copy, Debug)]
pub struct AlgorithmsPlugin;

impl RustyRedPlugin for AlgorithmsPlugin {
    fn name(&self) -> &'static str {
        "rustyred.algorithms"
    }

    fn capabilities(&self) -> Vec<PluginCapability> {
        algorithm_operations()
            .iter()
            .map(|operation| PluginCapability {
                kind: PluginCapabilityKind::Operation,
                name: operation.command().to_string(),
            })
            .collect()
    }

    fn algorithm_operations(&self) -> Vec<Arc<dyn AlgorithmOperation>> {
        algorithm_operations()
    }
}

// ===== shared payload helpers =====

fn sorted_scores(map: HashMap<String, f64>, top_k: Option<usize>) -> Vec<(String, f64)> {
    let mut entries: Vec<(String, f64)> = map.into_iter().collect();
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(k) = top_k {
        entries.truncate(k);
    }
    entries
}

fn score_rows(entries: &[(String, f64)], score_key: &str) -> Vec<Value> {
    entries
        .iter()
        .map(|(node_id, score)| json!({ "node_id": node_id, score_key: score }))
        .collect()
}

fn f64_stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({ "count": 0 });
    }
    let count = values.len();
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for &v in values {
        min = min.min(v);
        max = max.max(v);
        sum += v;
    }
    json!({
        "count": count,
        "min": min,
        "max": max,
        "mean": sum / count as f64,
        "sum": sum,
    })
}

fn write_scores(
    graph: &mut dyn AlgorithmGraph,
    entries: &[(String, f64)],
    property: &str,
) -> Result<usize, OperationError> {
    let mut written = 0;
    for (node_id, score) in entries {
        graph.write_node_property(node_id, property, json!(score))?;
        written += 1;
    }
    Ok(written)
}

fn read_vector(properties: &Value, key: &str) -> Option<Vec<f32>> {
    properties
        .get(key)?
        .as_array()?
        .iter()
        .map(|value| value.as_f64().map(|x| x as f32))
        .collect()
}

fn adjacency_from_graph(edges: &[EdgeRecord]) -> HashMap<String, Vec<(String, f64)>> {
    let mut adjacency: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    for edge in edges.iter().filter(|edge| !edge.tombstone) {
        adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .push((edge.to_id.clone(), edge.effective_confidence()));
    }
    adjacency
}

// ===== Deliverable 1: PageRank (the mode-plus-estimate proof) =====

#[derive(Clone, Copy, Debug)]
pub struct PageRankOp;

impl AlgorithmOperation for PageRankOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.pagerank"
    }
    fn name(&self) -> &'static str {
        "pagerank"
    }
    fn summary(&self) -> &'static str {
        "Power-iteration PageRank over the tenant graph (stream | stats | mutate)."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "damping": { "type": "number", "default": 0.85 },
                "max_iter": { "type": "integer", "default": 100 },
                "tolerance": { "type": "number", "default": 1e-6 },
                "top_k": { "type": "integer" },
                "mutate_property": { "type": "string", "default": "pagerank" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        // Two score vectors + out-links adjacency.
        estimate_from_coefficients(counts, 4096, 48, 96, 24, "PageRank")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let damping = arg_f64(args, "damping", 0.85);
        let max_iter = arg_usize(args, "max_iter", 100);
        let tolerance = arg_f64(args, "tolerance", 1e-6);
        let scores = pagerank(&edges, damping, max_iter, tolerance);
        match mode {
            OperationMode::Stream => {
                let top_k = args
                    .get("top_k")
                    .and_then(Value::as_u64)
                    .map(|k| k as usize);
                let entries = sorted_scores(scores, top_k);
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "damping": damping,
                    "node_count": entries.len(),
                    "scores": score_rows(&entries, "score"),
                }))
            }
            OperationMode::Stats => {
                let values: Vec<f64> = scores.values().copied().collect();
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stats",
                    "damping": damping,
                    "stats": f64_stats(&values),
                }))
            }
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("pagerank");
                let entries = sorted_scores(scores, None);
                let written = write_scores(graph, &entries, property)?;
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

// ===== Deliverable 2: KNN similarity-graph materializer =====

#[derive(Clone, Copy, Debug)]
pub struct SimilarityKnnOp;

impl AlgorithmOperation for SimilarityKnnOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.similarity_knn"
    }
    fn name(&self) -> &'static str {
        "similarity_knn"
    }
    fn summary(&self) -> &'static str {
        "K-nearest-neighbor similarity over the HNSW vector index; mutate materializes typed similarity edges."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["label", "vector_property"],
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "mutate", "estimate"], "default": "stream" },
                "label": { "type": "string" },
                "vector_property": { "type": "string" },
                "k": { "type": "integer", "default": 10 },
                "cutoff": { "type": "number", "default": 0.0, "description": "minimum cosine similarity" },
                "edge_type": { "type": "string", "default": "SIMILAR_TO" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, args: &Value) -> MemoryEstimate {
        let k = arg_usize(args, "k", 10).max(1) as u64;
        // k candidate edges per node held in the write set.
        estimate_from_coefficients(counts, 8192, 64, 64 + 48 * k, 16, "KNN similarity")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let label = require_str(args, "label")?.to_string();
        let vector_property = require_str(args, "vector_property")?.to_string();
        let k = arg_usize(args, "k", 10).max(1);
        let cutoff = arg_f64(args, "cutoff", 0.0);
        let edge_type = arg_str(args, "edge_type")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("SIMILAR_TO")
            .to_string();
        let mutate = mode == OperationMode::Mutate;

        let nodes = graph.nodes_with_label(&label)?;

        // For mutate, index the source's existing edges of this type so a re-run
        // replaces them (idempotency): old neighbors no longer in the top-k are
        // tombstoned, current ones overwrite a stable edge id.
        let mut existing_by_source: HashMap<String, Vec<EdgeRecord>> = HashMap::new();
        if mutate {
            for edge in graph.list_edges()? {
                if edge.edge_type == edge_type {
                    existing_by_source
                        .entry(edge.from_id.clone())
                        .or_default()
                        .push(edge);
                }
            }
        }

        let mut rows: Vec<Value> = Vec::new();
        let mut sources_processed = 0usize;
        let mut edges_written = 0usize;
        let mut edges_removed = 0usize;

        for node in &nodes {
            let Some(query) = read_vector(&node.properties, &vector_property) else {
                continue;
            };
            sources_processed += 1;
            let neighbors = graph.vector_top_k(&label, &vector_property, &query, k + 1)?;
            let kept: Vec<(String, f64)> = neighbors
                .into_iter()
                .filter(|(id, _)| id != &node.id)
                .filter(|(_, similarity)| (*similarity as f64) >= cutoff)
                .take(k)
                .map(|(id, similarity)| (id, similarity as f64))
                .collect();

            for (target, score) in &kept {
                rows.push(json!({
                    "source": node.id,
                    "target": target,
                    "score": score,
                }));
            }

            if mutate {
                let new_targets: BTreeSet<&str> =
                    kept.iter().map(|(target, _)| target.as_str()).collect();
                if let Some(existing) = existing_by_source.get(&node.id) {
                    for old in existing {
                        if old.tombstone {
                            continue;
                        }
                        if !new_targets.contains(old.to_id.as_str()) {
                            let mut retracted = old.clone();
                            retracted.tombstone = true;
                            graph.upsert_edge(retracted)?;
                            edges_removed += 1;
                        }
                    }
                }
                for (target, score) in &kept {
                    let edge = EdgeRecord::new(
                        format!("{edge_type}:{}->{}", node.id, target),
                        node.id.clone(),
                        edge_type.clone(),
                        target.clone(),
                        json!({ "score": score }),
                    );
                    graph.upsert_edge(edge)?;
                    edges_written += 1;
                }
            }
        }

        if mutate {
            Ok(json!({
                "operation": self.command(),
                "mode": "mutate",
                "edge_type": edge_type,
                "sources_processed": sources_processed,
                "edges_written": edges_written,
                "edges_removed": edges_removed,
            }))
        } else {
            Ok(json!({
                "operation": self.command(),
                "mode": "stream",
                "edge_type": edge_type,
                "sources_processed": sources_processed,
                "pairs": rows.len(),
                "similarities": rows,
            }))
        }
    }
}

// ===== Deliverable 3: Leiden =====

#[derive(Clone, Copy, Debug)]
pub struct LeidenOp;

impl AlgorithmOperation for LeidenOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.leiden"
    }
    fn name(&self) -> &'static str {
        "leiden"
    }
    fn summary(&self) -> &'static str {
        "Leiden community detection (Traag et al. 2019) with a gamma resolution; the default community algorithm."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "gamma": { "type": "number", "default": 1.0, "description": "resolution parameter" },
                "seed": { "type": "integer", "default": 0 },
                "max_levels": { "type": "integer", "default": 10 },
                "mutate_property": { "type": "string", "default": "community_id" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 8192, 96, 192, 48, "Leiden")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let gamma = arg_f64(args, "gamma", 1.0);
        let seed = arg_u64(args, "seed", 0);
        let max_levels = arg_usize(args, "max_levels", 10).max(1);
        let result = leiden(&edges, gamma, seed, max_levels);
        let community_count = result
            .community
            .values()
            .copied()
            .collect::<BTreeSet<u64>>()
            .len();
        match mode {
            OperationMode::Stream => {
                let mut rows: Vec<Value> = result
                    .community
                    .iter()
                    .map(|(node_id, community_id)| {
                        json!({ "node_id": node_id, "community_id": community_id })
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    a["node_id"]
                        .as_str()
                        .unwrap_or("")
                        .cmp(b["node_id"].as_str().unwrap_or(""))
                });
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "gamma": gamma,
                    "community_count": community_count,
                    "modularity": result.modularity,
                    "levels": result.levels,
                    "communities": rows,
                }))
            }
            OperationMode::Stats => Ok(json!({
                "operation": self.command(),
                "mode": "stats",
                "gamma": gamma,
                "community_count": community_count,
                "modularity": result.modularity,
                "levels": result.levels,
                "node_count": result.community.len(),
            })),
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("community_id");
                let mut written = 0;
                for (node_id, community_id) in &result.community {
                    graph.write_node_property(node_id, property, json!(community_id))?;
                    written += 1;
                }
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                    "community_count": community_count,
                    "modularity": result.modularity,
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

// ===== Deliverable 4: Betweenness, articulation points, bridges =====

#[derive(Clone, Copy, Debug)]
pub struct BetweennessOp;

impl AlgorithmOperation for BetweennessOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.betweenness"
    }
    fn name(&self) -> &'static str {
        "betweenness"
    }
    fn summary(&self) -> &'static str {
        "Brandes betweenness centrality (exact or sampled) plus articulation points and bridges."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "directed": { "type": "boolean", "default": false },
                "sample_size": { "type": "integer", "description": "omit for exact Brandes" },
                "seed": { "type": "integer", "default": 0 },
                "top_k": { "type": "integer" },
                "mutate_property": { "type": "string", "default": "betweenness" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        // Per-source predecessor lists dominate the working set.
        estimate_from_coefficients(counts, 8192, 128, 256, 64, "Betweenness")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let directed = arg_bool(args, "directed", false);
        let seed = arg_u64(args, "seed", 0);
        let scores = match args.get("sample_size").and_then(Value::as_u64) {
            Some(sample_size) => {
                betweenness_centrality_sampled(&edges, directed, sample_size as usize, seed)
            }
            None => betweenness_centrality(&edges, directed),
        };
        let (points, bridges) = articulation_points_and_bridges(&edges);
        let bridge_rows: Vec<Value> = bridges
            .iter()
            .map(|(a, b)| json!({ "from": a, "to": b }))
            .collect();

        match mode {
            OperationMode::Stream => {
                let top_k = args
                    .get("top_k")
                    .and_then(Value::as_u64)
                    .map(|k| k as usize);
                let entries = sorted_scores(scores, top_k);
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "directed": directed,
                    "scores": score_rows(&entries, "betweenness"),
                    "articulation_points": points.iter().collect::<Vec<_>>(),
                    "bridges": bridge_rows,
                }))
            }
            OperationMode::Stats => {
                let values: Vec<f64> = scores.values().copied().collect();
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stats",
                    "directed": directed,
                    "stats": f64_stats(&values),
                    "articulation_point_count": points.len(),
                    "bridge_count": bridges.len(),
                }))
            }
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("betweenness");
                let entries = sorted_scores(scores, None);
                let written = write_scores(graph, &entries, property)?;
                for node_id in &points {
                    graph.write_node_property(node_id, "articulation_point", json!(true))?;
                }
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                    "articulation_points_marked": points.len(),
                    "bridge_count": bridges.len(),
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

// ===== Deliverable 5: SCC, condensation, topological sort =====

#[derive(Clone, Copy, Debug)]
pub struct SccOp;

impl AlgorithmOperation for SccOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.scc"
    }
    fn name(&self) -> &'static str {
        "scc"
    }
    fn summary(&self) -> &'static str {
        "Tarjan strongly connected components, the condensation, and a topological sort (errors on cyclic input)."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "mutate_property": { "type": "string", "default": "component_id" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 4096, 64, 96, 32, "SCC")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let components = strongly_connected_components(&edges);
        let topo = topological_sort(&edges);
        let is_dag = topo.is_ok();

        match mode {
            OperationMode::Stream => {
                let component_rows: Vec<Value> = components
                    .iter()
                    .enumerate()
                    .map(|(id, members)| json!({ "component_id": id, "nodes": members }))
                    .collect();
                let condensation_order: Vec<Vec<String>> = topological_sort_condensation(&edges);
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "component_count": components.len(),
                    "is_dag": is_dag,
                    "components": component_rows,
                    "topological_order": topo.ok(),
                    "condensation_order": condensation_order,
                }))
            }
            OperationMode::Stats => {
                let largest = components.first().map(Vec::len).unwrap_or(0);
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stats",
                    "component_count": components.len(),
                    "largest_component_size": largest,
                    "is_dag": is_dag,
                }))
            }
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("component_id");
                let mut written = 0;
                for (id, members) in components.iter().enumerate() {
                    for node_id in members {
                        graph.write_node_property(node_id, property, json!(id))?;
                        written += 1;
                    }
                }
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                    "component_count": components.len(),
                    "is_dag": is_dag,
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

// ===== Deliverable 6a: Node similarity =====

#[derive(Clone, Copy, Debug)]
pub struct NodeSimilarityOp;

impl AlgorithmOperation for NodeSimilarityOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.node_similarity"
    }
    fn name(&self) -> &'static str {
        "node_similarity"
    }
    fn summary(&self) -> &'static str {
        "Jaccard or Overlap similarity over neighbor sets with a degree cutoff and top-k."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "estimate"], "default": "stream" },
                "metric": { "type": "string", "enum": ["jaccard", "overlap"], "default": "jaccard" },
                "degree_cutoff": { "type": "integer", "default": 1 },
                "similarity_cutoff": { "type": "number", "default": 0.0 },
                "top_k": { "type": "integer", "default": 10 },
                "label": { "type": "string", "description": "restrict to nodes of this label" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 4096, 96, 192, 48, "Node similarity")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let metric = match arg_str(args, "metric") {
            Some(raw) => SimilarityMetric::parse(raw)
                .ok_or_else(|| OperationError::invalid_params(format!("unknown metric {raw:?}")))?,
            None => SimilarityMetric::Jaccard,
        };
        let degree_cutoff = arg_usize(args, "degree_cutoff", 1);
        let similarity_cutoff = arg_f64(args, "similarity_cutoff", 0.0);
        let top_k = arg_usize(args, "top_k", 10);
        let restrict = match arg_str(args, "label") {
            Some(label) => {
                let ids: BTreeSet<String> = graph
                    .nodes_with_label(label)?
                    .into_iter()
                    .map(|node| node.id)
                    .collect();
                Some(ids)
            }
            None => None,
        };
        let pairs = node_similarity(
            &edges,
            metric,
            degree_cutoff,
            similarity_cutoff,
            top_k,
            restrict.as_ref(),
        );

        match mode {
            OperationMode::Stream => {
                let rows: Vec<Value> = pairs
                    .iter()
                    .map(|pair| {
                        json!({
                            "node1": pair.node1,
                            "node2": pair.node2,
                            "similarity": pair.similarity,
                        })
                    })
                    .collect();
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "metric": metric,
                    "pair_count": rows.len(),
                    "similarities": rows,
                }))
            }
            OperationMode::Stats => {
                let values: Vec<f64> = pairs.iter().map(|pair| pair.similarity).collect();
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stats",
                    "metric": metric,
                    "stats": f64_stats(&values),
                }))
            }
            _ => unreachable!("node_similarity supports stream and stats"),
        }
    }
}

// ===== Deliverable 6b: Link-prediction pairwise functions =====

#[derive(Clone, Copy, Debug)]
pub struct LinkPredictionOp;

impl AlgorithmOperation for LinkPredictionOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.link_prediction"
    }
    fn name(&self) -> &'static str {
        "link_prediction"
    }
    fn summary(&self) -> &'static str {
        "Pairwise link-prediction features: common neighbors, Adamic/Adar, resource allocation."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_ONLY
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "estimate"], "default": "stream" },
                "pairs": {
                    "type": "array",
                    "items": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 2 }
                },
                "node1": { "type": "string" },
                "node2": { "type": "string" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 4096, 64, 96, 48, "Link prediction")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        _mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let neighbors = neighbor_sets(&edges);

        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Some(array) = args.get("pairs").and_then(Value::as_array) {
            for item in array {
                let pair = item.as_array().ok_or_else(|| {
                    OperationError::invalid_params("each pair must be a [node1, node2] array")
                })?;
                let a = pair.first().and_then(Value::as_str).ok_or_else(|| {
                    OperationError::invalid_params("pair[0] must be a node id string")
                })?;
                let b = pair.get(1).and_then(Value::as_str).ok_or_else(|| {
                    OperationError::invalid_params("pair[1] must be a node id string")
                })?;
                pairs.push((a.to_string(), b.to_string()));
            }
        }
        if let (Some(a), Some(b)) = (arg_str(args, "node1"), arg_str(args, "node2")) {
            pairs.push((a.to_string(), b.to_string()));
        }
        if pairs.is_empty() {
            return Err(OperationError::invalid_params(
                "provide pairs:[[a,b],...] or node1 + node2",
            ));
        }

        let rows: Vec<Value> = pairs
            .iter()
            .map(|(a, b)| {
                json!({
                    "node1": a,
                    "node2": b,
                    "common_neighbors": common_neighbors(&neighbors, a, b),
                    "adamic_adar": adamic_adar(&neighbors, a, b),
                    "resource_allocation": resource_allocation(&neighbors, a, b),
                })
            })
            .collect();
        Ok(json!({
            "operation": self.command(),
            "mode": "stream",
            "pairs": rows,
        }))
    }
}

// ===== Morphological graph message-passing scaffold =====

#[derive(Clone, Copy, Debug)]
pub struct MorphologicalMessagePassingOp;

impl AlgorithmOperation for MorphologicalMessagePassingOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.morphological_message_passing"
    }
    fn name(&self) -> &'static str {
        "morphological_message_passing"
    }
    fn summary(&self) -> &'static str {
        "Advisory message passing over city2graph-style touched_to / connected_to / faced_to edges."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "feature_property": { "type": "string", "default": "features" },
                "mutate_property": { "type": "string", "default": "morphological_embedding" },
                "iterations": { "type": "integer", "default": 1, "minimum": 1 },
                "top_k": { "type": "integer" },
                "relation_weights": {
                    "type": "object",
                    "additionalProperties": { "type": "number" },
                    "default": { "touched_to": 1.0, "connected_to": 0.8, "faced_to": 0.6 }
                }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        // Feature table + accumulator table, bounded by the existing graph size.
        estimate_from_coefficients(counts, 8192, 96, 192, 48, "Morphological message passing")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let morphological_edges = morphological_edges_from_records(&edges);
        let stats = morphology_stats(&morphological_edges);
        if mode == OperationMode::Stats {
            return Ok(json!({
                "operation": self.command(),
                "mode": "stats",
                "stats": stats,
            }));
        }
        if morphological_edges.is_empty() {
            return Ok(json!({
                "operation": self.command(),
                "mode": mode.as_str(),
                "node_count": 0,
                "edge_count": 0,
                "rows": [],
            }));
        }

        let feature_property = arg_str(args, "feature_property").unwrap_or("features");
        let features = morphological_features(graph, &morphological_edges, feature_property)?;
        if features.is_empty() {
            return Err(OperationError::invalid_params(format!(
                "no nodes incident to morphological edges carry a numeric array property {feature_property:?}"
            )));
        }
        let iterations = arg_usize(args, "iterations", 1).max(1);
        let weights = relation_weights_from_args(args);
        let passed =
            crate::morphology::message_pass(&features, &morphological_edges, iterations, &weights)
                .map_err(|error| OperationError::invalid_params(error.to_string()))?;
        let rows = morphological_rows(passed, args.get("top_k").and_then(Value::as_u64));

        match mode {
            OperationMode::Stream => Ok(json!({
                "operation": self.command(),
                "mode": "stream",
                "iterations": iterations,
                "feature_property": feature_property,
                "relation_weights": weights,
                "edge_count": stats.edge_count,
                "node_count": rows.len(),
                "rows": rows,
            })),
            OperationMode::Mutate => {
                let property =
                    arg_str(args, "mutate_property").unwrap_or("morphological_embedding");
                for row in &rows {
                    let node_id = row["node_id"].as_str().ok_or_else(|| {
                        OperationError::invalid_params("internal row missing node_id")
                    })?;
                    graph.write_node_property(node_id, property, row["embedding"].clone())?;
                }
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "iterations": iterations,
                    "feature_property": feature_property,
                    "mutate_property": property,
                    "edge_count": stats.edge_count,
                    "nodes_written": rows.len(),
                }))
            }
            OperationMode::Stats | OperationMode::Estimate => unreachable!("handled earlier"),
        }
    }
}

// ===== Conform the pre-existing algorithms to the operation contract =====

#[derive(Clone, Copy, Debug)]
pub struct PersonalizedPageRankOp;

impl AlgorithmOperation for PersonalizedPageRankOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.ppr"
    }
    fn name(&self) -> &'static str {
        "ppr"
    }
    fn summary(&self) -> &'static str {
        "Personalized PageRank (ACL local-push) seeded from a node-mass map."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["seeds"],
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "seeds": { "type": "object", "additionalProperties": { "type": "number" } },
                "alpha": { "type": "number", "default": 0.15 },
                "epsilon": { "type": "number", "default": 1e-4 },
                "max_pushes": { "type": "integer", "default": 200000 },
                "top_k": { "type": "integer" },
                "mutate_property": { "type": "string", "default": "ppr" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 4096, 48, 96, 24, "Personalized PageRank")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let seeds: HashMap<String, f64> = serde_json::from_value(
            args.get("seeds")
                .cloned()
                .ok_or_else(|| OperationError::invalid_params("ppr requires a seeds object"))?,
        )
        .map_err(|error| {
            OperationError::invalid_params(format!("seeds must be an object: {error}"))
        })?;
        let alpha = arg_f64(args, "alpha", 0.15);
        let epsilon = arg_f64(args, "epsilon", 1e-4);
        let max_pushes = arg_usize(args, "max_pushes", 200_000);
        let adjacency = adjacency_from_graph(&edges);
        let scores = personalized_pagerank(&adjacency, &seeds, alpha, epsilon, max_pushes);

        match mode {
            OperationMode::Stream => {
                let top_k = args
                    .get("top_k")
                    .and_then(Value::as_u64)
                    .map(|k| k as usize);
                let entries = sorted_scores(scores, top_k);
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "alpha": alpha,
                    "scores": score_rows(&entries, "score"),
                }))
            }
            OperationMode::Stats => {
                let values: Vec<f64> = scores.values().copied().collect();
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stats",
                    "stats": f64_stats(&values),
                }))
            }
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("ppr");
                let entries = sorted_scores(scores, None);
                let written = write_scores(graph, &entries, property)?;
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConnectedComponentsOp;

impl AlgorithmOperation for ConnectedComponentsOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.components"
    }
    fn name(&self) -> &'static str {
        "components"
    }
    fn summary(&self) -> &'static str {
        "Weakly/strongly connected components over the tenant graph."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "directed": { "type": "boolean", "default": false },
                "mutate_property": { "type": "string", "default": "component" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 2048, 32, 48, 24, "Connected components")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let directed = arg_bool(args, "directed", false);
        let components = connected_components(&edges, directed);
        match mode {
            OperationMode::Stream => Ok(json!({
                "operation": self.command(),
                "mode": "stream",
                "directed": directed,
                "count": components.len(),
                "components": components,
            })),
            OperationMode::Stats => {
                let largest = components.first().map(Vec::len).unwrap_or(0);
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stats",
                    "count": components.len(),
                    "largest_component_size": largest,
                }))
            }
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("component");
                let mut written = 0;
                for (id, members) in components.iter().enumerate() {
                    for node_id in members {
                        graph.write_node_property(node_id, property, json!(id))?;
                        written += 1;
                    }
                }
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                    "count": components.len(),
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LabelPropagationOp;

impl AlgorithmOperation for LabelPropagationOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.communities"
    }
    fn name(&self) -> &'static str {
        "communities"
    }
    fn summary(&self) -> &'static str {
        "Label-propagation community detection with modularity (legacy default; prefer Leiden)."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_STATS_MUTATE
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "stats", "mutate", "estimate"], "default": "stream" },
                "mutate_property": { "type": "string", "default": "lpa_community" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 2048, 48, 96, 32, "Label propagation")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let (community, modularity) = label_propagation_communities(&edges);
        let community_count = community.values().copied().collect::<BTreeSet<u64>>().len();
        match mode {
            OperationMode::Stream => {
                let mut rows: Vec<Value> = community
                    .iter()
                    .map(|(node_id, community_id)| {
                        json!({ "node_id": node_id, "community_id": community_id })
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    a["node_id"]
                        .as_str()
                        .unwrap_or("")
                        .cmp(b["node_id"].as_str().unwrap_or(""))
                });
                Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "algorithm": "label_propagation",
                    "community_count": community_count,
                    "modularity": modularity,
                    "communities": rows,
                }))
            }
            OperationMode::Stats => Ok(json!({
                "operation": self.command(),
                "mode": "stats",
                "community_count": community_count,
                "modularity": modularity,
            })),
            OperationMode::Mutate => {
                let property = arg_str(args, "mutate_property").unwrap_or("lpa_community");
                let mut written = 0;
                for (node_id, community_id) in &community {
                    graph.write_node_property(node_id, property, json!(community_id))?;
                    written += 1;
                }
                Ok(json!({
                    "operation": self.command(),
                    "mode": "mutate",
                    "mutate_property": property,
                    "nodes_written": written,
                    "community_count": community_count,
                    "modularity": modularity,
                }))
            }
            OperationMode::Estimate => unreachable!("estimate handled by dispatch"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ShortestPathOp;

impl AlgorithmOperation for ShortestPathOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.shortest_path"
    }
    fn name(&self) -> &'static str {
        "shortest_path"
    }
    fn summary(&self) -> &'static str {
        "Shortest path between two nodes: confidence-weighted (default) or unweighted by hops."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_ONLY
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["source", "target"],
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "estimate"], "default": "stream" },
                "source": { "type": "string" },
                "target": { "type": "string" },
                "max_depth": { "type": "integer", "default": 16 },
                "weighted": { "type": "boolean", "default": true, "description": "false = unweighted (fewest hops)" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 2048, 32, 48, 24, "Shortest path")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        _mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let source = require_str(args, "source")?;
        let target = require_str(args, "target")?;
        let max_depth = arg_usize(args, "max_depth", 16);
        if arg_bool(args, "weighted", true) {
            match paths_shortest_weighted(&edges, source, target, max_depth) {
                Some((path, cost)) => Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "weighted": true,
                    "found": true,
                    "path": path,
                    "cost": cost,
                })),
                None => Ok(json!({
                    "operation": self.command(),
                    "mode": "stream",
                    "weighted": true,
                    "found": false,
                    "path": [],
                })),
            }
        } else {
            let path = paths_shortest(
                edge_tuples(&edges),
                source.to_string(),
                target.to_string(),
                max_depth,
            );
            Ok(json!({
                "operation": self.command(),
                "mode": "stream",
                "weighted": false,
                "found": !path.is_empty(),
                "path": path,
            }))
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExpandOp;

impl AlgorithmOperation for ExpandOp {
    fn command(&self) -> &'static str {
        "rustyred.algorithm.expand"
    }
    fn name(&self) -> &'static str {
        "expand"
    }
    fn summary(&self) -> &'static str {
        "Bounded neighborhood expansion from seed nodes; confidence-filtered (default) or unweighted with per-node depth."
    }
    fn modes(&self) -> &'static [OperationMode] {
        STREAM_ONLY
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["seeds"],
            "properties": {
                "mode": { "type": "string", "enum": ["stream", "estimate"], "default": "stream" },
                "seeds": { "type": "array", "items": { "type": "string" } },
                "max_depth": { "type": "integer", "default": 3 },
                "min_confidence": { "type": "number", "default": 0.0 },
                "weighted": { "type": "boolean", "default": true, "description": "false = unweighted BFS returning per-node depth" }
            }
        })
    }
    fn estimate(&self, counts: GraphCounts, _args: &Value) -> MemoryEstimate {
        estimate_from_coefficients(counts, 2048, 32, 48, 24, "Expand")
    }
    fn run(
        &self,
        graph: &mut dyn AlgorithmGraph,
        _mode: OperationMode,
        args: &Value,
    ) -> Result<Value, OperationError> {
        let edges = graph.list_edges()?;
        let seeds: Vec<String> = args
            .get("seeds")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if seeds.is_empty() {
            return Err(OperationError::invalid_params(
                "expand requires a seeds array",
            ));
        }
        let max_depth = arg_usize(args, "max_depth", 3);
        if arg_bool(args, "weighted", true) {
            let min_confidence = arg_f64(args, "min_confidence", 0.0);
            let reached = expand_bounded_weighted(&edges, &seeds, max_depth, min_confidence);
            Ok(json!({
                "operation": self.command(),
                "mode": "stream",
                "weighted": true,
                "reached_count": reached.len(),
                "reached": reached,
            }))
        } else {
            let reached = expand_bounded(edge_tuples(&edges), seeds, max_depth);
            let rows: Vec<Value> = reached
                .into_iter()
                .map(|(node_id, depth)| json!({ "node_id": node_id, "depth": depth }))
                .collect();
            Ok(json!({
                "operation": self.command(),
                "mode": "stream",
                "weighted": false,
                "reached_count": rows.len(),
                "reached": rows,
            }))
        }
    }
}

/// Convert live edges to `(from, edge_type, to)` tuples for the unweighted
/// traversal helpers.
fn edge_tuples(edges: &[EdgeRecord]) -> Vec<EdgeTuple> {
    edges
        .iter()
        .filter(|edge| !edge.tombstone)
        .map(|edge| {
            (
                edge.from_id.clone(),
                edge.edge_type.clone(),
                edge.to_id.clone(),
            )
        })
        .collect()
}

fn morphological_features(
    graph: &dyn AlgorithmGraph,
    edges: &[MorphologicalEdge],
    feature_property: &str,
) -> Result<BTreeMap<String, Vec<f64>>, OperationError> {
    let mut node_ids = BTreeSet::new();
    for edge in edges {
        node_ids.insert(edge.source_id.as_str());
        node_ids.insert(edge.target_id.as_str());
    }

    let mut features = BTreeMap::new();
    for node_id in node_ids {
        let Some(node) = graph.get_node(node_id)? else {
            continue;
        };
        if let Some(vector) = read_f64_vector(&node.properties, feature_property) {
            features.insert(node_id.to_string(), vector);
        }
    }
    Ok(features)
}

fn read_f64_vector(properties: &Value, key: &str) -> Option<Vec<f64>> {
    properties
        .get(key)?
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect()
}

fn relation_weights_from_args(args: &Value) -> BTreeMap<String, f64> {
    let mut weights = default_relation_weights();
    let Some(object) = args.get("relation_weights").and_then(Value::as_object) else {
        return weights;
    };
    for (relation, value) in object {
        if is_morphological_relation(relation) {
            if let Some(weight) = value.as_f64() {
                weights.insert(relation.trim().to_ascii_lowercase(), weight);
            }
        }
    }
    weights
}

fn morphological_rows(passed: BTreeMap<String, Vec<f64>>, top_k: Option<u64>) -> Vec<Value> {
    let mut rows: Vec<Value> = passed
        .into_iter()
        .map(|(node_id, embedding)| {
            let norm = embedding
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
            json!({
                "node_id": node_id,
                "embedding": embedding,
                "norm": norm,
            })
        })
        .collect();
    rows.sort_by(|left, right| {
        let left_norm = left["norm"].as_f64().unwrap_or(0.0);
        let right_norm = right["norm"].as_f64().unwrap_or(0.0);
        right_norm
            .partial_cmp(&left_norm)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                left["node_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["node_id"].as_str().unwrap_or_default())
            })
    });
    if let Some(top_k) = top_k {
        rows.truncate(top_k as usize);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_store::{GraphStore, InMemoryGraphStore, NodeRecord};
    use crate::operation::dispatch_operation;

    fn store_with_triangles() -> InMemoryGraphStore {
        let mut store = InMemoryGraphStore::new();
        for id in ["a", "b", "c", "d"] {
            store
                .upsert_node(NodeRecord::new(id, ["Node"], json!({})))
                .unwrap();
        }
        for (id, from, to) in [
            ("a->b", "a", "b"),
            ("b->c", "b", "c"),
            ("c->a", "c", "a"),
            ("c->d", "c", "d"),
        ] {
            store
                .upsert_edge(EdgeRecord::new(id, from, "REL", to, json!({})))
                .unwrap();
        }
        store
    }

    #[test]
    fn pagerank_round_trips_all_three_modes_and_estimate() {
        let mut store = store_with_triangles();
        let op = PageRankOp;

        // stream
        let stream = dispatch_operation(&op, &mut store, &json!({ "mode": "stream" })).unwrap();
        assert_eq!(stream["mode"], "stream");
        assert!(stream["scores"].as_array().unwrap().len() >= 3);

        // stats
        let stats = dispatch_operation(&op, &mut store, &json!({ "mode": "stats" })).unwrap();
        assert_eq!(stats["mode"], "stats");
        assert!(stats["stats"]["count"].as_u64().unwrap() >= 3);

        // estimate
        let estimate = dispatch_operation(&op, &mut store, &json!({ "mode": "estimate" })).unwrap();
        assert_eq!(estimate["mode"], "estimate");
        assert!(estimate["estimate"]["bytes_min"].as_u64().unwrap() > 0);

        // mutate writes the pagerank property, readable via node fetch
        let mutate = dispatch_operation(&op, &mut store, &json!({ "mode": "mutate" })).unwrap();
        assert_eq!(mutate["mode"], "mutate");
        let node = GraphStore::get_node(&store, "a").unwrap();
        assert!(node.properties.get("pagerank").is_some());
    }

    #[test]
    fn unsupported_mode_is_rejected() {
        let mut store = store_with_triangles();
        let op = NodeSimilarityOp;
        let result = dispatch_operation(&op, &mut store, &json!({ "mode": "mutate" }));
        assert!(result.is_err());
    }

    #[test]
    fn similarity_knn_materializes_idempotent_edges_and_is_traversable() {
        let mut store = InMemoryGraphStore::new();
        store.designate_vector_property("Doc", "vec", 3).unwrap();
        let docs = [
            ("a", [1.0_f64, 0.0, 0.0]),
            ("b", [0.99, 0.02, 0.0]),
            ("c", [0.96, 0.05, 0.0]),
            ("d", [0.0, 0.0, 1.0]),
        ];
        for (id, vector) in docs {
            store
                .upsert_node(NodeRecord::new(
                    id,
                    ["Doc"],
                    json!({ "vec": vector.to_vec() }),
                ))
                .unwrap();
        }

        let op = SimilarityKnnOp;
        let args = json!({
            "mode": "mutate",
            "label": "Doc",
            "vector_property": "vec",
            "k": 2,
            "cutoff": 0.5,
            "edge_type": "SIMILAR_TO"
        });
        let first = dispatch_operation(&op, &mut store, &args).unwrap();
        assert!(first["edges_written"].as_u64().unwrap() > 0);

        let live_similar = |store: &InMemoryGraphStore| -> Vec<EdgeRecord> {
            store
                .snapshot()
                .edges
                .into_iter()
                .filter(|e| e.edge_type == "SIMILAR_TO" && !e.tombstone)
                .collect()
        };

        let after_first = live_similar(&store);
        // each node gains at most k edges
        let mut per_source: HashMap<String, usize> = HashMap::new();
        for edge in &after_first {
            *per_source.entry(edge.from_id.clone()).or_default() += 1;
        }
        assert!(per_source.values().all(|&count| count <= 2));
        // neighbor query: `a` has SIMILAR_TO edges to its near neighbors
        assert!(after_first.iter().any(|e| e.from_id == "a"));

        // re-running leaves no stale edges (idempotent materialization)
        let before = after_first.len();
        dispatch_operation(&op, &mut store, &args).unwrap();
        assert_eq!(before, live_similar(&store).len());

        // personalized_pagerank traverses the SIMILAR_TO edges: seeded at `a`,
        // the near neighbors (reachable only via the materialized edges, since
        // there are no structural edges here) receive positive mass.
        let ppr = dispatch_operation(
            &PersonalizedPageRankOp,
            &mut store,
            &json!({ "mode": "stream", "seeds": { "a": 1.0 } }),
        )
        .unwrap();
        let scored: BTreeSet<String> = ppr["scores"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["score"].as_f64().unwrap_or(0.0) > 0.0)
            .map(|row| row["node_id"].as_str().unwrap().to_string())
            .collect();
        assert!(scored.contains("b") || scored.contains("c"));
    }

    #[test]
    fn morphological_message_passing_stream_stats_and_mutate() {
        let mut store = InMemoryGraphStore::new();
        for (id, features) in [
            ("place:a", vec![1.0, 0.0]),
            ("place:b", vec![0.0, 1.0]),
            ("movement:main", vec![0.25, 0.25]),
        ] {
            store
                .upsert_node(NodeRecord::new(
                    id,
                    ["Morphology"],
                    json!({ "features": features }),
                ))
                .unwrap();
        }
        store
            .upsert_edge(EdgeRecord::new(
                "a-touch-b",
                "place:a",
                "touched_to",
                "place:b",
                json!({}),
            ))
            .unwrap();
        store
            .upsert_edge(EdgeRecord::new(
                "a-face-main",
                "place:a",
                "faced_to",
                "movement:main",
                json!({}),
            ))
            .unwrap();

        let op = MorphologicalMessagePassingOp;
        let stream = dispatch_operation(
            &op,
            &mut store,
            &json!({
                "mode": "stream",
                "feature_property": "features",
                "relation_weights": { "touched_to": 1.0, "faced_to": 1.0 }
            }),
        )
        .unwrap();
        assert_eq!(stream["mode"], "stream");
        assert_eq!(stream["edge_count"], 2);
        let place_b = stream["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["node_id"] == "place:b")
            .expect("place:b row");
        assert_eq!(place_b["embedding"], json!([0.5, 0.5]));

        let stats = dispatch_operation(&op, &mut store, &json!({ "mode": "stats" })).unwrap();
        assert_eq!(stats["stats"]["touched_to_count"], 1);
        assert_eq!(stats["stats"]["faced_to_count"], 1);

        let mutate = dispatch_operation(
            &op,
            &mut store,
            &json!({ "mode": "mutate", "mutate_property": "morph" }),
        )
        .unwrap();
        assert_eq!(mutate["nodes_written"], 3);
        let node = GraphStore::get_node(&store, "place:b").unwrap();
        assert_eq!(node.properties["morph"], json!([0.5, 0.5]));
    }

    #[test]
    fn scc_stream_reports_cycle_and_condensation() {
        let mut store = store_with_triangles();
        let op = SccOp;
        let stream = dispatch_operation(&op, &mut store, &json!({ "mode": "stream" })).unwrap();
        assert_eq!(stream["is_dag"], false);
        assert!(stream["topological_order"].is_null());
        // {a,b,c} is one SCC; condensation order is non-empty.
        assert!(!stream["condensation_order"].as_array().unwrap().is_empty());
    }
}
