use std::collections::{HashMap, HashSet, VecDeque};

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
    use super::{expand_bounded, paths_shortest};

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
}
