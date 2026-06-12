//! Tier-1 graph algorithms, clean-room from the published literature.
//!
//! Every algorithm here is implemented from its source paper, not from any
//! existing library. The GDS project is used only as an API/scope map (which
//! algorithms, which parameter names); none of its GPLv3 code is used, and this
//! crate stays MIT.
//!
//! - Strongly connected components: Tarjan 1972 (iterative transcription).
//! - Topological sort over the condensation: Kahn 1962.
//! - Betweenness centrality: Brandes 2001 (exact and pivot-sampled).
//! - Articulation points and bridges: Hopcroft/Tarjan DFS lowpoint, one pass.
//! - Leiden community detection: Traag, Waltman, van Eck 2019 (local moving,
//!   refinement, aggregation; the connectivity guarantee is enforced).
//! - Node similarity (Jaccard, Overlap) and link-prediction features
//!   (common neighbors, Adamic/Adar, resource allocation): standard set
//!   formulas over neighbor sets.
//!
//! All functions are pure over `&[EdgeRecord]` (tombstoned edges are skipped)
//! and deterministic: results are stably ordered, and randomized phases take an
//! explicit `seed`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::graph_store::EdgeRecord;

// ===== shared helpers =====

/// Distinct live node ids, lexicographically sorted (stable indexing).
fn collect_nodes(edges: &[EdgeRecord]) -> Vec<String> {
    let mut set: BTreeSet<&str> = BTreeSet::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        set.insert(edge.from_id.as_str());
        set.insert(edge.to_id.as_str());
    }
    set.into_iter().map(str::to_string).collect()
}

fn index_nodes(nodes: &[String]) -> HashMap<&str, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect()
}

/// Simple (deduplicated) adjacency over node indices. Parallel edges collapse;
/// self-loops are kept only when `keep_self` (betweenness/SCC ignore them).
fn build_simple_adjacency(
    edges: &[EdgeRecord],
    index_of: &HashMap<&str, usize>,
    n: usize,
    directed: bool,
) -> Vec<Vec<usize>> {
    let mut sets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let (Some(&u), Some(&v)) = (
            index_of.get(edge.from_id.as_str()),
            index_of.get(edge.to_id.as_str()),
        ) else {
            continue;
        };
        if u == v {
            continue;
        }
        sets[u].insert(v);
        if !directed {
            sets[v].insert(u);
        }
    }
    sets.into_iter()
        .map(|set| set.into_iter().collect())
        .collect()
}

fn order_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

// ===== Strongly connected components: Tarjan 1972 =====

/// Tarjan's strongly connected components on the directed graph.
///
/// Iterative transcription of the 1972 lowlink algorithm. Each component's
/// members are sorted; components are ordered largest-first then
/// lexicographically (matching `connected_components`).
pub fn strongly_connected_components(edges: &[EdgeRecord]) -> Vec<Vec<String>> {
    let nodes = collect_nodes(edges);
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let index_of = index_nodes(&nodes);
    let adj = build_simple_adjacency(edges, &index_of, n, true);

    let mut index: Vec<i64> = vec![-1; n];
    let mut low: Vec<i64> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut tarjan_stack: Vec<usize> = Vec::new();
    let mut next_index: i64 = 0;
    let mut components: Vec<Vec<String>> = Vec::new();

    for start in 0..n {
        if index[start] >= 0 {
            continue;
        }
        // Explicit DFS frames: (node, next-neighbor position).
        let mut call: Vec<(usize, usize)> = Vec::new();
        index[start] = next_index;
        low[start] = next_index;
        next_index += 1;
        tarjan_stack.push(start);
        on_stack[start] = true;
        call.push((start, 0));

        while let Some(&(v, pos)) = call.last() {
            if pos < adj[v].len() {
                let w = adj[v][pos];
                call.last_mut().unwrap().1 += 1;
                if index[w] < 0 {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    tarjan_stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] && index[w] < low[v] {
                    low[v] = index[w];
                }
            } else {
                if low[v] == index[v] {
                    let mut component = Vec::new();
                    loop {
                        let w = tarjan_stack.pop().unwrap();
                        on_stack[w] = false;
                        component.push(nodes[w].clone());
                        if w == v {
                            break;
                        }
                    }
                    component.sort();
                    components.push(component);
                }
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    if low[v] < low[parent] {
                        low[parent] = low[v];
                    }
                }
            }
        }
    }

    components.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    components
}

/// The condensation of the directed graph: each SCC collapsed to a super-node,
/// with the induced acyclic super-edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condensation {
    /// SCCs, indexed by component id; each is a sorted list of node ids.
    pub components: Vec<Vec<String>>,
    /// node id -> component id.
    pub component_of: HashMap<String, usize>,
    /// Unique directed super-edges `(from_component, to_component)`, sorted.
    pub edges: Vec<(usize, usize)>,
}

pub fn condense(edges: &[EdgeRecord]) -> Condensation {
    let components = strongly_connected_components(edges);
    let mut component_of: HashMap<String, usize> = HashMap::new();
    for (cid, members) in components.iter().enumerate() {
        for node in members {
            component_of.insert(node.clone(), cid);
        }
    }
    let mut dag: BTreeSet<(usize, usize)> = BTreeSet::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let (Some(&cu), Some(&cv)) = (
            component_of.get(&edge.from_id),
            component_of.get(&edge.to_id),
        ) else {
            continue;
        };
        if cu != cv {
            dag.insert((cu, cv));
        }
    }
    Condensation {
        components,
        component_of,
        edges: dag.into_iter().collect(),
    }
}

