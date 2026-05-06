//! theseus_native: PyO3 entry point.
//!
//! Exports a single function, `push_ppr`, matching the live Python
//! signature in `apps/notebook/sparse_ppr.py:push_ppr` exactly:
//!
//!     push_ppr(
//!         adjacency: dict[int, list[tuple[int, float]]],
//!         seeds: dict[int, float],
//!         *,
//!         alpha: float = 0.15,
//!         epsilon: float = 1e-4,
//!         max_pushes: int = 200_000,
//!     ) -> dict[int, float]
//!
//! `alpha`, `epsilon`, `max_pushes` are keyword-only (PyO3 `*` separator).
//! `adjacency` keys and node IDs are Python `int` (not contiguous indices)
//! because Theseus PKs are arbitrary integers.

mod push_ppr;
mod graph_export;
mod search_kernel;
mod thg;

use pyo3::prelude::*;

#[pymodule]
fn theseus_native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(push_ppr::push_ppr, m)?)?;
    m.add_function(wrap_pyfunction!(
        search_kernel::search_normalize_urls_batch,
        m
    )?)?;
    m.add_function(wrap_pyfunction!(
        search_kernel::search_score_frontier_batch,
        m
    )?)?;
    m.add_function(wrap_pyfunction!(
        search_kernel::search_fuse_scores_batch,
        m
    )?)?;
    m.add_function(wrap_pyfunction!(search_kernel::search_cosine_topk, m)?)?;
    m.add_function(wrap_pyfunction!(graph_export::graph_remap_ids_batch, m)?)?;
    m.add_function(wrap_pyfunction!(graph_export::graph_pack_edges_batch, m)?)?;
    m.add_function(wrap_pyfunction!(thg::thg_expand_bounded, m)?)?;
    m.add_function(wrap_pyfunction!(thg::thg_paths_shortest, m)?)?;
    m.add_class::<thg::ThgCoreExecutor>()?;
    Ok(())
}
