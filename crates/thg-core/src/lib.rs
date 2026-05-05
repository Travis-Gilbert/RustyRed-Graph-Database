//! THG-Core: Theorem HotGraph command runtime.
//!
//! This crate has no Django, Python, or network-server dependencies. Both
//! PyO3 in-process bindings and the standalone HTTP server call this same
//! executor.

pub mod commands;
pub mod errors;
pub mod executor;
pub mod graph;
pub mod state;
pub mod store;

pub use commands::{ThgCommand, ThgRequest, ThgResponse};
pub use errors::{ThgError, ThgResult};
pub use executor::{execute_request_json, InMemoryThgExecutor, ThgExecutor};
pub use graph::{expand_bounded, paths_shortest, EdgeTuple};
pub use state::{stable_hash, ThgEdge, ThgNode, ThgState};