/// A cycle was found where an acyclic order was required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleError {
    pub nodes_ordered: usize,
    pub nodes_total: usize,
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "graph is cyclic: ordered {} of {} nodes before a cycle blocked progress",
            self.nodes_ordered, self.nodes_total
        )
    }
}

impl std::error::Error for CycleError {}

/// Kahn topological sort over the node graph. Errors if the graph has a cycle
/// (including self-loops). Deterministic: ties break on the lexicographically
/// smallest available node.
pub fn topological_sort(edges: &[EdgeRecord]) -> Result<Vec<String>, CycleError> {
    let nodes = collect_nodes(edges);
    let mut indegree: HashMap<&str, usize> = nodes.iter().map(|n| (n.as_str(), 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        adj.entry(edge.from_id.as_str())
            .or_default()
            .push(edge.to_id.as_str());
        *indegree.entry(edge.to_id.as_str()).or_insert(0) += 1;
    }
    let mut ready: BTreeSet<&str> = nodes
        .iter()
        .filter(|n| indegree[n.as_str()] == 0)
        .map(|n| n.as_str())
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    while let Some(&u) = ready.iter().next() {
        ready.remove(u);
        order.push(u.to_string());
        if let Some(neighbors) = adj.get(u) {
            for &w in neighbors {
                let degree = indegree.get_mut(w).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(w);
                }
            }
        }
    }
    if order.len() == nodes.len() {
        Ok(order)
    } else {
        Err(CycleError {
            nodes_ordered: order.len(),
            nodes_total: nodes.len(),
        })
    }
}

