# RustyRed Algorithms Plugin: Tier 1 — Execution Handoff

Extends the algorithm surface beyond PPR with the tier-1 set mined from Neo4j GDS 2.13, registered through the plugin API, adopting the GDS operation contract (execution modes plus estimation). Ships in this public MIT repo. The standing conventions apply (CONVENTIONS.md): one session, no phase two; named choices are requirements; done means verified end to end and observable.

## Invariants

- Everything here registers through the plugin API (operations plus lifecycle hooks), the same seam the geometry plugin uses. This doc's contract names come from RUSTYRED-PLUGIN-API.md; the executing agent conforms them to the implementation as actually landed (see Verify first). If something cannot be expressed through the API, the gap is fixed in the API, not special-cased here.
- Operation contract on every algorithm: a mode (stream, stats, mutate) and an estimate before run. Mutate writes results as node properties (community_id, betweenness, pagerank) so workflows compose: Leiden, then filter by community, then PPR within, then betweenness over the result. RAM-first makes estimate-before-run mandatory, not a nicety.
- Clean room. GDS open source is GPLv3; this repo is MIT. The GDS source is a scope and API map only. Implementations come from the published papers: Leiden (Traag, Waltman, van Eck 2019), betweenness (Brandes 2001), articulation points and bridges (DFS lowpoint), SCC (Tarjan 1972). No GDS code and no GDS-derived code enters this repo.
- Existing algorithms keep behavior and wire surface. pagerank, personalized_pagerank, label_propagation_communities, connected_components, paths_shortest, paths_shortest_weighted, expand_bounded, expand_bounded_weighted conform to the operation contract with no behavior change; existing rustyred_thg_algorithm_* tools and HTTP routes stay stable.
- The KNN materializer uses the existing HNSW vector index. No NN-Descent; GDS needs it only because Neo4j lacks a native ANN index. Materialized similarity edges are ordinary typed edges, visible to every other algorithm and to hybrid scoring.
- No consumer, no algorithm. Each deliverable names its consumer. GraphSAGE training and ML pipelines stay out; the ML lane remains export out, train outside, inference back in.

## Deliverables

### 1. Operation contract: mode plus estimate
Build: extend the plugin operation capability with `mode: stream | stats | mutate` and an `estimate()` returning memory bounds computed from node and edge counts. Prove it by conforming pagerank: callable through `execute_request_json` in all three modes; mutate writes a `pagerank` node property; estimate returns bounds; existing pagerank callers unchanged.
Acceptance: the three modes round-trip on a fixture graph; the mutate-written property is readable via node fetch; all existing pagerank tests pass unchanged.

### 2. KNN similarity-graph materializer
Build: operation `similarity_knn(label, vector_property, k, cutoff, edge_type = "SIMILAR_TO")` with modes stream and mutate. For each node carrying the designated vector: query HNSW for the top k above cutoff; mutate writes typed edges carrying a `score` property; re-running replaces that node's prior edges of that type (idempotent).
Consumers: turbovec code symbols (Qwen3-Embedding-4B, dim 2560) and the memory recall lane. The target query is PPR over the union of structural and SIMILAR_TO edges.
Acceptance: on a designated vector label, each node gains at most k edges above cutoff; neighbor queries return them; personalized_pagerank traverses them; a re-run leaves no stale edges.
Depends on 1.

### 3. Leiden
Build: clean-room from Traag et al. 2019 including the refinement phase, with a gamma resolution parameter. Modes stream, stats, mutate (`community_id`).
Consumers: code_map module detection, memory consolidation clustering, Theseus claim clusters.
Acceptance: recovers planted communities on a fixture graph; the Leiden guarantee holds and is asserted in a test (no returned community induces a disconnected subgraph); documented as the default community algorithm; label propagation and the deprecated louvain remain unchanged.
Depends on 1.

### 4. Betweenness, articulation points, bridges
Build: Brandes 2001 exact betweenness plus a sampled variant with a `sample_size` parameter; articulation points and bridges in one DFS. Modes stream, stats, mutate (`betweenness`).
Consumers: code impact (load-bearing symbols), Civic Atlas corridor identification, memory broker nodes, single points of failure.
Acceptance: exact betweenness matches brute force on a small fixture; the sampled variant lands within its stated tolerance on the same fixture; articulation points and bridges match the fixture's known cut vertices and cut edges.
Depends on 1.

### 5. SCC, condensation, topological sort
Build: Tarjan SCC; topological sort defined over the condensation and erroring on cyclic input otherwise.
Consumer: dependency cycles over the code graph's DEPENDS edges, and build order, once the code-graph plugin lands.
Acceptance: a fixture with a planted cycle yields that cycle as one SCC; toposort errors on the cyclic graph and succeeds on its condensation.
Depends on 1.

### 6. Node similarity and link-prediction features
Build: Jaccard and Overlap similarity over neighbor sets with a degree cutoff and top-k; pairwise functions `adamic_adar`, `common_neighbors`, `resource_allocation`.
Consumers: memory dedup before consolidation, near-clone detection in code, learned-scorer features, edge suggestion.
Acceptance: hand-computed fixture pairs match exactly; top-k similarity streams for a designated label; the three pairwise functions return exact values on the fixture.
Depends on 1.

## Surfaces
Operations register through the plugin API and surface as `rustyred_thg_algorithm_*` MCP tools and `/v1/tenants/:tenant_id/graph/algorithm/...` routes via the API's generation, beside the existing pagerank, ppr, communities, and components tools. No hand-wired adapter edits.

## What this does not do (separate handoffs)
Tier 2 (Steiner and prize-collecting Steiner, exposure-style contamination propagation, k-core, triangle count and clustering coefficient, HITS and ArticleRank, Yen's K-shortest paths, delta-stepping, A* with the geometry plugin's haversine heuristic, multi-source BFS, kmeans over vectors) and tier 3 (FastRP and HashGNN structural embeddings, SLLPA overlapping communities) are separate handoffs, each requiring a named consumer. A Pregel-style iterate-until-converged framework is deferred until a second algorithm author outside this plugin needs it.

## Grounding (verified)
- rustyred-core exports as of fe5350e (June 6): the algorithm free functions listed in the invariants; VectorIndex and VectorDesignation; HybridScoringConfig and edge_type_weights; HNSW-backed vector search; the tantivy full-text backend; versioned_graph.
- GDS 2.13 inventory read from the public neo4j/graph-data-science 2.13 branch: 40 algorithm families; the similarity module ships SimilarityGraphBuilder, the materialize-a-similarity-graph pattern deliverable 2 adopts. GDS license is GPLv3: map, not source.
- Live vector designation in the harness deployment: Page.semantic_vec, Qwen/Qwen3-Embedding-4B, dim 2560; code symbols slated for the same turbovec lane.
- Harness memory: doc_d449957d83639929 (GDS mining and ranking), doc_d4eaca42f004d8ce (code-graph plugin design, which consumes deliverables 2, 3, and 5), doc_4060b21f90baf4aa (plugin API and geometry plugin handoffs).

## Verify first
Locate the plugin API implementation. It is reported landed with the geometry plugin in the Civic Atlas RustyRed lane and is not on this repo's default branch as of fe5350e. Find the branch or repo, read the real plugin trait, registry, and operation registration, and conform this doc's names to it. If it cannot be located, RUSTYRED-PLUGIN-API.md is the prerequisite and is built first, not worked around. Also confirm where the algorithm free functions live relative to graph.rs and graph_store.rs before conforming them, and confirm the existing rustyred_thg_algorithm_* tool generation path before extending it.
