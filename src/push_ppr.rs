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
//! The Python reference is canonical: all numerical decisions match it
//! within float-rounding tolerance. Any divergence is a bug in this file.

use std::collections::{HashMap, HashSet, VecDeque};

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

/// Internal: extract a `dict[int, list[tuple[int, float]]]` into a
/// HashMap<i64, Vec<(i64, f64)>>. Returns TypeError on any malformed entry.
fn extract_adjacency(adjacency: &Bound<'_, PyDict>) -> PyResult<HashMap<i64, Vec<(i64, f64)>>> {
    let mut adj: HashMap<i64, Vec<(i64, f64)>> = HashMap::with_capacity(adjacency.len());
    for (key_obj, val_obj) in adjacency.iter() {
        let u: i64 = key_obj
            .extract()
            .map_err(|_| PyTypeError::new_err("adjacency keys must be int"))?;
        let nbr_list: Bound<'_, PyList> = val_obj
            .downcast_into::<PyList>()
            .map_err(|_| PyTypeError::new_err("adjacency values must be list[tuple[int, float]]"))?;
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
        adj.insert(u, nbrs);
    }
    Ok(adj)
}

/// Internal: extract `dict[int, float]` -> HashMap<i64, f64>.
fn extract_seeds(seeds: &Bound<'_, PyDict>) -> PyResult<HashMap<i64, f64>> {
    let mut out: HashMap<i64, f64> = HashMap::with_capacity(seeds.len());
    for (key_obj, val_obj) in seeds.iter() {
        let u: i64 = key_obj
            .extract()
            .map_err(|_| PyTypeError::new_err("seeds keys must be int"))?;
        let mass: f64 = val_obj
            .extract()
            .map_err(|_| PyTypeError::new_err("seeds values must be float"))?;
        out.insert(u, mass);
    }
    Ok(out)
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
    let adj = extract_adjacency(adjacency)?;
    let seeds_map = extract_seeds(seeds)?;

    let out_dict = PyDict::new_bound(py);
    if seeds_map.is_empty() {
        return Ok(out_dict);
    }

    // Per-node out-weight: sum of edge weights. Used to scale the
    // epsilon threshold so degree-1 hubs don't dominate the queue.
    let mut out_weight: HashMap<i64, f64> = HashMap::with_capacity(adj.len());
    for (u, nbrs) in adj.iter() {
        if !nbrs.is_empty() {
            let total: f64 = nbrs.iter().map(|(_, w)| *w).sum();
            out_weight.insert(*u, total);
        }
    }

    // Threshold: epsilon * max(out_weight.get(u, 0.0), 1.0). The 1.0 floor
    // protects nodes with no out-edges (or seeds with no entry in adj)
    // from a zero threshold that would never converge.
    let threshold = |u: i64, out_weight: &HashMap<i64, f64>| -> f64 {
        let ow = out_weight.get(&u).copied().unwrap_or(0.0);
        epsilon * ow.max(1.0)
    };

    // PPR estimate p[u] and residual r[u].
    let mut p: HashMap<i64, f64> = HashMap::new();
    let mut r: HashMap<i64, f64> = HashMap::with_capacity(seeds_map.len() * 4);
    for (u, mass) in seeds_map.iter() {
        r.insert(*u, *mass);
    }

    // FIFO queue. Python uses collections.deque; Rust VecDeque is the
    // direct analogue. Iteration order over seeds_map differs between
    // Python dict (insertion order) and HashMap (arbitrary), so the
    // converged result is order-independent up to the per-node 1e-5
    // tolerance the parity tests enforce.
    let mut queue: VecDeque<i64> = VecDeque::with_capacity(seeds_map.len());
    let mut in_queue: HashSet<i64> = HashSet::with_capacity(seeds_map.len());
    for u in seeds_map.keys() {
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

        // Spread (1 - alpha) fraction proportionally to edge weights.
        let nbrs = match adj.get(&u) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let node_out = match out_weight.get(&u) {
            Some(w) if *w > 0.0 => *w,
            _ => continue,
        };
        let spread_total = (1.0 - alpha) * residual;
        for (v, w) in nbrs.iter() {
            let add = spread_total * (*w / node_out);
            let new_rv = *r.get(v).unwrap_or(&0.0) + add;
            r.insert(*v, new_rv);
            if !in_queue.contains(v) && new_rv > threshold(*v, &out_weight) {
                queue.push_back(*v);
                in_queue.insert(*v);
            }
        }
    }

    // Marshal HashMap<i64, f64> -> Python dict.
    for (k, v) in p.iter() {
        out_dict.set_item(*k, *v)?;
    }
    Ok(out_dict)
}
