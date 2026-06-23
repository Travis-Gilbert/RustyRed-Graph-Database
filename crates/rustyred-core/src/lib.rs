//! Rusty Red core command runtime.
//!
//! This crate has no Python or network-server dependencies. The standalone
//! helper crate, MCP adapter, and HTTP server call this same executor.

// `result_large_err`: GraphStoreError is intentionally rich (it carries context
// used for HTTP/MCP error mapping); boxing it to shrink Result is a post-0.9.1
// cleanup. `too_many_arguments`: several graph entrypoints (e.g. hybrid_search)
// take many parameters by design.
#![allow(clippy::result_large_err, clippy::too_many_arguments)]

pub mod algorithm_ops;
pub mod algorithms;
pub mod commands;
pub mod crdt;
pub mod errors;
pub mod executor;
pub mod fulltext;
#[cfg(feature = "tantivy")]
pub mod fulltext_tantivy;
#[cfg(feature = "geometry")]
pub mod geometry;
pub mod graph;
pub mod graph_store;
pub mod instant_kg;
pub mod morphology;
pub mod operation;
pub mod plugin;
pub mod spatial;
#[cfg(feature = "s2")]
pub mod spatial_s2;
pub mod state;
pub mod store;
pub mod versioned_graph;

pub use algorithm_ops::{algorithm_operations, AlgorithmsPlugin};
pub use algorithms::{
    adamic_adar, articulation_points_and_bridges, betweenness_centrality,
    betweenness_centrality_sampled, common_neighbors, condense, leiden, neighbor_sets,
    node_similarity, partition_modularity, resource_allocation, strongly_connected_components,
    topological_sort, topological_sort_condensation, Condensation, CycleError, LeidenResult,
    SimilarityMetric, SimilarityPair,
};
pub use commands::{RustyredCommand, RustyredRequest, RustyredResponse};
pub use crdt::{
    diff_since, diff_snapshot_since, join_delta, merge_edge_record, merge_node_record,
    try_diff_since, try_join_delta, ActorId, Hlc, HlcClock, JoinReport, StampedBatch,
    StampedMutation, VersionVector,
};
pub use errors::{RustyredError, RustyredResult};
pub use executor::{execute_request_json, InMemoryRustyredExecutor, RustyredExecutor};
pub use fulltext::{
    make_fulltext_backend, make_fulltext_backend_from_value, FullTextBackend, FullTextBackendError,
    FullTextDesignation, FullTextIndex, RUSTY_RED_FULLTEXT_BACKEND_ENV,
};
#[cfg(feature = "geometry")]
pub use geometry::{
    GeometryDesignation, GeometryEncoder, GeometryEncoding, GeometryError, GeometryIndex,
    GeometryPlugin, PointEncoder, WkbEncoder, WktEncoder,
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
pub use morphology::{
    default_relation_weights, dual_graph_edges, is_morphological_relation,
    message_pass as morphological_message_pass, morphological_edges_from_records, morphology_stats,
    relation_weights_from_map, MorphologicalEdge, MorphologicalNodeKind, MorphologyError,
    MorphologyStats, StreetSegmentTopology, CONNECTED_TO, FACED_TO, TOUCHED_TO,
};
pub use operation::{
    dispatch_operation, AlgorithmGraph, AlgorithmOperation, GraphCounts, MemoryEstimate,
    OperationError, OperationMode,
};
pub use plugin::{
    builtin_plugin_registry, FullTextBackendRegistration, PluginCapability, PluginCapabilityKind,
    PluginOperationContext, PluginOperationRegistration, PluginRegistry, RustyRedPlugin,
    SpatialBackendRegistration,
};
pub use spatial::{
    make_spatial_backend, make_spatial_backend_from_value, SpatialBackend, SpatialDesignation,
    SpatialError, SpatialIndex, RUSTY_RED_SPATIAL_BACKEND_ENV,
};
pub use state::{stable_hash, RustyredEdge, RustyredNode, RustyredState};
pub use versioned_graph::{
    build_prolly_tree, checkout_graph_version, compile_graph_pack, diff_graph_snapshots,
    graph_version_log, merge_graph_snapshots, resolve_auto_confidence_edge,
    snapshot_content_objects, update_graph_ref, CompiledGraphPack, GraphCheckoutResult,
    GraphCommit, GraphCompileOptions, GraphCompilerCapability, GraphContentObject, GraphDiffEntry,
    GraphMergeConflict, GraphMergeOptions, GraphMergeResolution, GraphMergeResult, GraphMergeSide,
    GraphMergeStrategy, GraphObjectKind, GraphPackManifest, GraphProllyTree, GraphRefUpdate,
    GraphTreeChild, GraphTreeEntry, GraphTreeNode, GraphVersionDiff, GraphVersionLog,
    GraphVersionRef, GraphVersionRepository, DEFAULT_GRAPH_BRANCH, GRAPH_PACK_COMPILER_VERSION,
    VERSIONED_GRAPH_PROTOCOL_VERSION,
};