/// Topological order over the condensation: the SCCs in a valid build order.
/// Always succeeds, since the condensation is acyclic by construction.
pub fn topological_sort_condensation(edges: &[EdgeRecord]) -> Vec<Vec<String>> {
    let cond = condense(edges);
    let n = cond.components.len();
    let mut indegree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in &cond.edges {
        adj[u].push(v);
        indegree[v] += 1;
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order: Vec<Vec<String>> = Vec::with_capacity(n);
    while let Some(&c) = ready.iter().next() {
        ready.remove(&c);
        order.push(cond.components[c].clone());
        for &w in &adj[c] {
            indegree[w] -= 1;
            if indegree[w] == 0 {
                ready.insert(w);
            }
        }
    }
    order
}

// ===== Betweenness centrality: Brandes 2001 =====

/// Exact betweenness centrality (unweighted) via Brandes 2001. For undirected
/// graphs each pair is counted once (the accumulated value is halved).
pub fn betweenness_centrality(edges: &[EdgeRecord], directed: bool) -> HashMap<String, f64> {
    let nodes = collect_nodes(edges);
    let n = nodes.len();
    let mut result: HashMap<String, f64> = nodes.iter().map(|x| (x.clone(), 0.0)).collect();
    if n == 0 {
        return result;
    }
    let index_of = index_nodes(&nodes);
    let adj = build_simple_adjacency(edges, &index_of, n, directed);
    let mut betweenness = vec![0.0f64; n];
    for s in 0..n {
        brandes_single_source(s, &adj, &mut betweenness);
    }
    let scale = if directed { 1.0 } else { 0.5 };
    for (i, node) in nodes.iter().enumerate() {
        result.insert(node.clone(), betweenness[i] * scale);
    }
    result
}

/// Pivot-sampled betweenness (Brandes/Pich estimator): run the single-source
/// dependency accumulation from `sample_size` randomly chosen pivots and scale
/// by `n / sample_size`. `seed` makes pivot selection deterministic. With
/// `sample_size >= n` this equals the exact result.
pub fn betweenness_centrality_sampled(
    edges: &[EdgeRecord],
    directed: bool,
    sample_size: usize,
    seed: u64,
) -> HashMap<String, f64> {
    let nodes = collect_nodes(edges);
    let n = nodes.len();
    let mut result: HashMap<String, f64> = nodes.iter().map(|x| (x.clone(), 0.0)).collect();
    if n == 0 {
        return result;
    }
    let index_of = index_nodes(&nodes);
    let adj = build_simple_adjacency(edges, &index_of, n, directed);
    let k = sample_size.clamp(1, n);
    let pivots = sample_distinct_indices(n, k, seed);
    let mut betweenness = vec![0.0f64; n];
    for &s in &pivots {
        brandes_single_source(s, &adj, &mut betweenness);
    }
    let scale = (n as f64 / k as f64) * if directed { 1.0 } else { 0.5 };
    for (i, node) in nodes.iter().enumerate() {
        result.insert(node.clone(), betweenness[i] * scale);
    }
    result
}

fn brandes_single_source(s: usize, adj: &[Vec<usize>], betweenness: &mut [f64]) {
    let n = adj.len();
    let mut order: Vec<usize> = Vec::new();
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut sigma = vec![0.0f64; n];
    let mut distance = vec![-1i64; n];
    sigma[s] = 1.0;
    distance[s] = 0;
    let mut queue = VecDeque::new();
    queue.push_back(s);
    while let Some(v) = queue.pop_front() {
        order.push(v);
        for &w in &adj[v] {
            if distance[w] < 0 {
                distance[w] = distance[v] + 1;
                queue.push_back(w);
            }
            if distance[w] == distance[v] + 1 {
                sigma[w] += sigma[v];
                predecessors[w].push(v);
            }
        }
    }
    let mut delta = vec![0.0f64; n];
    while let Some(w) = order.pop() {
        for &v in &predecessors[w] {
            delta[v] += (sigma[v] / sigma[w]) * (1.0 + delta[w]);
        }
        if w != s {
            betweenness[w] += delta[w];
        }
    }
}

/// SplitMix64 — a tiny deterministic PRNG so randomized phases need no external
/// crate and replay exactly from a seed.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `k` distinct indices in `0..n` via a partial Fisher-Yates shuffle seeded by
/// `seed`. Deterministic for a given `(n, k, seed)`.
fn sample_distinct_indices(n: usize, k: usize, seed: u64) -> Vec<usize> {
    let take = k.min(n);
    let mut permutation: Vec<usize> = (0..n).collect();
    let mut state = seed ^ 0xD1B5_4A32_D192_ED03;
    for i in 0..take {
        let j = i + (splitmix64(&mut state) as usize) % (n - i);
        permutation.swap(i, j);
    }
    permutation.truncate(take);
    permutation
}

// ===== Articulation points and bridges: DFS lowpoint, one pass =====

struct LowpointFrame {
    node: usize,
    parent_edge: usize,
    next: usize,
    children: usize,
}

const NO_PARENT_EDGE: usize = usize::MAX;

/// Articulation points (cut vertices) and bridges (cut edges) of the undirected
/// projection, found in a single DFS computing discovery/low values
/// (Hopcroft-Tarjan). Parallel edges are tracked by a per-instance id, so a
/// second parallel edge correctly prevents a bridge. Bridges are returned as
/// lexicographically ordered, deduplicated pairs.
pub fn articulation_points_and_bridges(
    edges: &[EdgeRecord],
) -> (BTreeSet<String>, Vec<(String, String)>) {
    let nodes = collect_nodes(edges);
    let n = nodes.len();
    let mut points: BTreeSet<String> = BTreeSet::new();
    let mut bridges: Vec<(String, String)> = Vec::new();
    if n == 0 {
        return (points, bridges);
    }
    let index_of = index_nodes(&nodes);

    // Undirected adjacency with a distinct id per edge instance.
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut edge_uid = 0usize;
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let (Some(&u), Some(&v)) = (
            index_of.get(edge.from_id.as_str()),
            index_of.get(edge.to_id.as_str()),
        ) else {
            continue;
        };
        if u == v {
            continue;
        }
        adj[u].push((v, edge_uid));
        adj[v].push((u, edge_uid));
        edge_uid += 1;
    }
    for neighbors in adj.iter_mut() {
        neighbors.sort_unstable();
    }

    let mut disc: Vec<i64> = vec![-1; n];
    let mut low: Vec<i64> = vec![0; n];
    let mut timer: i64 = 0;

    for start in 0..n {
        if disc[start] >= 0 {
            continue;
        }
        disc[start] = timer;
        low[start] = timer;
        timer += 1;
        let mut stack: Vec<LowpointFrame> = vec![LowpointFrame {
            node: start,
            parent_edge: NO_PARENT_EDGE,
            next: 0,
            children: 0,
        }];

        while let Some(top) = stack.last_mut() {
            let v = top.node;
            if top.next < adj[v].len() {
                let (w, uid) = adj[v][top.next];
                top.next += 1;
                if uid == top.parent_edge {
                    continue; // skip the single edge we entered v through
                }
                if disc[w] < 0 {
                    top.children += 1;
                    disc[w] = timer;
                    low[w] = timer;
                    timer += 1;
                    stack.push(LowpointFrame {
                        node: w,
                        parent_edge: uid,
                        next: 0,
                        children: 0,
                    });
                } else if disc[w] < low[v] {
                    low[v] = disc[w];
                }
            } else {
                let frame = stack.pop().unwrap();
                let v = frame.node;
                if let Some(parent) = stack.last() {
                    let p = parent.node;
                    let parent_is_root = parent.parent_edge == NO_PARENT_EDGE;
                    if low[v] < low[p] {
                        low[p] = low[v];
                    }
                    if !parent_is_root && low[v] >= disc[p] {
                        points.insert(nodes[p].clone());
                    }
                    if low[v] > disc[p] {
                        bridges.push(order_pair(&nodes[p], &nodes[v]));
                    }
                } else if frame.children > 1 {
                    // Root is an articulation point iff it has >1 DFS child.
                    points.insert(nodes[v].clone());
                }
            }
        }
    }

    bridges.sort();
    bridges.dedup();
    (points, bridges)
}

// ===== Leiden community detection: Traag, Waltman, van Eck 2019 =====

/// Result of a Leiden run.
#[derive(Clone, Debug, PartialEq)]
pub struct LeidenResult {
    /// node id -> community id (compacted to `0..k`).
    pub community: HashMap<String, u64>,
    /// Modularity of the final partition at the given resolution.
    pub modularity: f64,
    /// Number of aggregation levels performed.
    pub levels: usize,
}

/// Internal undirected weighted graph over node indices.
struct WeightedGraph {
    n: usize,
    adj: Vec<Vec<(usize, f64)>>,
    self_loop: Vec<f64>,
    degree: Vec<f64>,
    total_weight: f64, // m: sum of edge weights (each undirected edge once)
}

