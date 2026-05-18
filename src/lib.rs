//! rusty_red_native: PyO3 entry point.
//!
//! Exports a native `push_ppr` implementation with this Python signature:
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
//! because application node IDs are arbitrary integers.

mod bgi;
mod cmh;
mod graph_export;
mod push_ppr;
mod search_kernel;
mod thg;

use pyo3::prelude::*;

#[pymodule]
fn rusty_red_native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(push_ppr::push_ppr, m)?)?;
    m.add_function(wrap_pyfunction!(cmh::cmh_body_hash, m)?)?;
    m.add_function(wrap_pyfunction!(cmh::cmh_atom_id_v1, m)?)?;
    m.add_function(wrap_pyfunction!(cmh::cmh_handoff_state_hash_v1, m)?)?;
    m.add_function(wrap_pyfunction!(bgi::bgi_stable_hash_json, m)?)?;
    m.add_function(wrap_pyfunction!(bgi::bgi_fact_pack_hash_rows_json, m)?)?;
    m.add_function(wrap_pyfunction!(bgi::bgi_egraph_receipt_summary_json, m)?)?;
    m.add_function(wrap_pyfunction!(
        bgi::bgi_egraph_extract_context_pack_json,
        m
    )?)?;
    m.add_function(wrap_pyfunction!(bgi::bgi_datalog_receipt_summary_json, m)?)?;
    m.add_function(wrap_pyfunction!(bgi::bgi_datalog_derive_core_json, m)?)?;
    m.add_function(wrap_pyfunction!(bgi::bgi_compact_receipts_json, m)?)?;
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
