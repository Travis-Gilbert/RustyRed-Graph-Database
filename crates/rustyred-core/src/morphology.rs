//! Morphological graph primitives for the city2graph parity lane.
//!
//! The Python city2graph bridge remains the oracle. This module is the small,
//! advisory Rust-side spine: typed relation records, deterministic topology
//! helpers, and backend-neutral message passing that can be lowered to Burn once
//! the geometry parity layer is in place.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::graph_store::EdgeRecord;

pub const TOUCHED_TO: &str = "touched_to";
pub const CONNECTED_TO: &str = "connected_to";
pub const FACED_TO: &str = "faced_to";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MorphologicalNodeKind {
    Place,
    Movement,
}

impl MorphologicalNodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Place => "place",
            Self::Movement => "movement",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MorphologicalEdge {
    pub edge_id: String,
    pub source_id: String,
    pub source_kind: MorphologicalNodeKind,
    pub relation: String,
    pub target_id: String,
    pub target_kind: MorphologicalNodeKind,
    pub confidence: f64,
}

impl MorphologicalEdge {
    pub fn new(
        source_id: impl Into<String>,
        source_kind: MorphologicalNodeKind,
        relation: impl Into<String>,
        target_id: impl Into<String>,
        target_kind: MorphologicalNodeKind,
    ) -> Self {
        let source_id = source_id.into();
        let relation = normalize_relation(&relation.into());
        let target_id = target_id.into();
        let edge_id = format!("morphological:{source_id}:{relation}:{target_id}");
        Self {
            edge_id,
            source_id,
            source_kind,
            relation,
            target_id,
            target_kind,
            confidence: 1.0,
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    pub fn from_edge_record(edge: &EdgeRecord) -> Option<Self> {
        let relation = normalize_relation(&edge.edge_type);
        let (source_kind, target_kind) = relation_kinds(&relation)?;
        Some(Self {
            edge_id: edge.id.clone(),
            source_id: edge.from_id.clone(),
            source_kind,
            relation,
            target_id: edge.to_id.clone(),
            target_kind,
            confidence: edge.effective_confidence(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreetSegmentTopology {
    pub segment_id: String,
    pub start_node_id: String,
    pub end_node_id: String,
}

impl StreetSegmentTopology {
    pub fn new(
        segment_id: impl Into<String>,
        start_node_id: impl Into<String>,
        end_node_id: impl Into<String>,
    ) -> Self {
        Self {
            segment_id: segment_id.into(),
            start_node_id: start_node_id.into(),
            end_node_id: end_node_id.into(),
        }
    }

    fn endpoints(&self) -> [&str; 2] {
        [&self.start_node_id, &self.end_node_id]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MorphologyStats {
    pub edge_count: usize,
    pub touched_to_count: usize,
    pub connected_to_count: usize,
    pub faced_to_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MorphologyError {
    pub message: String,
}

impl MorphologyError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for MorphologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MorphologyError {}

pub fn is_morphological_relation(relation: &str) -> bool {
    relation_kinds(&normalize_relation(relation)).is_some()
}

pub fn morphological_edges_from_records(edges: &[EdgeRecord]) -> Vec<MorphologicalEdge> {
    edges
        .iter()
        .filter(|edge| !edge.tombstone)
        .filter_map(MorphologicalEdge::from_edge_record)
        .collect()
}

pub fn dual_graph_edges(segments: &[StreetSegmentTopology]) -> Vec<MorphologicalEdge> {
    let mut out = Vec::new();
    for (index, left) in segments.iter().enumerate() {
        for right in segments.iter().skip(index + 1) {
            if share_endpoint(left, right) {
                out.push(MorphologicalEdge::new(
                    &left.segment_id,
                    MorphologicalNodeKind::Movement,
                    CONNECTED_TO,
                    &right.segment_id,
                    MorphologicalNodeKind::Movement,
                ));
                out.push(MorphologicalEdge::new(
                    &right.segment_id,
                    MorphologicalNodeKind::Movement,
                    CONNECTED_TO,
                    &left.segment_id,
                    MorphologicalNodeKind::Movement,
                ));
            }
        }
    }
    out
}

pub fn morphology_stats(edges: &[MorphologicalEdge]) -> MorphologyStats {
    let mut stats = MorphologyStats {
        edge_count: edges.len(),
        touched_to_count: 0,
        connected_to_count: 0,
        faced_to_count: 0,
    };
    for edge in edges {
        match edge.relation.as_str() {
            TOUCHED_TO => stats.touched_to_count += 1,
            CONNECTED_TO => stats.connected_to_count += 1,
            FACED_TO => stats.faced_to_count += 1,
            _ => {}
        }
    }
    stats
}

pub fn default_relation_weights() -> BTreeMap<String, f64> {
    BTreeMap::from([
        (TOUCHED_TO.to_string(), 1.0),
        (CONNECTED_TO.to_string(), 0.8),
        (FACED_TO.to_string(), 0.6),
    ])
}

pub fn message_pass(
    features: &BTreeMap<String, Vec<f64>>,
    edges: &[MorphologicalEdge],
    iterations: usize,
    relation_weights: &BTreeMap<String, f64>,
) -> Result<BTreeMap<String, Vec<f64>>, MorphologyError> {
    let dimension = feature_dimension(features)?;
    let mut state = features.clone();
    for _ in 0..iterations {
        let mut sums: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut weights: BTreeMap<String, f64> = BTreeMap::new();
        for edge in edges {
            let Some(source) = state.get(&edge.source_id) else {
                continue;
            };
            let weight =
                relation_weights.get(&edge.relation).copied().unwrap_or(1.0) * edge.confidence;
            if weight == 0.0 {
                continue;
            }
            let entry = sums
                .entry(edge.target_id.clone())
                .or_insert_with(|| vec![0.0; dimension]);
            for (slot, value) in entry.iter_mut().zip(source.iter()) {
                *slot += *value * weight;
            }
            *weights.entry(edge.target_id.clone()).or_insert(0.0) += weight.abs();
        }

        let mut next = state.clone();
        for (node_id, sum) in sums {
            let denom = weights.get(&node_id).copied().unwrap_or(1.0).max(1.0);
            let incoming = sum.into_iter().map(|value| value / denom);
            match next.get_mut(&node_id) {
                Some(existing) => {
                    for (slot, value) in existing.iter_mut().zip(incoming) {
                        *slot = (*slot + value) / 2.0;
                    }
                }
                None => {
                    next.insert(node_id, incoming.collect());
                }
            }
        }
        state = next;
    }
    Ok(state)
}

fn feature_dimension(features: &BTreeMap<String, Vec<f64>>) -> Result<usize, MorphologyError> {
    let mut dimensions = BTreeSet::new();
    for values in features.values() {
        if values.is_empty() {
            return Err(MorphologyError::new("feature vectors must not be empty"));
        }
        dimensions.insert(values.len());
    }
    match dimensions.len() {
        0 => Err(MorphologyError::new(
            "at least one feature vector is required",
        )),
        1 => Ok(*dimensions.iter().next().expect("one dimension")),
        _ => Err(MorphologyError::new(
            "all feature vectors must have the same dimension",
        )),
    }
}

fn share_endpoint(left: &StreetSegmentTopology, right: &StreetSegmentTopology) -> bool {
    let left_endpoints = left.endpoints();
    let right_endpoints = right.endpoints();
    left_endpoints
        .iter()
        .any(|endpoint| right_endpoints.contains(endpoint))
}

fn relation_kinds(relation: &str) -> Option<(MorphologicalNodeKind, MorphologicalNodeKind)> {
    match relation {
        TOUCHED_TO => Some((MorphologicalNodeKind::Place, MorphologicalNodeKind::Place)),
        CONNECTED_TO => Some((
            MorphologicalNodeKind::Movement,
            MorphologicalNodeKind::Movement,
        )),
        FACED_TO => Some((
            MorphologicalNodeKind::Place,
            MorphologicalNodeKind::Movement,
        )),
        _ => None,
    }
}

fn normalize_relation(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

pub fn relation_weights_from_map(raw: &HashMap<String, f64>) -> BTreeMap<String, f64> {
    let mut weights = default_relation_weights();
    for (relation, weight) in raw {
        let relation = normalize_relation(relation);
        if is_morphological_relation(&relation) {
            weights.insert(relation, *weight);
        }
    }
    weights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_graph_connects_segments_that_share_endpoints() {
        let edges = dual_graph_edges(&[
            StreetSegmentTopology::new("s1", "a", "b"),
            StreetSegmentTopology::new("s2", "b", "c"),
            StreetSegmentTopology::new("s3", "d", "e"),
        ]);

        assert_eq!(edges.len(), 2);
        assert!(edges
            .iter()
            .any(|edge| edge.source_id == "s1" && edge.target_id == "s2"));
        assert!(edges.iter().all(|edge| edge.relation == CONNECTED_TO));
    }

    #[test]
    fn message_passing_uses_typed_relation_weights() {
        let features = BTreeMap::from([
            ("place:a".to_string(), vec![1.0, 0.0]),
            ("place:b".to_string(), vec![0.0, 1.0]),
        ]);
        let edges = vec![MorphologicalEdge::new(
            "place:a",
            MorphologicalNodeKind::Place,
            TOUCHED_TO,
            "place:b",
            MorphologicalNodeKind::Place,
        )];
        let weights = BTreeMap::from([(TOUCHED_TO.to_string(), 1.0)]);

        let out = message_pass(&features, &edges, 1, &weights).unwrap();
        assert_eq!(out["place:a"], vec![1.0, 0.0]);
        assert_eq!(out["place:b"], vec![0.5, 0.5]);
    }
}