impl WeightedGraph {
    fn from_edges(edges: &[EdgeRecord]) -> (Self, Vec<String>) {
        let nodes = collect_nodes(edges);
        let n = nodes.len();
        let index_of = index_nodes(&nodes);
        let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
        let mut self_loop = vec![0.0f64; n];
        for edge in edges {
            if edge.tombstone {
                continue;
            }
            let (Some(&u), Some(&v)) = (
                index_of.get(edge.from_id.as_str()),
                index_of.get(edge.to_id.as_str()),
            ) else {
                continue;
            };
            let w = edge.effective_confidence().max(1e-9);
            if u == v {
                self_loop[u] += w;
            } else {
                let key = if u < v { (u, v) } else { (v, u) };
                *weights.entry(key).or_insert(0.0) += w;
            }
        }
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        for (&(u, v), &w) in &weights {
            adj[u].push((v, w));
            adj[v].push((u, w));
        }
        let mut degree = vec![0.0f64; n];
        let mut total_weight = 0.0;
        for u in 0..n {
            let incident: f64 = adj[u].iter().map(|(_, w)| *w).sum();
            degree[u] = incident + 2.0 * self_loop[u];
            total_weight += incident / 2.0 + self_loop[u];
        }
        (
            Self {
                n,
                adj,
                self_loop,
                degree,
                total_weight,
            },
            nodes,
        )
    }

    fn two_m(&self) -> f64 {
        (2.0 * self.total_weight).max(1e-12)
    }
}

/// Run Leiden at resolution `gamma`. `seed` controls node visiting order in the
/// randomized phases; `max_levels` caps aggregation. Communities are guaranteed
/// connected (the refinement phase plus a final connectivity split enforce the
/// Leiden guarantee).
pub fn leiden(edges: &[EdgeRecord], gamma: f64, seed: u64, max_levels: usize) -> LeidenResult {
    let (mut graph, nodes) = WeightedGraph::from_edges(edges);
    let n0 = graph.n;
    if n0 == 0 {
        return LeidenResult {
            community: HashMap::new(),
            modularity: 0.0,
            levels: 0,
        };
    }

    let mut rng_state = seed ^ 0x2545_F491_4F6C_DD1D;
    let mut orig_to_current: Vec<usize> = (0..n0).collect();
    let mut partition: Vec<usize> = (0..graph.n).collect();
    let mut levels = 0usize;

    let level_cap = max_levels.max(1);
    for _ in 0..level_cap {
        levels += 1;
        local_move(&graph, &mut partition, gamma, &mut rng_state);
        let refined = refine(&graph, &partition, gamma, &mut rng_state);
        let (aggregated, super_of, super_partition) = aggregate(&graph, &refined, &partition);
        if aggregated.n == graph.n {
            break; // no community merged: converged
        }
        for slot in orig_to_current.iter_mut() {
            *slot = super_of[*slot];
        }
        graph = aggregated;
        partition = super_partition;
    }

    // Map original nodes onto their final coarse community.
    let mut raw_community: Vec<usize> = vec![0; n0];
    for (orig, slot) in raw_community.iter_mut().enumerate() {
        *slot = partition[orig_to_current[orig]];
    }

    // Enforce the Leiden connectivity guarantee: split any community whose
    // induced subgraph is disconnected. Faithful refinement makes this a no-op;
    // it is a safety net that makes the guarantee unconditional.
    let split = enforce_connected_communities(edges, &nodes, &raw_community);

    // Compact ids to 0..k in first-appearance order.
    let mut remap: HashMap<usize, u64> = HashMap::new();
    let mut community: HashMap<String, u64> = HashMap::with_capacity(n0);
    let mut next_id = 0u64;
    for (i, node) in nodes.iter().enumerate() {
        let id = *remap.entry(split[i]).or_insert_with(|| {
            let v = next_id;
            next_id += 1;
            v
        });
        community.insert(node.clone(), id);
    }

    let modularity = partition_modularity(edges, &community, gamma);
    LeidenResult {
        community,
        modularity,
        levels,
    }
}

/// Greedy local moving: repeatedly move each node to the neighboring community
/// that most increases modularity at resolution `gamma`, until stable.
fn local_move(graph: &WeightedGraph, community: &mut [usize], gamma: f64, rng: &mut u64) {
    let n = graph.n;
    if n == 0 {
        return;
    }
    let two_m = graph.two_m();
    let mut community_tot = community_totals(graph, community);
    let order = shuffled_order(n, rng);

    let mut improved = true;
    let mut passes = 0;
    while improved && passes < 64 {
        improved = false;
        passes += 1;
        for &u in &order {
            let cu = community[u];
            let ku = graph.degree[u];
            // weight from u into each neighboring community
            let mut to_community: HashMap<usize, f64> = HashMap::new();
            for &(v, w) in &graph.adj[u] {
                *to_community.entry(community[v]).or_insert(0.0) += w;
            }
            // remove u from its community
            community_tot[cu] -= ku;
            let k_u_cu = *to_community.get(&cu).unwrap_or(&0.0);

            let mut best_community = cu;
            let mut best_gain = k_u_cu - gamma * ku * community_tot[cu] / two_m;
            for (&c, &k_u_c) in &to_community {
                if c == cu {
                    continue;
                }
                let gain = k_u_c - gamma * ku * community_tot[c] / two_m;
                if gain > best_gain + 1e-12 {
                    best_gain = gain;
                    best_community = c;
                }
            }
            community_tot[best_community] += ku;
            if best_community != cu {
                community[u] = best_community;
                improved = true;
            }
        }
    }
}

