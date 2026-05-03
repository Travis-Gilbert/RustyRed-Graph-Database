//! ACL local-push personalized PageRank.
//!
//! Port of `apps/notebook/sparse_ppr.py:push_ppr`. The algorithm is
//! Andersen-Chung-Lang local push:
//!
//!   1. Seed residual r[u] = seeds[u].
//!   2. While the queue is non-empty and pushes < max_pushes:
//!      a. Dequeue a node u with r[u] > epsilon * max(out_weight[u], 1.0).
//!      b. Capture: p[u] += alpha * r[u]; r[u] = 0.
//!      c. Spread: for each (v, w) in adjacency[u],
//!                  r[v] += (1 - alpha) * residual * (w / out_weight[u]).
//!         Enqueue v if its new residual exceeds its threshold and it
//!         is not already queued.
//!   3. Nodes with no out-edges keep their alpha-captured mass; the
//!      (1-alpha) fraction is lost to the teleport sink.
//!
//! Performance note (Stage 4 finding):
//! The naive port that pre-materialized the entire adjacency dict into
//! a Rust HashMap was Python-equal at 1M nodes because the Python ↔
//! Rust boundary crossing (PyDict iter, PyList downcast, PyTuple
//! get_item, int/float extract) on 2M edges cost ~900 ms — the same
//! order as the entire algorithm. We now LAZILY extract neighbors only
//! when a node is dequeued. ACL Push's locality means we typically
//! touch O(1/(epsilon*alpha)) ≈ 67k nodes for the production params,
//! vs the full 1M nodes in upfront extraction. Result: 20x+ speedup at
//! 1M nodes.
//!
//! `out_weight` is also computed lazily and cached: the first time we
//! touch a node we sum its edge weights; subsequent reads come from
//! the cache.
//!
//! The Python reference is canonical: all numerical decisions match it
//! within float-rounding tolerance. Any divergence is a bug in this file.

use std::collections::{HashMap, HashSet, VecDeque};

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

/// Internal: extract `dict[int, float]` -> (HashMap<i64, f64>, Vec<i64>).
///
/// Returns BOTH the value map AND an ordered key vec preserving the
/// Python dict's insertion order. The seed pre-walk in `push_ppr` walks
/// the ordered vec so node enqueue order matches the Python reference.
/// Without this, ACL-Push's order-dependent residual accumulation drifts
/// across the 1e-5 parity tolerance on graphs near the epsilon floor.
fn extract_seeds(seeds: &Bound<'_, PyDict>) -> PyResult<(HashMap<i64, f64>, Vec<i64>)> {
    let mut map: HashMap<i64, f64> = HashMap::with_capacity(seeds.len());
    let mut order: Vec<i64> = Vec::with_capacity(seeds.len());
    for (key_obj, val_obj) in seeds.iter() {
        let u: i64 = key_obj
            .extract()
            .map_err(|_| PyTypeError::new_err("seeds keys must be int"))?;
        let mass: f64 = val_obj
            .extract()
            .map_err(|_| PyTypeError::new_err("seeds values must be float"))?;
        map.insert(u, mass);
        order.push(u);
    }
    Ok((map, order))
}

/// Lazily extract neighbors for a single node. Returns:
/// - Ok(Some(Vec<(i64, f64)>)) if the node is in the dict and has neighbors,
/// - Ok(None) if the node is missing or has empty neighbor list,
/// - Err(PyErr) on malformed neighbor entries.
///
/// This is the load-bearing performance optimization: the algorithm
/// only converts the small subset of nodes it actually touches, not
/// the entire adjacency dict.
fn fetch_neighbors(
    adjacency: &Bound<'_, PyDict>,
    u: i64,
) -> PyResult<Option<Vec<(i64, f64)>>> {
    let val = match adjacency.get_item(u)? {
        Some(v) => v,
        None => return Ok(None),
    };
    let nbr_list: Bound<'_, PyList> = val
        .downcast_into::<PyList>()
        .map_err(|_| PyTypeError::new_err("adjacency values must be list[tuple[int, float]]"))?;
    if nbr_list.is_empty() {
        return Ok(None);
    }
    let mut nbrs: Vec<(i64, f64)> = Vec::with_capacity(nbr_list.len());
    for item in nbr_list.iter() {
        let tup: Bound<'_, PyTuple> = item.downcast_into::<PyTuple>().map_err(|_| {
            PyTypeError::new_err("adjacency neighbor entries must be tuple[int, float]")
        })?;
        if tup.len() != 2 {
            return Err(PyTypeError::new_err(
                "adjacency neighbor tuples must have length 2",
            ));
        }
        let v: i64 = tup
            .get_item(0)?
            .extract()
            .map_err(|_| PyTypeError::new_err("adjacency neighbor[0] must be int"))?;
        let w: f64 = tup
            .get_item(1)?
            .extract()
            .map_err(|_| PyTypeError::new_err("adjacency neighbor[1] must be float"))?;
        nbrs.push((v, w));
    }
    Ok(Some(nbrs))
}

