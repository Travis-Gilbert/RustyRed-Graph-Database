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

// ===== Phase 6: Graph algorithms =====

/// ACL local-push Personalized PageRank (matches `theseus_native::push_ppr`).
///
/// `adjacency` maps source node id to `(target, weight)` pairs. `seeds` maps
/// node id to initial residual mass; values should sum to ~1.0. Returns
/// approximate PPR scores for nodes touched during the push.
///
/// Reference: Andersen, Chung, Lang (FOCS 2006).
pub fn personalized_pagerank(
    adjacency: &HashMap<String, Vec<(String, f64)>>,
    seeds: &HashMap<String, f64>,
    alpha: f64,
    epsilon: f64,
    max_pushes: usize,
) -> HashMap<String, f64> {
    let mut p: HashMap<String, f64> = HashMap::new();
    let mut r: HashMap<String, f64> = seeds.clone();
    let mut pushes = 0usize;

    loop {
        if pushes >= max_pushes {
            break;
        }
        // Pick the node with the largest r/deg ratio above threshold.
        let mut target: Option<(String, f64, f64)> = None;
        for (node, &residual) in r.iter() {
            let degree = adjacency.get(node).map(|n| n.len()).unwrap_or(0) as f64;
            let deg_eff = if degree > 0.0 { degree } else { 1.0 };
            if residual / deg_eff <= epsilon {
                continue;
            }
            if target
                .as_ref()
                .map(|(_, _, prev)| residual / deg_eff > *prev)
                .unwrap_or(true)
            {
                target = Some((node.clone(), residual, residual / deg_eff));
            }
        }

        let Some((u, residual_u, _)) = target else {
            break;
        };

        *p.entry(u.clone()).or_insert(0.0) += alpha * residual_u;
        r.insert(u.clone(), 0.0);
        if let Some(neighbors) = adjacency.get(&u) {
            let degree = neighbors.len() as f64;
            if degree > 0.0 {
                let push_amount = (1.0 - alpha) * residual_u / degree;
                for (target_node, _weight) in neighbors {
                    *r.entry(target_node.clone()).or_insert(0.0) += push_amount;
                }
            }
        }
        pushes += 1;
    }
    p
}