/// Refinement: within each community of `partition`, start from singletons and
/// merge nodes only into a refined sub-community they connect to and that
/// improves modularity. Because a node only ever joins a refined community it
/// has an edge into, every refined community is connected.
fn refine(graph: &WeightedGraph, partition: &[usize], gamma: f64, rng: &mut u64) -> Vec<usize> {
    let n = graph.n;
    let two_m = graph.two_m();
    let mut refined: Vec<usize> = (0..n).collect();
    let mut refined_tot: Vec<f64> = graph.degree.clone();
    let order = shuffled_order(n, rng);

    for &u in &order {
        let parent = partition[u];
        let cu = refined[u];
        let ku = graph.degree[u];
        // weight from u into refined communities, restricted to same parent
        let mut to_refined: HashMap<usize, f64> = HashMap::new();
        for &(v, w) in &graph.adj[u] {
            if partition[v] != parent {
                continue;
            }
            *to_refined.entry(refined[v]).or_insert(0.0) += w;
        }
        refined_tot[cu] -= ku;
        let mut best_community = cu;
        // Staying a singleton has gain 0 against itself; require a strictly
        // positive, connectivity-preserving merge to move.
        let mut best_gain = 0.0f64;
        for (&c, &k_u_c) in &to_refined {
            if c == cu {
                continue;
            }
            let gain = k_u_c - gamma * ku * refined_tot[c] / two_m;
            if gain > best_gain + 1e-12 {
                best_gain = gain;
                best_community = c;
            }
        }
        refined_tot[best_community] += ku;
        refined[u] = best_community;
    }
    refined
}

/// Build the aggregate graph from the refined partition, carrying the coarse
/// partition up as the initial community of each super-node.
fn aggregate(
    graph: &WeightedGraph,
    refined: &[usize],
    partition: &[usize],
) -> (WeightedGraph, Vec<usize>, Vec<usize>) {
    // Compact refined ids to 0..k = super-node ids.
    let mut super_of_refined: HashMap<usize, usize> = HashMap::new();
    let mut super_count = 0usize;
    for &r in refined {
        super_of_refined.entry(r).or_insert_with(|| {
            let v = super_count;
            super_count += 1;
            v
        });
    }
    let super_of: Vec<usize> = (0..graph.n)
        .map(|u| super_of_refined[&refined[u]])
        .collect();
    let k = super_count;

    let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
    let mut self_loop = vec![0.0f64; k];
    // intra-refined-community weight becomes the super-node self-loop
    for u in 0..graph.n {
        let su = super_of[u];
        self_loop[su] += graph.self_loop[u];
        for &(v, w) in &graph.adj[u] {
            let sv = super_of[v];
            if su == sv {
                self_loop[su] += w / 2.0; // each intra edge seen twice
            } else if su < sv {
                *weights.entry((su, sv)).or_insert(0.0) += w / 2.0;
            } else {
                *weights.entry((sv, su)).or_insert(0.0) += w / 2.0;
            }
        }
    }
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); k];
    for (&(a, b), &w) in &weights {
        adj[a].push((b, w));
        adj[b].push((a, w));
    }
    let mut degree = vec![0.0f64; k];
    let mut total_weight = 0.0;
    for u in 0..k {
        let incident: f64 = adj[u].iter().map(|(_, w)| *w).sum();
        degree[u] = incident + 2.0 * self_loop[u];
        total_weight += incident / 2.0 + self_loop[u];
    }

    // Super-node's initial community = coarse community of any of its members.
    let mut super_partition = vec![usize::MAX; k];
    for u in 0..graph.n {
        super_partition[super_of[u]] = partition[u];
    }

    (
        WeightedGraph {
            n: k,
            adj,
            self_loop,
            degree,
            total_weight,
        },
        super_of,
        super_partition,
    )
}

fn community_totals(graph: &WeightedGraph, community: &[usize]) -> Vec<f64> {
    let max_c = community.iter().copied().max().unwrap_or(0);
    let mut totals = vec![0.0f64; max_c + 1];
    for u in 0..graph.n {
        totals[community[u]] += graph.degree[u];
    }
    totals
}

fn shuffled_order(n: usize, rng: &mut u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (splitmix64(rng) as usize) % (i + 1);
        order.swap(i, j);
    }
    order
}

/// Split any community whose induced subgraph (over live edges) is disconnected
/// into its connected pieces. Returns a community label per node (by index into
/// `nodes`).
fn enforce_connected_communities(
    edges: &[EdgeRecord],
    nodes: &[String],
    community: &[usize],
) -> Vec<usize> {
    let index_of = index_nodes(nodes);
    let n = nodes.len();
    // Undirected adjacency restricted to same-community edges.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let (Some(&u), Some(&v)) = (
            index_of.get(edge.from_id.as_str()),
            index_of.get(edge.to_id.as_str()),
        ) else {
            continue;
        };
        if u != v && community[u] == community[v] {
            adj[u].push(v);
            adj[v].push(u);
        }
    }
    let mut label = vec![usize::MAX; n];
    let mut next = 0usize;
    for start in 0..n {
        if label[start] != usize::MAX {
            continue;
        }
        let mut queue = VecDeque::new();
        queue.push_back(start);
        label[start] = next;
        while let Some(v) = queue.pop_front() {
            for &w in &adj[v] {
                if label[w] == usize::MAX {
                    label[w] = next;
                    queue.push_back(w);
                }
            }
        }
        next += 1;
    }
    label
}

