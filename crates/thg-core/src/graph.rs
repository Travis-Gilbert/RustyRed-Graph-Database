use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use crate::graph_store::EdgeRecord;

pub type EdgeTuple = (String, String, String);

pub fn expand_bounded(
    edges: Vec<EdgeTuple>,
    seeds: Vec<String>,
    max_depth: usize,
) -> Vec<(String, usize)> {
    let adjacency = adjacency_from_edges(edges);
    let mut best_depth: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    for seed in seeds {
        if best_depth.contains_key(&seed) {
            continue;
        }
        best_depth.insert(seed.clone(), 0);
        queue.push_back((seed, 0));
    }

    while let Some((node_id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        if let Some(neighbors) = adjacency.get(&node_id) {
            for neighbor in neighbors {
                if best_depth.contains_key(neighbor) {
                    continue;
                }
                let next_depth = depth + 1;
                best_depth.insert(neighbor.clone(), next_depth);
                queue.push_back((neighbor.clone(), next_depth));
            }
        }
    }

    let mut out: Vec<(String, usize)> = best_depth.into_iter().collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    out
}

pub fn paths_shortest(
    edges: Vec<EdgeTuple>,
    source: String,
    target: String,
    max_depth: usize,
) -> Vec<String> {
    if source.is_empty() || target.is_empty() {
        return Vec::new();
    }
    if source == target {
        return vec![source];
    }

    let adjacency = adjacency_from_edges(edges);
    let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back((source.clone(), vec![source.clone()]));
    visited.insert(source);

    while let Some((node_id, path)) = queue.pop_front() {
        if path.len().saturating_sub(1) >= max_depth {
            continue;
        }
        if let Some(neighbors) = adjacency.get(&node_id) {
            for neighbor in neighbors {
                if visited.contains(neighbor) {
                    continue;
                }
                let mut next_path = path.clone();
                next_path.push(neighbor.clone());
                if neighbor == &target {
                    return next_path;
                }
                visited.insert(neighbor.clone());
                queue.push_back((neighbor.clone(), next_path));
            }
        }
    }

    Vec::new()
}

pub fn expand_bounded_weighted(
    edges: &[EdgeRecord],
    seeds: &[String],
    max_depth: usize,
    min_confidence: f64,
) -> Vec<String> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        if edge.effective_confidence() < min_confidence {
            continue;
        }
        adjacency
            .entry(edge.from_id.as_str())
            .or_default()
            .push(edge.to_id.as_str());
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    for seed in seeds {
        if visited.insert(seed.clone()) {
            queue.push_back((seed.clone(), 0));
        }
    }

    while let Some((node_id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        if let Some(neighbors) = adjacency.get(node_id.as_str()) {
            for &neighbor in neighbors {
                if visited.insert(neighbor.to_string()) {
                    queue.push_back((neighbor.to_string(), depth + 1));
                }
            }
        }
    }

    let mut out: Vec<String> = visited.into_iter().collect();
    out.sort();
    out
}

#[derive(PartialEq)]
struct WeightedNode {
    node_id: String,
    cost: f64,
}

impl Eq for WeightedNode {}

impl PartialOrd for WeightedNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WeightedNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}

pub fn paths_shortest_weighted(
    edges: &[EdgeRecord],
    source: &str,
    target: &str,
    max_depth: usize,
) -> Option<(Vec<String>, f64)> {
    if source.is_empty() || target.is_empty() {
        return None;
    }
    if source == target {
        return Some((vec![source.to_string()], 0.0));
    }

    let mut adjacency: HashMap<&str, Vec<(&str, f64)>> = HashMap::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let cost = 1.0 - edge.effective_confidence();
        adjacency
            .entry(edge.from_id.as_str())
            .or_default()
            .push((edge.to_id.as_str(), cost));
    }

    let mut best_cost: HashMap<String, f64> = HashMap::new();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut heap = BinaryHeap::new();

    best_cost.insert(source.to_string(), 0.0);
    heap.push(WeightedNode {
        node_id: source.to_string(),
        cost: 0.0,
    });

    while let Some(WeightedNode { node_id, cost }) = heap.pop() {
        if node_id == target {
            let mut path = vec![target.to_string()];
            let mut current = target.to_string();
            while let Some(p) = parent.get(&current) {
                path.push(p.clone());
                current = p.clone();
            }
            path.reverse();
            return Some((path, cost));
        }

        if let Some(&known) = best_cost.get(&node_id) {
            if cost > known {
                continue;
            }
        }

        let depth = {
            let mut d = 0usize;
            let mut cur = node_id.clone();
            while let Some(p) = parent.get(&cur) {
                d += 1;
                cur = p.clone();
            }
            d
        };
        if depth >= max_depth {
            continue;
        }

        if let Some(neighbors) = adjacency.get(node_id.as_str()) {
            for &(neighbor, edge_cost) in neighbors {
                let new_cost = cost + edge_cost;
                let neighbor_str = neighbor.to_string();
                if !best_cost.contains_key(&neighbor_str) || new_cost < best_cost[&neighbor_str] {
                    best_cost.insert(neighbor_str.clone(), new_cost);
                    parent.insert(neighbor_str.clone(), node_id.clone());
                    heap.push(WeightedNode {
                        node_id: neighbor_str,
                        cost: new_cost,
                    });
                }
            }
        }
    }

    None
}

