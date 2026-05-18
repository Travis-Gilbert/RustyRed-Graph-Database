//! PyO3 bindings over RustyRed-Core.

use pyo3::prelude::*;
use rustyred_core::{expand_bounded, paths_shortest, InMemoryRustyredExecutor};

type EdgeTuple = (String, String, String);

#[pyfunction]
pub fn rustyred_expand_bounded(
    edges: Vec<EdgeTuple>,
    seeds: Vec<String>,
    max_depth: usize,
) -> Vec<(String, usize)> {
    expand_bounded(edges, seeds, max_depth)
}

#[pyfunction]
pub fn rustyred_paths_shortest(
    edges: Vec<EdgeTuple>,
    source: String,
    target: String,
    max_depth: usize,
) -> Vec<String> {
    paths_shortest(edges, source, target, max_depth)
}

#[pyclass]
pub struct RustyredCoreExecutor {
    executor: InMemoryRustyredExecutor,
}

#[pymethods]
impl RustyredCoreExecutor {
    #[new]
    fn new() -> Self {
        Self {
            executor: InMemoryRustyredExecutor::new(),
        }
    }

    fn execute_json(&mut self, request_json: &str) -> String {
        self.executor.execute_json(request_json)
    }

    fn state_hash(&self) -> String {
        self.executor.state_hash()
    }
}