/// Newman-Girvan modularity at resolution `gamma` for a node->community map.
pub fn partition_modularity(
    edges: &[EdgeRecord],
    community: &HashMap<String, u64>,
    gamma: f64,
) -> f64 {
    let mut degree: HashMap<&str, f64> = HashMap::new();
    let mut intra: HashMap<u64, f64> = HashMap::new();
    let mut tot: HashMap<u64, f64> = HashMap::new();
    let mut two_m = 0.0f64;
    for edge in edges {
        if edge.tombstone {
            continue;
        }
        let w = edge.effective_confidence().max(1e-9);
        *degree.entry(edge.from_id.as_str()).or_insert(0.0) += w;
        *degree.entry(edge.to_id.as_str()).or_insert(0.0) += w;
        two_m += 2.0 * w;
        let cu = community.get(&edge.from_id);
        let cv = community.get(&edge.to_id);
        if let (Some(&cu), Some(&cv)) = (cu, cv) {
            if cu == cv {
                *intra.entry(cu).or_insert(0.0) += 2.0 * w;
            }
        }
    }
    if two_m <= 0.0 {
        return 0.0;
    }
    for (node, &deg) in &degree {
        if let Some(&c) = community.get(*node) {
            *tot.entry(c).or_insert(0.0) += deg;
        }
    }
    let mut q = 0.0;
    let communities: BTreeSet<u64> = community.values().copied().collect();
    for c in communities {
        let e_c = *intra.get(&c).unwrap_or(&0.0);
        let a_c = *tot.get(&c).unwrap_or(&0.0);
        q += e_c / two_m - gamma * (a_c / two_m).powi(2);
    }
    q
}

// ===== Node similarity and link prediction =====

/// Similarity metric over neighbor sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimilarityMetric {
    Jaccard,
    Overlap,
}

impl SimilarityMetric {
    pub fn parse(raw: &str) -> Option<SimilarityMetric> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "jaccard" => Some(SimilarityMetric::Jaccard),
            "overlap" => Some(SimilarityMetric::Overlap),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimilarityPair {
    pub node1: String,
    pub node2: String,
    pub similarity: f64,
}

/// Undirected neighbor sets (excluding self) over live edges.
pub fn neighbor_sets(edges: &[EdgeRecord]) -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges {
        if edge.tombstone || edge.from_id == edge.to_id {
            continue;
        }
        map.entry(edge.from_id.clone())
            .or_default()
            .insert(edge.to_id.clone());
        map.entry(edge.to_id.clone())
            .or_default()
            .insert(edge.from_id.clone());
    }
    map
}

fn set_similarity(a: &BTreeSet<String>, b: &BTreeSet<String>, metric: SimilarityMetric) -> f64 {
    let intersection = a.intersection(b).count() as f64;
    if intersection == 0.0 {
        return 0.0;
    }
    match metric {
        SimilarityMetric::Jaccard => {
            let union = a.len() + b.len() - intersection as usize;
            if union == 0 {
                0.0
            } else {
                intersection / union as f64
            }
        }
        SimilarityMetric::Overlap => {
            let min_degree = a.len().min(b.len());
            if min_degree == 0 {
                0.0
            } else {
                intersection / min_degree as f64
            }
        }
    }
}

/// Pairwise node similarity over neighbor sets. Restricts to nodes whose degree
/// is at least `degree_cutoff`, keeps pairs scoring at least `similarity_cutoff`,
/// and returns at most `top_k` neighbors per source node (0 = unlimited).
/// `restrict` optionally limits sources/targets to a designated node set.
pub fn node_similarity(
    edges: &[EdgeRecord],
    metric: SimilarityMetric,
    degree_cutoff: usize,
    similarity_cutoff: f64,
    top_k: usize,
    restrict: Option<&BTreeSet<String>>,
) -> Vec<SimilarityPair> {
    let neighbors = neighbor_sets(edges);
    let eligible = |node: &str| -> bool {
        let degree_ok = neighbors.get(node).map(BTreeSet::len).unwrap_or(0) >= degree_cutoff.max(1);
        let scope_ok = restrict.map(|set| set.contains(node)).unwrap_or(true);
        degree_ok && scope_ok
    };

    let mut pairs: Vec<SimilarityPair> = Vec::new();
    for (a, na) in &neighbors {
        if !eligible(a) {
            continue;
        }
        // Candidate targets: nodes sharing at least one neighbor with `a`.
        let mut candidates: BTreeSet<&String> = BTreeSet::new();
        for shared in na {
            if let Some(others) = neighbors.get(shared) {
                for other in others {
                    if other != a {
                        candidates.insert(other);
                    }
                }
            }
        }
        let mut scored: Vec<SimilarityPair> = Vec::new();
        for b in candidates {
            if !eligible(b) {
                continue;
            }
            let nb = &neighbors[b];
            let similarity = set_similarity(na, nb, metric);
            if similarity >= similarity_cutoff && similarity > 0.0 {
                scored.push(SimilarityPair {
                    node1: a.clone(),
                    node2: b.clone(),
                    similarity,
                });
            }
        }
        scored.sort_by(|x, y| {
            y.similarity
                .partial_cmp(&x.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| x.node2.cmp(&y.node2))
        });
        if top_k > 0 {
            scored.truncate(top_k);
        }
        pairs.extend(scored);
    }
    pairs
}

