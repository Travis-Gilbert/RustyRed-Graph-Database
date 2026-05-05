//! PyO3 bindings over THG-Core.

use pyo3::prelude::*;
use thg_core::{expand_bounded, paths_shortest, InMemoryThgExecutor};

type EdgeTuple = (String, String, String);

#[pyfunction]
pub fn thg_expand_bounded(
    edges: Vec<EdgeTuple>,
    seeds: Vec<String>,
    max_depth: usize,
) -> Vec<(String, usize)> {
    expand_bounded(edges, seeds, max_depth)
}

#[pyfunction]
pub fn thg_paths_shortest(
    edges: Vec<EdgeTuple>,
    source: String,
    target: String,
    max_depth: usize,
) -> Vec<String> {
    paths_shortest(edges, source, target, max_depth)
}

#[pyclass]
pub struct ThgCoreExecutor {
    executor: InMemoryThgExecutor,
}

#[pymethods]
impl ThgCoreExecutor {
    #[new]
    fn new() -> Self {
        Self {
            executor: InMemoryThgExecutor::new(),
        }
    }

    fn execute_json(&mut self, request_json: &str) -> String {
        self.executor.execute_json(request_json)
    }

    fn state_hash(&self) -> String {
        self.executor.state_hash()
    }
}