fn adjacency_from_edges(edges: Vec<EdgeTuple>) -> HashMap<String, Vec<String>> {
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from_id, _edge_type, to_id) in edges {
        adjacency.entry(from_id).or_default().push(to_id);
    }
    for neighbors in adjacency.values_mut() {
        neighbors.sort();
        neighbors.dedup();
    }
    adjacency
}

#[cfg(test)]
mod tests {
    use super::{expand_bounded, expand_bounded_weighted, paths_shortest, paths_shortest_weighted};
    use crate::graph_store::EdgeRecord;
    use serde_json::json;

    #[test]
    fn expand_bounded_returns_depth_ordered_nodes() {
        let result = expand_bounded(
            vec![
                (
                    "task:1".to_string(),
                    "REQUIRES".to_string(),
                    "skill:search".to_string(),
                ),
                (
                    "skill:search".to_string(),
                    "HAS_TOOL".to_string(),
                    "tool:web".to_string(),
                ),
                (
                    "tool:web".to_string(),
                    "VALIDATED_BY".to_string(),
                    "validator:json".to_string(),
                ),
            ],
            vec!["task:1".to_string()],
            2,
        );

        assert_eq!(
            result,
            vec![
                ("task:1".to_string(), 0),
                ("skill:search".to_string(), 1),
                ("tool:web".to_string(), 2),
            ]
        );
    }

    #[test]
    fn paths_shortest_returns_directed_path() {
        let result = paths_shortest(
            vec![
                (
                    "task:1".to_string(),
                    "REQUIRES".to_string(),
                    "skill:search".to_string(),
                ),
                (
                    "skill:search".to_string(),
                    "HAS_TOOL".to_string(),
                    "tool:web".to_string(),
                ),
                (
                    "task:1".to_string(),
                    "REQUIRES".to_string(),
                    "tool:slow".to_string(),
                ),
            ],
            "task:1".to_string(),
            "tool:web".to_string(),
            3,
        );

        assert_eq!(
            result,
            vec![
                "task:1".to_string(),
                "skill:search".to_string(),
                "tool:web".to_string()
            ]
        );
    }

    fn make_edge(id: &str, from: &str, to: &str, confidence: Option<f64>) -> EdgeRecord {
        let mut e = EdgeRecord::new(id, from, "RELATED", to, json!({}));
        e.confidence = confidence;
        e
    }

    #[test]
    fn expand_bounded_weighted_filters_low_confidence() {
        let edges = vec![
            make_edge("e1", "a", "b", Some(0.9)),
            make_edge("e2", "b", "c", Some(0.2)),
            make_edge("e3", "a", "d", Some(0.8)),
        ];
        let result = expand_bounded_weighted(&edges, &["a".to_string()], 3, 0.5);
        assert!(result.contains(&"a".to_string()));
        assert!(result.contains(&"b".to_string()));
        assert!(result.contains(&"d".to_string()));
        assert!(!result.contains(&"c".to_string()));
    }

    #[test]
    fn expand_bounded_weighted_treats_none_as_1() {
        let edges = vec![make_edge("e1", "a", "b", None)];
        let result = expand_bounded_weighted(&edges, &["a".to_string()], 1, 0.5);
        assert!(result.contains(&"b".to_string()));
    }

    #[test]
    fn paths_shortest_weighted_returns_path_and_cost() {
        let edges = vec![
            make_edge("e1", "a", "b", Some(0.9)),
            make_edge("e2", "b", "c", Some(0.8)),
        ];
        let result = paths_shortest_weighted(&edges, "a", "c", 5);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert_eq!(path, vec!["a", "b", "c"]);
        let expected_cost = (1.0 - 0.9) + (1.0 - 0.8);
        assert!((cost - expected_cost).abs() < 1e-10);
    }

    #[test]
    fn paths_shortest_weighted_prefers_high_confidence() {
        let edges = vec![
            make_edge("e1", "a", "b", Some(0.5)),
            make_edge("e2", "b", "d", Some(0.5)),
            make_edge("e3", "a", "c", Some(0.99)),
            make_edge("e4", "c", "d", Some(0.99)),
        ];
        let result = paths_shortest_weighted(&edges, "a", "d", 5).unwrap();
        assert_eq!(result.0, vec!["a", "c", "d"]);
    }

    #[test]
    fn paths_shortest_weighted_same_source_target() {
        let edges = vec![make_edge("e1", "a", "b", Some(0.9))];
        let result = paths_shortest_weighted(&edges, "a", "a", 5).unwrap();
        assert_eq!(result.0, vec!["a"]);
        assert!((result.1 - 0.0).abs() < 1e-10);
    }
}
