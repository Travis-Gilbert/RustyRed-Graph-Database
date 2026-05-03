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

use pyo3::prelude::*;

#[pymodule]
fn theseus_native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(push_ppr::push_ppr, m)?)?;
    Ok(())
}
