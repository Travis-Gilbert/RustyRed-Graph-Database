# Burn Morphological Graph — North Star

A direction, not an execution handoff. It records what a Burn-plus-CubeCL morphological-graph library is, why it exists, what it deliberately is and is not, and how it stays honest against its Python oracle. It spawns execution handoffs later against the geometry and algorithms plugins; it is not one itself. Read it before starting any Rust port of city2graph so the port does not drift into a monolith, a one-to-one clone, or a premature replacement.

## The thing

A Rust library that turns urban geospatial data (building footprints, street centerlines, parcels) into a heterogeneous typed graph, and runs message passing over it, RAM-resident over RustyRed, CPU-native via Burn with a GPU path via CubeCL. It is the parity target for the Python city2graph adoption (see the Civic Atlas backend's `PYTHON-MORPHOLOGICAL-GRAPH-NORTHSTAR.md`). The Python runs first and is the oracle; this is what replaces it at parity over time.

## Why it exists

Three reasons, in order of durability.

It fills a real ecosystem gap. There is no established GNN or message-passing library on Burn. Burn already exposes the tensor primitives message passing needs (gather, scatter, select across its backends; scatter-add over edge-indexed neighbor features is the core operation). Building this is filling a gap in a fast-rising Rust ML framework, not reimplementing something that exists. That is what makes it a portfolio-grade contribution, the same way an AlphaFold3 Pairformer block in Burn was novel because nobody had done one.

It is the convergence point five threads already bend toward. One library sits at the confluence of work already in flight: it consumes the geometry plugin (tessellation, contiguity, the S2 cover); it produces the typed graph the Burn building-head Pairformer trains on; its message passing is the primitive the algorithms plugin's GNN-inference operation needs; its dual-graph output is the routing graph the flat traffic display layer lacks; and it runs RAM-resident over RustyRed, the operational story that distinguishes the whole stack from the Python lane. This is not scope creep. It is the shape the architecture has been reaching for.

It removes a heavy Python dependency from the hot path, eventually. city2graph pulls the full scientific-GIS stack (momepy, libpysal, osmnx, geopandas, shapely). The Rust version, at parity, lets the morphological graph run in-process over RustyRed without that stack. Eventually, not now; the Python is correct and shipped, and this earns its place only at parity.

## What it is and is not

It is the geometry spine of city2graph, reimplemented in Rust, with a Burn message-passing layer on top. The hard part is the geometry, not the GNN. The message passing is comparatively small once the tensor primitives are in hand; the real work is the spatial algorithms:

- Enclosed morphological tessellation (city2graph wraps momepy): partition space into cells around buildings, bounded by streets, so adjacency is principled cell contiguity rather than a distance threshold. This is the defensible `adjacent_to` and `same_block_as`.
- Spatial weights and contiguity (city2graph wraps libpysal): queen and rook contiguity over the tessellation.
- The two-cap reachability field: snap a center onto the street network, run one single-source Dijkstra, reuse that one cost field to judge streets, buildings, and cells against a single metric, with network distance and perpendicular access distance held as two independent caps so a building across a barrier from a reachable street is not falsely included. This is a better isochrone primitive than a buffer.
- Dual-graph street topology: street segments become nodes, shared endpoints become edges. This is the routing graph the traffic display layer lacks.
- The heterogeneous-graph to tensor bridge: the typed graph (`touched_to`, `connected_to`, `faced_to`) lowered to the tensor form Burn message passing consumes.

It is built as reference plugins for the geometry and algorithms plugins, not a separate monolith. The tessellation, contiguity, and reachability are geometry-plugin capabilities; the message passing is an algorithms-plugin operation. It slots into the plugin API the rest of RustyRed already uses.

It is not a one-to-one port. city2graph carries academic surface this does not need: mobility flow graphs, GTFS transit modeling, metapath construction. Port the spine listed above; decline the rest. A future thread can add a piece if a consumer ever needs it, under the same no-consumer-no-feature discipline the algorithms plugin already holds.

It is not a replacement until parity is proven. The Python runs in Theseus and feeds the Pairformer today. This does not get wired in front of that until its edge sets match the Python's on the same inputs.

## Honesty against the oracle

The Python city2graph is the correctness check. Same footprints and centerlines in, compare the typed edge sets out. The port is at parity when, for a held set of Flint blocks, the Rust `touched_to` / `connected_to` / `faced_to` edges match the Python's within a stated tolerance. Parity is a measured claim against the oracle, not a judgment call. Until then, the Rust output is advisory and the Python remains the source feeding the model.

## Where execution goes when it starts

Execution handoffs spawned from this North Star target the existing plugins:

- Geometry plugin: tessellation adjacency, contiguity, the two-cap reachability field, the dual-graph street topology. These are spatial-algorithm deliverables against the geometry plugin's encoder and S2 cover.
- Algorithms plugin: the Burn message-passing operation, building on the tensor primitives and the typed graph, and on the GNN-inference operation the algorithms plugin already contemplates.

Each handoff is written in the execution register with named acceptance, re-verifies repo state at build time, and names its consumer. None of that is this document's job; this document is the why and the boundary.

## Grounding

- Burn exposes gather, scatter, and select across its backends (verified in tracel-ai/burn: burn-tch, burn-candle), the primitives message passing is built from. No established GNN library on Burn exists (verified by search), which is the gap this fills.
- city2graph (BSD-3, Liverpool GDS Lab) produces the typed `("place","touched_to","place")`, `("movement","connected_to","movement")`, `("place","faced_to","movement")` graph via enclosed tessellation, dual-graph street topology, and a single-source reachability field with separate network and access caps (verified by reading its morphology module). Its hard deps are the full scientific-GIS stack.
- The five threads this converges: the geometry plugin and algorithms plugin specs in this repo (docs/plans/), the Burn building-head Pairformer decision, the routing-graph gap in the Civic Atlas traffic layer, and RustyRed's RAM-resident substrate.
- Harness memory: the city2graph decision (Python now plus Burn later, sequential not either-or) and its five-thread convergence are encoded; the civic Pairformer is an R-GCN multi-task regressor whose typed relations this graph supplies.

## Status

Direction only. No execution has started. The Python adoption (the oracle) is the active path; this is the target it is measured against and eventually replaced by. This North Star is the durable record so that a later session does not, without this context, wire Civic Atlas to depend on Theseus permanently for civic geometry, start the port without using the Python as the oracle, or port the academic surface that was meant to be declined.
