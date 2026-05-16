//! THG-Core: Theorem HotGraph command runtime.
//!
//! This crate has no Django, Python, or network-server dependencies. Both
//! PyO3 in-process bindings and the standalone HTTP server call this same
//! executor.

pub mod commands;
pub mod errors;
pub mod executor;
pub mod fulltext;
#[cfg(feature = "tantivy")]
pub mod fulltext_tantivy;
pub mod graph;
pub mod graph_store;
pub mod spatial;
#[cfg(feature = "s2")]
pub mod spatial_s2;
pub mod state;
pub mod store;

pub use commands::{ThgCommand, ThgRequest, ThgResponse};
pub use errors::{ThgError, ThgResult};
pub use executor::{execute_request_json, InMemoryThgExecutor, ThgExecutor};
pub use fulltext::{
    make_fulltext_backend, make_fulltext_backend_from_value, FullTextBackend, FullTextBackendError,
    FullTextDesignation, FullTextIndex, RUSTY_RED_FULLTEXT_BACKEND_ENV,
};
#[allow(deprecated)]
pub use graph::louvain_communities;
pub use graph::{
    connected_components, expand_bounded, expand_bounded_weighted, label_propagation_communities,
    pagerank, paths_shortest, paths_shortest_weighted, personalized_pagerank, EdgeTuple,
};
pub use graph_store::{
    default_hybrid_edge_type_weights, manifest_version_compatible, read_manifest,
    sanitize_tenant_segment, unix_ms, Direction, EdgeRecord, EpistemicType, GraphMutation,
    GraphMutationBatch, GraphRebuildReport, GraphSnapshot, GraphStats, GraphStore, GraphStoreError,
    GraphStoreResult, GraphTransaction, GraphWriteResult, HybridScoringConfig, InMemoryGraphStore,
    NeighborHit, NeighborQuery, NodeQuery, NodeRecord, Provenance, RedCoreDurability,
    RedCoreGraphStore, RedCoreManifest, RedCoreOptions, RedCoreStatus, VectorDesignation,
    VectorIndex, VectorPoint, VerifyProblem, VerifyReport, CURRENT_FORMAT_VERSION,
};
#[cfg(feature = "redis-store")]
pub use graph_store::{RedisGraphKeyspace, RedisGraphStore};
pub use spatial::{
    make_spatial_backend, make_spatial_backend_from_value, SpatialBackend, SpatialDesignation,
    SpatialError, SpatialIndex, RUSTY_RED_SPATIAL_BACKEND_ENV,
};
pub use state::{stable_hash, ThgEdge, ThgNode, ThgState};
