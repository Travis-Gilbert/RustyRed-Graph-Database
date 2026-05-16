//! THG-Core: Theorem HotGraph command runtime.
//!
//! This crate has no Django, Python, or network-server dependencies. Both
//! PyO3 in-process bindings and the standalone HTTP server call this same
//! executor.

pub mod commands;
pub mod errors;
pub mod executor;
pub mod graph;
pub mod graph_store;
pub mod state;
pub mod store;

pub use commands::{ThgCommand, ThgRequest, ThgResponse};
pub use errors::{ThgError, ThgResult};
pub use executor::{execute_request_json, InMemoryThgExecutor, ThgExecutor};
pub use graph::{
    connected_components, expand_bounded, expand_bounded_weighted, louvain_communities, pagerank,
    paths_shortest, paths_shortest_weighted, personalized_pagerank, EdgeTuple,
};
pub use graph_store::{
    manifest_version_compatible, read_manifest, sanitize_tenant_segment, Direction, EdgeRecord,
    EpistemicType, GraphMutation, GraphMutationBatch, GraphRebuildReport, GraphSnapshot,
    GraphStats, GraphStore, GraphStoreError, GraphStoreResult, GraphTransaction, GraphWriteResult,
    InMemoryGraphStore, NeighborHit, NeighborQuery, NodeQuery, NodeRecord, Provenance,
    RedCoreDurability, RedCoreGraphStore, RedCoreManifest, RedCoreOptions, RedCoreStatus,
    VectorDesignation, VectorIndex, VectorPoint, VerifyProblem, VerifyReport,
    CURRENT_FORMAT_VERSION,
};
#[cfg(feature = "redis-store")]
pub use graph_store::{RedisGraphKeyspace, RedisGraphStore};
pub use state::{stable_hash, ThgEdge, ThgNode, ThgState};
