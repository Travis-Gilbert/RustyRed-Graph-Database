//! Rusty Red standalone Rust helper facade.
//!
//! The public release does not require Python or native extension bindings.
//! Product HTTP, MCP, and direct Rust callers all use the same `rustyred-core`
//! graph algorithms.

use std::collections::HashMap;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub type NodeId = i64;
pub type WeightedAdjacency = HashMap<NodeId, Vec<(NodeId, f64)>>;
pub type SeedMap = HashMap<NodeId, f64>;
pub type ScoreMap = HashMap<NodeId, f64>;

pub use rustyred_core::{
    connected_components, label_propagation_communities, pagerank, personalized_pagerank,
};

/// Run ACL local-push Personalized PageRank with integer node identifiers.
///
/// This is a pure Rust convenience wrapper over `rustyred-core` for callers
/// that do not need to construct full graph-store records.
pub fn push_ppr(
    adjacency: &WeightedAdjacency,
    seeds: &SeedMap,
    alpha: f64,
    epsilon: f64,
    max_pushes: usize,
) -> ScoreMap {
    let string_adjacency: HashMap<String, Vec<(String, f64)>> = adjacency
        .iter()
        .map(|(source, neighbors)| {
            (
                source.to_string(),
                neighbors
                    .iter()
                    .map(|(target, weight)| (target.to_string(), *weight))
                    .collect(),
            )
        })
        .collect();
    let string_seeds: HashMap<String, f64> = seeds
        .iter()
        .map(|(node, mass)| (node.to_string(), *mass))
        .collect();

    rustyred_core::personalized_pagerank(
        &string_adjacency,
        &string_seeds,
        alpha,
        epsilon,
        max_pushes,
    )
    .into_iter()
    .filter_map(|(node, score)| node.parse::<NodeId>().ok().map(|id| (id, score)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_package_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn push_ppr_matches_core_for_integer_ids() {
        let adjacency = HashMap::from([
            (0, vec![(1, 1.0)]),
            (1, vec![(2, 1.0)]),
            (2, vec![(0, 1.0)]),
        ]);
        let seeds = HashMap::from([(0, 1.0)]);

        let int_scores = push_ppr(&adjacency, &seeds, 0.15, 1e-4, 20_000);

        let core_adjacency = HashMap::from([
            ("0".to_string(), vec![("1".to_string(), 1.0)]),
            ("1".to_string(), vec![("2".to_string(), 1.0)]),
            ("2".to_string(), vec![("0".to_string(), 1.0)]),
        ]);
        let core_seeds = HashMap::from([("0".to_string(), 1.0)]);
        let core_scores =
            rustyred_core::personalized_pagerank(&core_adjacency, &core_seeds, 0.15, 1e-4, 20_000);

        assert_eq!(int_scores.len(), core_scores.len());
        for (node, score) in int_scores {
            let core_score = core_scores
                .get(&node.to_string())
                .expect("core score for integer node");
            assert!((score - core_score).abs() < 1e-12);
        }
    }
}
