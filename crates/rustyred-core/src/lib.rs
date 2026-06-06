//! Rusty Red core command runtime.
//!
//! This crate has no Python or network-server dependencies. The standalone
//! helper crate, MCP adapter, and HTTP server call this same executor.

// `result_large_err`: GraphStoreError is intentionally rich (it carries context
// used for HTTP/MCP error mapping); boxing it to shrink Result is a post-0.9.1
// cleanup. `too_many_arguments`: several graph entrypoints (e.g. hybrid_search)
// take many parameters by design.
#![allow(clippy::result_large_err, clippy::too_many_arguments)]

pub mod commands;
pub mod errors;
pub mod executor;
pub mod fulltext;
#[cfg(feature = "tantivy")]
pub mod fulltext_tantivy;
pub mod graph;
pub mod graph_store;
pub mod instant_kg;
pub mod spatial;
#[cfg(feature = "s2")]
pub mod spatial_s2;
pub mod state;
pub mod store;
pub mod versioned_graph;

pub use commands::{RustyredCommand, RustyredRequest, RustyredResponse};
pub use errors::{RustyredError, RustyredResult};
pub use executor::{execute_request_json, InMemoryRustyredExecutor, RustyredExecutor};
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
pub use instant_kg::{
    instant_kg_payload_delta, instant_kg_payload_manifest, instant_kg_status_payload,
    CodeKgEncodedFile, CodeKgManifest, EdgeExplanation, HarnessInstantKg, ImpactResult,
    InstantKgStatus, PprResult, SearchResult, SessionDelta, INSTANT_KG_DEFAULT_ENCODER_VERSION,
    INSTANT_KG_DEFAULT_INGEST_VERSION, INSTANT_KG_PROTOCOL_VERSION,
};
pub use spatial::{
    make_spatial_backend, make_spatial_backend_from_value, SpatialBackend, SpatialDesignation,
    SpatialError, SpatialIndex, RUSTY_RED_SPATIAL_BACKEND_ENV,
};
pub use state::{stable_hash, RustyredEdge, RustyredNode, RustyredState};
pub use versioned_graph::{
    build_prolly_tree, checkout_graph_version, compile_graph_pack, diff_graph_snapshots,
    graph_version_log, merge_graph_snapshots, snapshot_content_objects, update_graph_ref,
    CompiledGraphPack, GraphCheckoutResult, GraphCommit, GraphCompileOptions,
    GraphCompilerCapability, GraphContentObject, GraphDiffEntry, GraphMergeConflict,
    GraphMergeOptions, GraphMergeResolution, GraphMergeResult, GraphMergeSide, GraphMergeStrategy,
    GraphObjectKind, GraphPackManifest, GraphProllyTree, GraphRefUpdate, GraphTreeChild,
    GraphTreeEntry, GraphTreeNode, GraphVersionDiff, GraphVersionLog, GraphVersionRef,
    GraphVersionRepository, DEFAULT_GRAPH_BRANCH, GRAPH_PACK_COMPILER_VERSION,
    VERSIONED_GRAPH_PROTOCOL_VERSION,
};
