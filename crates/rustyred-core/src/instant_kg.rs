use std::collections::{BTreeSet, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::graph::personalized_pagerank;
use crate::graph_store::{Direction, EdgeRecord, GraphSnapshot, GraphStats, NodeRecord};
use crate::state::stable_hash;

pub const INSTANT_KG_PROTOCOL_VERSION: &str = "harness-instant-kg-v1";
pub const INSTANT_KG_DEFAULT_ENCODER_VERSION: &str = "lightweight-code-text-v1";
pub const INSTANT_KG_DEFAULT_INGEST_VERSION: &str = "rustyred-instant-kg-v1";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CodeKgEncodedFile {
    pub path: String,
    pub sha: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CodeKgManifest {
    pub repo_id: String,
    pub repo_hash: String,
    pub commit_sha: String,
    pub encoder_version: String,
    pub ingest_version: String,
    pub base_graph_hash: String,
    pub encoded_files: Vec<CodeKgEncodedFile>,
    pub objects_total: usize,
    pub edges_total: usize,
}

impl CodeKgManifest {
    pub fn from_base_snapshot(
        repo_id: impl Into<String>,
        commit_sha: impl Into<String>,
        base: &GraphSnapshot,
    ) -> Self {
        let repo_id = repo_id.into();
        let commit_sha = commit_sha.into();
        Self {
            repo_hash: stable_hash(&repo_id),
            base_graph_hash: stable_hash(base),
            repo_id,
            commit_sha,
            encoder_version: INSTANT_KG_DEFAULT_ENCODER_VERSION.to_string(),
            ingest_version: INSTANT_KG_DEFAULT_INGEST_VERSION.to_string(),
            encoded_files: Vec::new(),
            objects_total: base.nodes.iter().filter(|node| !node.tombstone).count(),
            edges_total: base.edges.iter().filter(|edge| !edge.tombstone).count(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SessionDelta {
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub objects: Vec<NodeRecord>,
    #[serde(default)]
    pub edges: Vec<EdgeRecord>,
    #[serde(default)]
    pub tombstoned_object_ids: Vec<String>,
    #[serde(default)]
    pub removed_edge_ids: Vec<String>,
}

impl SessionDelta {
    fn tombstoned_objects(&self) -> BTreeSet<String> {
        self.tombstoned_object_ids
            .iter()
            .cloned()
            .chain(
                self.objects
                    .iter()
                    .filter(|node| node.tombstone)
                    .map(|node| node.id.clone()),
            )
            .collect()
    }

    fn removed_edges(&self) -> BTreeSet<String> {
        self.removed_edge_ids
            .iter()
            .cloned()
            .chain(
                self.edges
                    .iter()
                    .filter(|edge| edge.tombstone)
                    .map(|edge| edge.id.clone()),
            )
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InstantKgStatus {
    pub protocol_version: String,
    pub repo_id: String,
    pub base_commit_sha: String,
    pub delta_commit_sha: Option<String>,
    pub encoder_version: String,
    pub ingest_version: String,
    pub base_graph_hash: String,
    pub merged_graph_hash: String,
    pub base_objects: usize,
    pub base_edges: usize,
    pub delta_objects: usize,
    pub delta_edges: usize,
    pub tombstoned_objects: usize,
    pub removed_edges: usize,
    pub total_objects: usize,
    pub total_edges: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PprResult {
    pub object_id: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<NodeRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ImpactResult {
    pub object_id: String,
    pub depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<NodeRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SearchResult {
    pub object_id: String,
    pub score: f64,
    pub matched_fields: Vec<String>,
    pub object: NodeRecord,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EdgeExplanation {
    pub edge_id: String,
    pub src: String,
    pub dst: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub layer: String,
    pub evidence: Vec<String>,
    pub edge: EdgeRecord,
}

#[derive(Clone, Debug)]
pub struct HarnessInstantKg {
    manifest: CodeKgManifest,
    delta: SessionDelta,
    objects: HashMap<String, NodeRecord>,
    edges: HashMap<String, EdgeRecord>,
    base_edge_ids: BTreeSet<String>,
    delta_edge_ids: BTreeSet<String>,
}

impl HarnessInstantKg {
    pub fn new(base: GraphSnapshot, manifest: Option<CodeKgManifest>, delta: SessionDelta) -> Self {
        let manifest = manifest.unwrap_or_else(|| {
            CodeKgManifest::from_base_snapshot("unknown", format!("v{}", base.version), &base)
        });
        let tombstoned = delta.tombstoned_objects();
        let removed_edges = delta.removed_edges();

        let mut objects = HashMap::new();
        for node in base.nodes.into_iter().filter(|node| !node.tombstone) {
            if !tombstoned.contains(&node.id) {
                objects.insert(node.id.clone(), node);
            }
        }
        for node in delta.objects.iter().filter(|node| !node.tombstone) {
            objects.insert(node.id.clone(), node.clone());
        }

        let mut edges = HashMap::new();
        let mut base_edge_ids = BTreeSet::new();
        for edge in base.edges.into_iter().filter(|edge| !edge.tombstone) {
            if removed_edges.contains(&edge.id) {
                continue;
            }
            if tombstoned.contains(&edge.from_id) || tombstoned.contains(&edge.to_id) {
                continue;
            }
            if objects.contains_key(&edge.from_id) && objects.contains_key(&edge.to_id) {
                base_edge_ids.insert(edge.id.clone());
                edges.insert(edge.id.clone(), edge);
            }
        }

        let mut delta_edge_ids = BTreeSet::new();
        for edge in delta.edges.iter().filter(|edge| !edge.tombstone) {
            if objects.contains_key(&edge.from_id) && objects.contains_key(&edge.to_id) {
                delta_edge_ids.insert(edge.id.clone());
                edges.insert(edge.id.clone(), edge.clone());
            }
        }

        Self {
            manifest,
            delta,
            objects,
            edges,
            base_edge_ids,
            delta_edge_ids,
        }
    }

    pub fn status(&self) -> InstantKgStatus {
        let merged = self.merged_snapshot();
        InstantKgStatus {
            protocol_version: INSTANT_KG_PROTOCOL_VERSION.to_string(),
            repo_id: self.manifest.repo_id.clone(),
            base_commit_sha: self.manifest.commit_sha.clone(),
            delta_commit_sha: self.delta.commit_sha.clone(),
            encoder_version: self.manifest.encoder_version.clone(),
            ingest_version: self.manifest.ingest_version.clone(),
            base_graph_hash: self.manifest.base_graph_hash.clone(),
            merged_graph_hash: stable_hash(&merged),
            base_objects: self.manifest.objects_total,
            base_edges: self.manifest.edges_total,
            delta_objects: self
                .delta
                .objects
                .iter()
                .filter(|node| !node.tombstone)
                .count(),
            delta_edges: self
                .delta
                .edges
                .iter()
                .filter(|edge| !edge.tombstone)
                .count(),
            tombstoned_objects: self.delta.tombstoned_objects().len(),
            removed_edges: self.delta.removed_edges().len(),
            total_objects: self.objects.len(),
            total_edges: self.edges.len(),
        }
    }

    pub fn merged_snapshot(&self) -> GraphSnapshot {
        let mut nodes: Vec<NodeRecord> = self.objects.values().cloned().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut edges: Vec<EdgeRecord> = self.edges.values().cloned().collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        GraphSnapshot {
            version: 0,
            nodes,
            edges,
        }
    }

    pub fn stats(&self) -> GraphStats {
        let snapshot = self.merged_snapshot();
        GraphStats {
            version: snapshot.version,
            nodes_total: snapshot.nodes.len(),
            edges_total: snapshot.edges.len(),
            labels_total: snapshot
                .nodes
                .iter()
                .flat_map(|node| node.labels.iter().cloned())
                .collect::<BTreeSet<_>>()
                .len(),
            edge_types_total: snapshot
                .edges
                .iter()
                .map(|edge| edge.edge_type.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            property_keys_total: snapshot
                .nodes
                .iter()
                .flat_map(|node| {
                    node.properties
                        .as_object()
                        .map(|object| object.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect::<BTreeSet<_>>()
                .len(),
            property_indexes_total: 0,
            memory_bytes: serde_json::to_vec(&snapshot)
                .map(|bytes| bytes.len())
                .unwrap_or(0),
            memory_quota_bytes: 0,
        }
    }

    pub fn get_object(&self, object_id: &str) -> Option<&NodeRecord> {
        self.objects.get(object_id)
    }

    pub fn resolve_symbol_name(&self, symbol_name: &str) -> Option<String> {
        let needle = symbol_name.trim();
        if needle.is_empty() {
            return None;
        }
        if self.objects.contains_key(needle) {
            return Some(needle.to_string());
        }
        let mut matches: Vec<String> = self
            .objects
            .values()
            .filter(|object| object_has_symbol_name(object, needle))
            .map(|object| object.id.clone())
            .collect();
        matches.sort();
        matches.into_iter().next()
    }

    pub fn get_edges_from(&self, object_id: &str) -> Vec<EdgeRecord> {
        let mut out: Vec<EdgeRecord> = self
            .edges
            .values()
            .filter(|edge| edge.from_id == object_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.edge_type
                .cmp(&b.edge_type)
                .then_with(|| a.to_id.cmp(&b.to_id))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    pub fn ppr(
        &self,
        seeds: &HashMap<String, f64>,
        alpha: f64,
        epsilon: f64,
        max_pushes: usize,
        top_k: usize,
    ) -> Vec<PprResult> {
        let adjacency = self.adjacency();
        let mut entries: Vec<(String, f64)> =
            personalized_pagerank(&adjacency, seeds, alpha, epsilon, max_pushes)
                .into_iter()
                .collect();
        entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        entries.truncate(top_k);
        entries
            .into_iter()
            .map(|(object_id, score)| PprResult {
                object: self.objects.get(&object_id).cloned(),
                object_id,
                score,
            })
            .collect()
    }

    pub fn impact(&self, seed: &str, direction: Direction, max_depth: usize) -> Vec<ImpactResult> {
        if !self.objects.contains_key(seed) {
            return Vec::new();
        }
        let adjacency = self.directional_adjacency(direction);
        let mut queue = VecDeque::from([(seed.to_string(), 0usize)]);
        let mut seen = BTreeSet::from([seed.to_string()]);
        let mut out = Vec::new();

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for neighbor in adjacency.get(&node_id).into_iter().flatten() {
                if !seen.insert(neighbor.clone()) {
                    continue;
                }
                let next_depth = depth + 1;
                out.push(ImpactResult {
                    object_id: neighbor.clone(),
                    depth: next_depth,
                    object: self.objects.get(neighbor).cloned(),
                });
                queue.push_back((neighbor.clone(), next_depth));
            }
        }
        out.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });
        out
    }

    pub fn related_objects(&self, seed: &str, kinds: &[String], top_k: usize) -> Vec<PprResult> {
        let seeds = HashMap::from([(seed.to_string(), 1.0)]);
        let mut results = self.ppr(
            &seeds,
            0.15,
            1e-4,
            200_000,
            top_k.saturating_mul(4).max(top_k),
        );
        if !kinds.is_empty() {
            results.retain(|result| {
                result
                    .object
                    .as_ref()
                    .is_some_and(|object| object_matches_kinds(object, kinds))
            });
        }
        results.retain(|result| result.object_id != seed);
        results.truncate(top_k);
        results
    }

    pub fn search(&self, query: &str, kinds: &[String], top_k: usize) -> Vec<SearchResult> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|term| term.trim().to_ascii_lowercase())
            .filter(|term| !term.is_empty())
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::new();
        for object in self.objects.values() {
            if !kinds.is_empty() && !object_matches_kinds(object, kinds) {
                continue;
            }
            let fields = searchable_fields(object);
            let mut matched_fields = BTreeSet::new();
            let mut score = 0.0f64;
            for (field, text) in fields {
                let haystack = text.to_ascii_lowercase();
                for term in &terms {
                    if haystack.contains(term) {
                        matched_fields.insert(field.clone());
                        score += if field == "id" || field == "name" {
                            2.0
                        } else {
                            1.0
                        };
                    }
                }
            }
            if score > 0.0 {
                results.push(SearchResult {
                    object_id: object.id.clone(),
                    score,
                    matched_fields: matched_fields.into_iter().collect(),
                    object: object.clone(),
                });
            }
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });
        results.truncate(top_k);
        results
    }

    pub fn explain_edge(&self, src: &str, dst: &str) -> Vec<EdgeExplanation> {
        let mut explanations: Vec<EdgeExplanation> = self
            .edges
            .values()
            .filter(|edge| edge.from_id == src && edge.to_id == dst)
            .map(|edge| {
                let layer = if self.delta_edge_ids.contains(&edge.id) {
                    "delta"
                } else if self.base_edge_ids.contains(&edge.id) {
                    "base"
                } else {
                    "merged"
                };
                EdgeExplanation {
                    edge_id: edge.id.clone(),
                    src: edge.from_id.clone(),
                    dst: edge.to_id.clone(),
                    edge_type: edge.edge_type.clone(),
                    layer: layer.to_string(),
                    evidence: edge_evidence(edge),
                    edge: edge.clone(),
                }
            })
            .collect();
        explanations.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        explanations
    }

    fn adjacency(&self) -> HashMap<String, Vec<(String, f64)>> {
        let mut adjacency: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for edge in self.edges.values() {
            adjacency
                .entry(edge.from_id.clone())
                .or_default()
                .push((edge.to_id.clone(), edge.effective_confidence()));
        }
        adjacency
    }

    fn directional_adjacency(&self, direction: Direction) -> HashMap<String, Vec<String>> {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in self.edges.values() {
            match direction {
                Direction::Out => adjacency
                    .entry(edge.from_id.clone())
                    .or_default()
                    .push(edge.to_id.clone()),
                Direction::In => adjacency
                    .entry(edge.to_id.clone())
                    .or_default()
                    .push(edge.from_id.clone()),
            }
        }
        for neighbors in adjacency.values_mut() {
            neighbors.sort();
            neighbors.dedup();
        }
        adjacency
    }
}

fn object_matches_kinds(object: &NodeRecord, kinds: &[String]) -> bool {
    let kind_values: BTreeSet<String> =
        kinds.iter().map(|kind| kind.to_ascii_lowercase()).collect();
    object
        .labels
        .iter()
        .any(|label| kind_values.contains(&label.to_ascii_lowercase()))
        || object
            .properties
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind_values.contains(&kind.to_ascii_lowercase()))
}

fn object_has_symbol_name(object: &NodeRecord, needle: &str) -> bool {
    let id_tail = object
        .id
        .rsplit([':', '/', '#'])
        .next()
        .is_some_and(|tail| tail.eq_ignore_ascii_case(needle));
    id_tail
        || ["name", "symbol", "title"].iter().any(|key| {
            object
                .properties
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(needle))
        })
}

fn searchable_fields(object: &NodeRecord) -> Vec<(String, String)> {
    let mut fields = vec![
        ("id".to_string(), object.id.clone()),
        ("labels".to_string(), object.labels.join(" ")),
    ];
    for key in [
        "name",
        "title",
        "path",
        "module",
        "symbol",
        "content",
        "text",
        "docstring",
    ] {
        if let Some(value) = object.properties.get(key) {
            if let Some(text) = value.as_str() {
                fields.push((key.to_string(), text.to_string()));
            }
        }
    }
    fields
}

fn edge_evidence(edge: &EdgeRecord) -> Vec<String> {
    let mut evidence = Vec::new();
    if let Some(source) = edge.properties.get("source").and_then(Value::as_str) {
        evidence.push(source.to_string());
    }
    if let Some(path) = edge.properties.get("path").and_then(Value::as_str) {
        let line = edge.properties.get("line").and_then(Value::as_u64);
        evidence.push(match line {
            Some(line) => format!("{path}:{line}"),
            None => path.to_string(),
        });
    }
    if let Some(reason) = edge.properties.get("reason").and_then(Value::as_str) {
        evidence.push(reason.to_string());
    }
    if evidence.is_empty() {
        evidence.push(format!(
            "{} edge {} connects {} -> {}",
            edge.edge_type, edge.id, edge.from_id, edge.to_id
        ));
    }
    evidence
}

pub fn instant_kg_payload_delta(value: Option<Value>) -> SessionDelta {
    value
        .and_then(|raw| serde_json::from_value(raw).ok())
        .unwrap_or_default()
}

pub fn instant_kg_payload_manifest(value: Option<Value>) -> Option<CodeKgManifest> {
    value.and_then(|raw| serde_json::from_value(raw).ok())
}

pub fn instant_kg_status_payload(status: InstantKgStatus, stats: GraphStats) -> Value {
    json!({
        "ok": true,
        "status": status,
        "stats": stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_store::{EdgeRecord, GraphSnapshot, NodeRecord};

    #[test]
    fn delta_tombstones_base_objects_and_edges() {
        let base = GraphSnapshot {
            version: 10,
            nodes: vec![
                NodeRecord::new("file:a", ["File"], json!({ "path": "src/a.rs" })),
                NodeRecord::new("sym:old", ["Symbol"], json!({ "name": "old" })),
            ],
            edges: vec![EdgeRecord::new(
                "edge:old",
                "file:a",
                "contains",
                "sym:old",
                json!({}),
            )],
        };
        let view = HarnessInstantKg::new(
            base,
            None,
            SessionDelta {
                tombstoned_object_ids: vec!["sym:old".to_string()],
                ..SessionDelta::default()
            },
        );

        assert!(view.get_object("sym:old").is_none());
        assert!(view.get_edges_from("file:a").is_empty());
        assert_eq!(view.status().total_objects, 1);
        assert_eq!(view.status().total_edges, 0);
    }

    #[test]
    fn delta_objects_override_base_and_ppr_uses_merged_edges() {
        let base = GraphSnapshot {
            version: 1,
            nodes: vec![
                NodeRecord::new("file:a", ["File"], json!({ "path": "src/a.rs" })),
                NodeRecord::new("sym:f", ["Symbol"], json!({ "name": "before" })),
            ],
            edges: vec![],
        };
        let view = HarnessInstantKg::new(
            base,
            None,
            SessionDelta {
                objects: vec![
                    NodeRecord::new("sym:f", ["Symbol"], json!({ "name": "after" })),
                    NodeRecord::new("sym:g", ["Symbol"], json!({ "name": "g" })),
                ],
                edges: vec![EdgeRecord::new(
                    "edge:fg",
                    "sym:f",
                    "calls",
                    "sym:g",
                    json!({}),
                )],
                ..SessionDelta::default()
            },
        );

        assert_eq!(
            view.get_object("sym:f").unwrap().properties["name"],
            json!("after")
        );
        let scores = view.ppr(
            &HashMap::from([("sym:f".to_string(), 1.0)]),
            0.15,
            1e-4,
            200_000,
            5,
        );
        assert!(scores.iter().any(|row| row.object_id == "sym:g"));
        assert_eq!(view.status().delta_objects, 2);
        assert_eq!(view.status().delta_edges, 1);
    }

    #[test]
    fn search_and_explain_edge_surface_code_evidence() {
        let base = GraphSnapshot {
            version: 1,
            nodes: vec![
                NodeRecord::new("file:lib", ["File"], json!({ "path": "src/lib.rs" })),
                NodeRecord::new("sym:encode", ["Symbol"], json!({ "name": "encode_repo" })),
            ],
            edges: vec![EdgeRecord::new(
                "edge:contains",
                "file:lib",
                "contains",
                "sym:encode",
                json!({ "path": "src/lib.rs", "line": 42 }),
            )],
        };
        let view = HarnessInstantKg::new(base, None, SessionDelta::default());

        let hits = view.search("encode", &["Symbol".to_string()], 3);
        assert_eq!(hits[0].object_id, "sym:encode");
        assert_eq!(
            view.resolve_symbol_name("encode_repo").as_deref(),
            Some("sym:encode")
        );
        assert_eq!(
            view.resolve_symbol_name("encode").as_deref(),
            Some("sym:encode")
        );

        let explanations = view.explain_edge("file:lib", "sym:encode");
        assert_eq!(explanations[0].layer, "base");
        assert_eq!(explanations[0].evidence[0], "src/lib.rs:42");
    }
}