/// Connected components on the (optionally directed) graph.
/// Returns components as lists of node ids (sorted within each component).
pub fn connected_components(edges: &[EdgeRecord], directed: bool) -> Vec<Vec<String>> {
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_nodes: HashSet<String> = HashSet::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        all_nodes.insert(edge.from_id.clone());
        all_nodes.insert(edge.to_id.clone());
        adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .push(edge.to_id.clone());
        if !directed {
            adjacency
                .entry(edge.to_id.clone())
                .or_default()
                .push(edge.from_id.clone());
        }
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut components: Vec<Vec<String>> = Vec::new();
    for start in all_nodes.iter() {
        if visited.contains(start) {
            continue;
        }
        let mut component: Vec<String> = Vec::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(start.clone());
        visited.insert(start.clone());
        while let Some(node) = queue.pop_front() {
            component.push(node.clone());
            if let Some(neighbors) = adjacency.get(&node) {
                for neighbor in neighbors {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    components
}

/// Power-iteration PageRank. Returns score per node id, sums to 1.0.
pub fn pagerank(
    edges: &[EdgeRecord],
    damping: f64,
    max_iter: usize,
    tolerance: f64,
) -> HashMap<String, f64> {
    let mut nodes: HashSet<String> = HashSet::new();
    let mut out_links: HashMap<String, Vec<String>> = HashMap::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        nodes.insert(edge.from_id.clone());
        nodes.insert(edge.to_id.clone());
        out_links
            .entry(edge.from_id.clone())
            .or_default()
            .push(edge.to_id.clone());
    }
    if nodes.is_empty() {
        return HashMap::new();
    }
    let n = nodes.len() as f64;
    let init = 1.0 / n;
    let mut rank: HashMap<String, f64> = nodes.iter().map(|id| (id.clone(), init)).collect();

    for _ in 0..max_iter {
        let mut new_rank: HashMap<String, f64> = nodes.iter().map(|id| (id.clone(), 0.0)).collect();
        let mut dangling = 0.0;
        for node in nodes.iter() {
            let out = out_links.get(node).map(|v| v.len()).unwrap_or(0);
            let mass = rank[node];
            if out == 0 {
                dangling += mass;
            } else {
                let share = mass / out as f64;
                for target in &out_links[node] {
                    if let Some(slot) = new_rank.get_mut(target) {
                        *slot += share;
                    }
                }
            }
        }
        let teleport = (1.0 - damping) / n + damping * dangling / n;
        for (_id, score) in new_rank.iter_mut() {
            *score = teleport + damping * *score;
        }
        let mut delta = 0.0;
        for id in nodes.iter() {
            delta += (new_rank[id] - rank[id]).abs();
        }
        rank = new_rank;
        if delta < tolerance {
            break;
        }
    }
    rank
}

/// Community detection via Raghavan-Albert-Kumara label propagation on the
/// undirected, edge-weight-aware projection. Cheap, deterministic, and
/// adequate for showing cluster structure in mid-sized graphs.
/// Returns (node_id -> community_id, modularity).
pub fn louvain_communities(edges: &[EdgeRecord]) -> (HashMap<String, u64>, f64) {
    let mut adjacency: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut total_weight = 0.0;
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let w = edge.effective_confidence().max(1e-6);
        *adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .entry(edge.to_id.clone())
            .or_insert(0.0) += w;
        *adjacency
            .entry(edge.to_id.clone())
            .or_default()
            .entry(edge.from_id.clone())
            .or_insert(0.0) += w;
        total_weight += 2.0 * w;
    }

    let mut nodes: Vec<String> = adjacency.keys().cloned().collect();
    nodes.sort();

    // Each node starts in its own community.
    let mut label: HashMap<String, u64> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i as u64))
        .collect();

    let strength: HashMap<String, f64> = adjacency
        .iter()
        .map(|(n, neighbors)| (n.clone(), neighbors.values().sum()))
        .collect();

    // Synchronous-ish label propagation: for each node pick the community
    // that maximizes the weighted vote among neighbors. Ties broken by the
    // lowest community id (deterministic).
    for _ in 0..32 {
        let mut changed = false;
        let snapshot = label.clone();
        for node in nodes.iter() {
            let Some(neighbors) = adjacency.get(node) else {
                continue;
            };
            if neighbors.is_empty() {
                continue;
            }
            let mut votes: HashMap<u64, f64> = HashMap::new();
            for (neighbor, w) in neighbors.iter() {
                let c = snapshot.get(neighbor).copied().unwrap_or(0);
                *votes.entry(c).or_insert(0.0) += w;
            }
            let mut entries: Vec<(u64, f64)> = votes.into_iter().collect();
            entries.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
            let best = entries[0].0;
            if label[node] != best {
                label.insert(node.clone(), best);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Compact community ids to 0..k for prettier output.
    let mut compact: HashMap<u64, u64> = HashMap::new();
    let mut next = 0u64;
    let mut community: HashMap<String, u64> = HashMap::with_capacity(label.len());
    for (node, c) in label.iter() {
        let id = *compact.entry(*c).or_insert_with(|| {
            let v = next;
            next += 1;
            v
        });
        community.insert(node.clone(), id);
    }

    // Newman-Girvan modularity on the resulting partition.
    let mut modularity = 0.0;
    for (u, neighbors) in adjacency.iter() {
        let cu = community[u];
        let ku = *strength.get(u).unwrap_or(&0.0);
        for (v, w) in neighbors.iter() {
            let cv = *community.get(v).unwrap_or(&u64::MAX);
            if cu == cv {
                let kv = *strength.get(v).unwrap_or(&0.0);
                modularity += w - ku * kv / total_weight.max(1e-9);
            }
        }
    }
    if total_weight > 0.0 {
        modularity /= total_weight;
    }
    (community, modularity)
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

    use super::{connected_components, louvain_communities, pagerank, personalized_pagerank};
    use std::collections::HashMap;

    #[test]
    fn ppr_concentrates_mass_near_seed() {
        // a -> b -> c, seed at a
        let mut adj: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        adj.insert("a".into(), vec![("b".into(), 1.0)]);
        adj.insert("b".into(), vec![("c".into(), 1.0)]);
        adj.insert("c".into(), vec![]);
        let mut seeds = HashMap::new();
        seeds.insert("a".to_string(), 1.0);
        let scores = personalized_pagerank(&adj, &seeds, 0.15, 1e-5, 10_000);
        // a should be the highest-scoring node
        let mut entries: Vec<_> = scores.iter().collect();
        entries.sort_by(|x, y| y.1.partial_cmp(x.1).unwrap());
        assert_eq!(entries[0].0, "a");
        assert!(*entries[0].1 > 0.0);
    }

    #[test]
    fn connected_components_partitions_disconnected_graph() {
        let edges = vec![
            make_edge("e1", "a", "b", None),
            make_edge("e2", "b", "c", None),
            make_edge("e3", "x", "y", None),
        ];
        let comps = connected_components(&edges, false);
        assert_eq!(comps.len(), 2);
        // first component is the larger one
        assert_eq!(comps[0].len(), 3);
        assert_eq!(comps[1].len(), 2);
    }

    #[test]
    fn pagerank_sums_to_one_and_converges() {
        let edges = vec![
            make_edge("e1", "a", "b", None),
            make_edge("e2", "b", "c", None),
            make_edge("e3", "c", "a", None),
            make_edge("e4", "a", "c", None),
        ];
        let rank = pagerank(&edges, 0.85, 100, 1e-6);
        assert_eq!(rank.len(), 3);
        let total: f64 = rank.values().sum();
        assert!((total - 1.0).abs() < 1e-3, "total mass = {total}");
    }

    #[test]
    fn louvain_finds_two_clusters() {
        // Two triangles connected by a single weak edge.
        let edges = vec![
            // cluster 1
            make_edge("e1", "a", "b", Some(0.9)),
            make_edge("e2", "b", "c", Some(0.9)),
            make_edge("e3", "a", "c", Some(0.9)),
            // cluster 2
            make_edge("e4", "x", "y", Some(0.9)),
            make_edge("e5", "y", "z", Some(0.9)),
            make_edge("e6", "x", "z", Some(0.9)),
            // bridge
            make_edge("e7", "a", "x", Some(0.1)),
        ];
        let (community, modularity) = louvain_communities(&edges);
        assert_eq!(community.len(), 6);
        // a, b, c share a community
        assert_eq!(community["a"], community["b"]);
        assert_eq!(community["b"], community["c"]);
        // x, y, z share a community
        assert_eq!(community["x"], community["y"]);
        assert_eq!(community["y"], community["z"]);
        // the two communities are different
        assert_ne!(community["a"], community["x"]);
        assert!(modularity > 0.0);
    }
}
