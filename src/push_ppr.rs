//! ACL local-push personalized PageRank.
//!
//! STUB. The real implementation lands in Task 1.4. This stub exists so
//! that Task 1.3 can verify the wheel builds, the module imports, and
//! the signature matches before any algorithm code is written.

use pyo3::prelude::*;
use pyo3::types::PyDict;

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
    // Suppress unused warnings on the stub; real impl uses these in Task 1.4.
    let _ = (adjacency, seeds, alpha, epsilon, max_pushes);
    Ok(PyDict::new_bound(py))
}