/// Number of common neighbors of `a` and `b`.
pub fn common_neighbors(neighbors: &BTreeMap<String, BTreeSet<String>>, a: &str, b: &str) -> f64 {
    match (neighbors.get(a), neighbors.get(b)) {
        (Some(na), Some(nb)) => na.intersection(nb).count() as f64,
        _ => 0.0,
    }
}

/// Adamic/Adar index: sum over common neighbors `w` of `1 / ln(deg(w))`.
pub fn adamic_adar(neighbors: &BTreeMap<String, BTreeSet<String>>, a: &str, b: &str) -> f64 {
    let (Some(na), Some(nb)) = (neighbors.get(a), neighbors.get(b)) else {
        return 0.0;
    };
    let mut score = 0.0;
    for w in na.intersection(nb) {
        let degree = neighbors.get(w).map(BTreeSet::len).unwrap_or(0);
        if degree > 1 {
            score += 1.0 / (degree as f64).ln();
        }
    }
    score
}

/// Resource allocation index: sum over common neighbors `w` of `1 / deg(w)`.
pub fn resource_allocation(
    neighbors: &BTreeMap<String, BTreeSet<String>>,
    a: &str,
    b: &str,
) -> f64 {
    let (Some(na), Some(nb)) = (neighbors.get(a), neighbors.get(b)) else {
        return 0.0;
    };
    let mut score = 0.0;
    for w in na.intersection(nb) {
        let degree = neighbors.get(w).map(BTreeSet::len).unwrap_or(0);
        if degree > 0 {
            score += 1.0 / degree as f64;
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edge(from: &str, to: &str) -> EdgeRecord {
        EdgeRecord::new(format!("{from}->{to}"), from, "REL", to, json!({}))
    }

    fn weighted_edge(from: &str, to: &str, confidence: f64) -> EdgeRecord {
        let mut e = edge(from, to);
        e.confidence = Some(confidence);
        e
    }

    // ----- SCC / condensation / toposort -----

    #[test]
    fn scc_finds_planted_cycle_as_one_component() {
        // a -> b -> c -> a (cycle), c -> d, d is its own SCC.
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "a"),
            edge("c", "d"),
        ];
        let sccs = strongly_connected_components(&edges);
        // largest first: {a,b,c} then {d}
        assert_eq!(sccs[0], vec!["a", "b", "c"]);
        assert_eq!(sccs[1], vec!["d"]);
    }

    #[test]
    fn toposort_errors_on_cycle_and_succeeds_on_condensation() {
        let cyclic = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "a"),
            edge("c", "d"),
        ];
        assert!(topological_sort(&cyclic).is_err());

        let order = topological_sort_condensation(&cyclic);
        // {a,b,c} must come before {d}
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], vec!["a", "b", "c"]);
        assert_eq!(order[1], vec!["d"]);

        let acyclic = vec![edge("a", "b"), edge("b", "c")];
        assert_eq!(topological_sort(&acyclic).unwrap(), vec!["a", "b", "c"]);
    }

    // ----- Betweenness -----

    #[test]
    fn betweenness_matches_brute_force_on_path() {
        // Path a - b - c - d - e (undirected). The middle node c lies on the
        // most shortest paths.
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "d"),
            edge("d", "e"),
        ];
        let bc = betweenness_centrality(&edges, false);
        // Exact undirected betweenness of a 5-path: ends 0, b=3, c=4, d=3.
        assert!((bc["a"] - 0.0).abs() < 1e-9);
        assert!((bc["e"] - 0.0).abs() < 1e-9);
        assert!((bc["b"] - 3.0).abs() < 1e-9);
        assert!((bc["c"] - 4.0).abs() < 1e-9);
        assert!((bc["d"] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn sampled_betweenness_within_tolerance_of_exact() {
        // Star-of-paths so betweenness is non-trivial.
        let mut edges = Vec::new();
        for i in 0..8 {
            edges.push(edge("hub", &format!("leaf{i}")));
            edges.push(edge(&format!("leaf{i}"), &format!("tip{i}")));
        }
        let exact = betweenness_centrality(&edges, false);
        // With sample_size >= node count the pivot set is the full node set, so
        // the Brandes/Pich estimator (scale = n/k = 1) reproduces the exact
        // result: stated tolerance is 0 at full sampling.
        let sampled = betweenness_centrality_sampled(&edges, false, 1_000, 7);
        for (node, value) in &exact {
            assert!((sampled[node] - value).abs() < 1e-6, "node {node}");
        }
        // A partial sample still ranks the hub as the top broker.
        let partial = betweenness_centrality_sampled(&edges, false, 6, 7);
        let partial_top = partial
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(partial_top, "hub");
        // hub is the top broker.
        let max_node = exact
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(max_node, "hub");
    }

    // ----- Articulation points and bridges -----

    #[test]
    fn articulation_and_bridges_match_known_cut_structure() {
        // Two triangles joined by a single edge c-d. c and d are cut vertices,
        // and c-d is the only bridge.
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "a"),
            edge("c", "d"),
            edge("d", "e"),
            edge("e", "f"),
            edge("f", "d"),
        ];
        let (points, bridges) = articulation_points_and_bridges(&edges);
        assert!(points.contains("c"));
        assert!(points.contains("d"));
        assert!(!points.contains("a"));
        assert_eq!(bridges, vec![("c".to_string(), "d".to_string())]);
    }

    #[test]
    fn parallel_edge_is_not_a_bridge() {
        // a=b with two parallel edges, plus a pendant b-c. a-b is not a bridge.
        let edges = vec![
            EdgeRecord::new("e1", "a", "REL", "b", json!({})),
            EdgeRecord::new("e2", "a", "REL", "b", json!({})),
            edge("b", "c"),
        ];
        let (_points, bridges) = articulation_points_and_bridges(&edges);
        assert_eq!(bridges, vec![("b".to_string(), "c".to_string())]);
    }

    // ----- Leiden -----

    #[test]
    fn leiden_recovers_planted_communities() {
        // Two dense triangles linked by one weak bridge.
        let edges = vec![
            weighted_edge("a", "b", 1.0),
            weighted_edge("b", "c", 1.0),
            weighted_edge("a", "c", 1.0),
            weighted_edge("x", "y", 1.0),
            weighted_edge("y", "z", 1.0),
            weighted_edge("x", "z", 1.0),
            weighted_edge("c", "x", 0.05),
        ];
        let result = leiden(&edges, 1.0, 42, 10);
        assert_eq!(result.community["a"], result.community["b"]);
        assert_eq!(result.community["b"], result.community["c"]);
        assert_eq!(result.community["x"], result.community["y"]);
        assert_eq!(result.community["y"], result.community["z"]);
        assert_ne!(result.community["a"], result.community["x"]);
        assert!(result.modularity > 0.0);
    }

    #[test]
    fn leiden_communities_are_connected() {
        // A graph where a naive partition could place disconnected nodes
        // together; the guarantee must hold for every community.
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "a"),
            edge("d", "e"),
            edge("e", "f"),
            edge("f", "d"),
            edge("c", "d"),
            edge("g", "h"),
        ];
        let result = leiden(&edges, 1.0, 1, 10);
        assert_community_connectivity(&edges, &result.community);
    }

    fn assert_community_connectivity(edges: &[EdgeRecord], community: &HashMap<String, u64>) {
        use std::collections::HashSet;
        // group nodes by community
        let mut groups: HashMap<u64, Vec<String>> = HashMap::new();
        for (node, &c) in community {
            groups.entry(c).or_default().push(node.clone());
        }
        // adjacency over live edges
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in edges {
            if e.tombstone {
                continue;
            }
            adj.entry(e.from_id.as_str())
                .or_default()
                .push(e.to_id.as_str());
            adj.entry(e.to_id.as_str())
                .or_default()
                .push(e.from_id.as_str());
        }
        for (c, members) in &groups {
            if members.len() <= 1 {
                continue;
            }
            let member_set: HashSet<&str> = members.iter().map(String::as_str).collect();
            let mut seen: HashSet<&str> = HashSet::new();
            let start = members[0].as_str();
            let mut stack = vec![start];
            seen.insert(start);
            while let Some(v) = stack.pop() {
                if let Some(neighbors) = adj.get(v) {
                    for &w in neighbors {
                        if member_set.contains(w) && seen.insert(w) {
                            stack.push(w);
                        }
                    }
                }
            }
            assert_eq!(
                seen.len(),
                members.len(),
                "community {c} is disconnected: {members:?}"
            );
        }
    }

    // ----- Node similarity / link prediction -----

    #[test]
    fn node_similarity_matches_hand_computed_jaccard() {
        // a and b both connect to {x, y}; a also to z. N(a)={x,y,z}, N(b)={x,y}.
        // Jaccard(a,b) = |{x,y}| / |{x,y,z}| = 2/3.
        let edges = vec![
            edge("a", "x"),
            edge("a", "y"),
            edge("a", "z"),
            edge("b", "x"),
            edge("b", "y"),
        ];
        let pairs = node_similarity(&edges, SimilarityMetric::Jaccard, 1, 0.0, 0, None);
        let ab = pairs
            .iter()
            .find(|p| p.node1 == "a" && p.node2 == "b")
            .expect("a-b pair present");
        assert!((ab.similarity - 2.0 / 3.0).abs() < 1e-9);

        let overlap = node_similarity(&edges, SimilarityMetric::Overlap, 1, 0.0, 0, None);
        let ab_overlap = overlap
            .iter()
            .find(|p| p.node1 == "a" && p.node2 == "b")
            .unwrap();
        // Overlap = 2 / min(3,2) = 1.0
        assert!((ab_overlap.similarity - 1.0).abs() < 1e-9);
    }

    #[test]
    fn link_prediction_functions_return_exact_values() {
        // common neighbors of a and b are {x, y}.
        // deg(x): x connects to a, b, w  => 3 ; deg(y): y connects to a, b => 2.
        let edges = vec![
            edge("a", "x"),
            edge("a", "y"),
            edge("b", "x"),
            edge("b", "y"),
            edge("x", "w"),
        ];
        let neighbors = neighbor_sets(&edges);
        assert!((common_neighbors(&neighbors, "a", "b") - 2.0).abs() < 1e-9);
        // resource allocation = 1/deg(x) + 1/deg(y) = 1/3 + 1/2 = 5/6
        assert!((resource_allocation(&neighbors, "a", "b") - 5.0 / 6.0).abs() < 1e-9);
        // adamic-adar = 1/ln(3) + 1/ln(2)
        let expected = 1.0 / 3.0_f64.ln() + 1.0 / 2.0_f64.ln();
        assert!((adamic_adar(&neighbors, "a", "b") - expected).abs() < 1e-9);
    }
}