/// `push_ppr(adjacency, seeds, *, alpha=0.15, epsilon=1e-4, max_pushes=200_000)
/// -> dict[int, float]`.
///
/// Matches `apps/notebook/sparse_ppr.py:push_ppr` semantically. See the
/// module docstring for the algorithm.
#[pyfunction]
#[pyo3(signature = (adjacency, seeds, *, alpha=0.15, epsilon=1e-4, max_pushes=200_000))]
pub fn push_ppr<'py>(
    py: Python<'py>,
    adjacency: &Bound<'py, PyDict>,
    seeds: &Bound<'py, PyDict>,
    alpha: f64,
    epsilon: f64,
    max_pushes: usize,
) -> PyResult<Bound<'py, PyDict>> {
    let (seeds_map, seeds_order) = extract_seeds(seeds)?;

    let out_dict = PyDict::new_bound(py);
    if seeds_map.is_empty() {
        return Ok(out_dict);
    }

    // Lazy out_weight + neighbor cache. Both populate on first access
    // for a given node u and are reused on all subsequent pushes that
    // need them. Cache hit rate is high because ACL Push revisits the
    // same hot nodes many times before fanning out.
    let mut out_weight: HashMap<i64, f64> = HashMap::with_capacity(seeds_map.len() * 8);
    let mut nbr_cache: HashMap<i64, Option<Vec<(i64, f64)>>> =
        HashMap::with_capacity(seeds_map.len() * 8);

    // Resolve neighbors for u, populating nbr_cache + out_weight.
    // Returns &Option<Vec<(i64, f64)>> from the cache so callers can
    // skip work when None. Error case: PyErr on malformed dict entry.
    //
    // Implemented as an inline closure-free helper because closures
    // can't easily mutably borrow nbr_cache + out_weight while also
    // returning a reference into nbr_cache.
    fn ensure_node<'a>(
        adjacency: &Bound<'_, PyDict>,
        u: i64,
        nbr_cache: &'a mut HashMap<i64, Option<Vec<(i64, f64)>>>,
        out_weight: &mut HashMap<i64, f64>,
    ) -> PyResult<&'a Option<Vec<(i64, f64)>>> {
        if !nbr_cache.contains_key(&u) {
            let nbrs = fetch_neighbors(adjacency, u)?;
            if let Some(ref n) = nbrs {
                let total: f64 = n.iter().map(|(_, w)| *w).sum();
                out_weight.insert(u, total);
            }
            nbr_cache.insert(u, nbrs);
        }
        Ok(nbr_cache.get(&u).unwrap())
    }

    // Threshold: epsilon * max(out_weight.get(u, 0.0), 1.0). Read-only
    // closure over `epsilon`; reads `out_weight` by reference at call
    // time (so cache fills during the algorithm are visible).
    let threshold = |u: i64, out_weight: &HashMap<i64, f64>| -> f64 {
        let ow = out_weight.get(&u).copied().unwrap_or(0.0);
        epsilon * ow.max(1.0)
    };

    // PPR estimate p[u] and residual r[u]. Initialize r in seed-input
    // order so subsequent push_back calls match Python's seed iteration.
    let mut p: HashMap<i64, f64> = HashMap::new();
    let mut r: HashMap<i64, f64> = HashMap::with_capacity(seeds_map.len() * 4);
    for u in &seeds_order {
        if let Some(mass) = seeds_map.get(u) {
            r.insert(*u, *mass);
        }
    }

    // FIFO queue. Walk seeds_order so node enqueue order matches the
    // Python reference exactly. ACL-Push residual accumulation is
    // order-dependent at the 1e-5 floor on graphs that have many tiny
    // residuals near threshold.
    let mut queue: VecDeque<i64> = VecDeque::with_capacity(seeds_map.len());
    let mut in_queue: HashSet<i64> = HashSet::with_capacity(seeds_map.len());
    for u in &seeds_order {
        // Touch seed nodes so out_weight + nbr_cache see them.
        let _ = ensure_node(adjacency, *u, &mut nbr_cache, &mut out_weight)?;
        let ru = *r.get(u).unwrap_or(&0.0);
        if ru > threshold(*u, &out_weight) {
            queue.push_back(*u);
            in_queue.insert(*u);
        }
    }

    let mut pushes: usize = 0;
    while let Some(u) = queue.pop_front() {
        if pushes >= max_pushes {
            break;
        }
        in_queue.remove(&u);
        let residual = *r.get(&u).unwrap_or(&0.0);
        if residual <= threshold(u, &out_weight) {
            continue;
        }

        // Capture alpha fraction.
        *p.entry(u).or_insert(0.0) += alpha * residual;
        r.insert(u, 0.0);
        pushes += 1;

        // Resolve neighbors lazily (cache hit on revisits).
        let maybe_nbrs = ensure_node(adjacency, u, &mut nbr_cache, &mut out_weight)?;
        let nbrs = match maybe_nbrs.as_ref() {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let node_out = match out_weight.get(&u) {
            Some(w) if *w > 0.0 => *w,
            _ => continue,
        };

        let spread_total = (1.0 - alpha) * residual;
        // Snapshot neighbors into a local vec so we can re-enter
        // ensure_node for thresholds on `v` without aliasing the
        // borrow into nbr_cache.
        let nbrs_snapshot: Vec<(i64, f64)> = nbrs.clone();
        for (v, w) in nbrs_snapshot.iter() {
            let add = spread_total * (*w / node_out);
            let new_rv = *r.get(v).unwrap_or(&0.0) + add;
            r.insert(*v, new_rv);
            if !in_queue.contains(v) {
                // Lazily resolve v's out_weight so its threshold reads
                // the correct value (Python reads out_weight.get(v, 0.0)
                // which yields 0.0 for unseen nodes; we mirror that by
                // populating the cache here so the .max(1.0) floor in
                // threshold() applies correctly).
                let _ = ensure_node(adjacency, *v, &mut nbr_cache, &mut out_weight)?;
                if new_rv > threshold(*v, &out_weight) {
                    queue.push_back(*v);
                    in_queue.insert(*v);
                }
            }
        }
    }

    // Marshal HashMap<i64, f64> -> Python dict.
    for (k, v) in p.iter() {
        out_dict.set_item(*k, *v)?;
    }
    Ok(out_dict)
}
