# RustyRed GraphDB

RustyRed is a remarkably fast Graph + Vector database.
It runs entirely in RAM.
Designed to help humans and their agents work well together.

Featuring GraphCache graph state-aware cache, a first-class MCP agent port, built-in-RAG both graph and vector
multi-tenancy, HNSW vector search, confidence-weighted epistemic edges, and document storage.
Written in Rust, the best way to write a database. In my humble opinion.

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/new/template/RUSTY_RED_GRAPH_DATABASE_TEMPLATE_ID?utm_medium=integration&utm_source=button&utm_campaign=rusty-red-graph-database)

Note put template ID here before making public  `RUSTY_RED_GRAPH_DATABASE_TEMPLATE_ID` in the badge URL with the Railway template code.

## What Rusty Red does

- **Graph storage** with AOF/snapshot persistence, per-tenant isolation, single-writer serializable commits, and committed read snapshots
- **Stable, versioned on-disk format** with `thg-upgrade-format` migrations between releases (no export/re-import on upgrade)
- **HNSW vector search** on node properties via `instant-distance`, with hybrid scoring that blends vector similarity and graph proximity
- **Inverted-index BM25 full-text search** with automatic indexing on node upserts
- **H3 spatial index** on node lat/lon properties with radius and bounding-box queries
- **Epistemic edge types** (Supports, Contradicts, Tension, Derives, Cites) with confidence-weighted traversal across configurable hop depth
- **Graph algorithms over HTTP/MCP**: PPR, connected components, PageRank, and label-propagation community detection
- **MCP agent port** with scoped auth tokens, read-only and read-write modes, tool annotations, and structured tool/resource/prompt surfaces
- **Graph-version-aware cache** (10 kinds) that detects stale entries when the underlying graph mutates
- **Bounded Cypher surface**: single-hop and outgoing multi-hop MATCH, bounded variable-length expand, path aliases, property projections, `COUNT(*) / COUNT(binding)`, and transaction-scoped `CREATE`/`MERGE`/`SET`/`DELETE`
- **JSONL bulk loader** for nodes and edges
- **Observability**: Prometheus `/metrics` (17 counters), slow-query ring buffer at `/v1/diagnostics/slow_queries`
- **HTTP transaction API**: `/v1/transactions/begin|commit|rollback` with snapshot isolation
- **Native algorithm helpers** exposed through the root PyO3 compatibility crate, including ACL local-push Personalized PageRank

## What you can't do yet

These are on the roadmap, in roughly this priority order:

1. Incoming and undirected Cypher relationship patterns, plus the rest of full OpenCypher/GQL coverage
2. `OPTIONAL MATCH`, `WITH`, `UNION`, `CALL`, `ORDER BY`, `SKIP`, `DISTINCT`
3. `SUM` / `AVG` / `MIN` / `MAX` aggregations
4. `REMOVE` clauses
5. CSV/JSONL `LOAD CSV` syntactic form (JSONL bulk endpoints exist already)
6. Distributed snapshot replication
7. Per-query spatial backend selection; H3 ships by default and S2 is available behind the `s2` feature plus `RUSTY_RED_SPATIAL_BACKEND=s2`

## Crate structure

| Crate | Purpose |
|-------|---------|
| `thg-core` | Graph store engine, command executor, HNSW vector index, epistemic edges |
| `thg-mcp` | MCP agent port: tool dispatch, resource reads, prompt surface |
| `thg-product-server` | HTTP server, query surface, graph cache, auth, OpenAPI |
| `thg-server` | Standalone compatibility command server |
| `thg-resp-server` | RESP protocol shim (limited, not a Redis replacement) |
| root crate | PyO3 compatibility bindings for native graph/search helpers |

## Build (local development)

Requires Rust 1.85+ and `maturin >= 1.7`.

```bash
python3 -m pip install --user maturin
cargo check --workspace
maturin develop --release
```

`cargo check --workspace` validates the Rust workspace. `maturin develop --release` builds the root `abi3-py312` compatibility wheel into the active Python environment.

## Product server

The product server runs in `RUSTY_RED_MODE=embedded` with RedCore
RAM-first storage and local AOF/snapshot persistence. It exposes graph
operations, vector search, epistemic traversal, Cypher queries, and the
graph-version cache over HTTP and MCP.

`RUSTY_RED_MODE=redis` is available only for legacy compatibility deployments.
The normal Rusty Red service does not require Redis, FalkorDB, Memgraph, or any
second Rusty Red service.

Run the product server locally:

```bash
RUSTY_RED_MODE=embedded RUSTY_RED_DATA_DIR=data/rusty-red cargo run -p thg-product-server
```

Strict local durability mode is explicit:

```bash
RUSTY_RED_MODE=embedded \
RUSTY_RED_CONCURRENCY=single_writer \
RUSTY_RED_TXN_ISOLATION=serializable \
RUSTY_RED_STRICT_ACID=true \
RUSTY_RED_DURABILITY=aof_always \
RUSTY_RED_DATA_DIR=data/rusty-red \
cargo run -p thg-product-server
```

Core routes are documented by `GET /openapi.json`. The OpenAPI document is
generated from the product server crate version and currently covers every
canonical route in `crates/thg-product-server/src/router.rs`.

Canonical routes:

```text
GET  /health
GET  /ready
GET  /openapi.json
GET  /.well-known/mcp/thg.json
GET  /.well-known/agent.json
POST /mcp
GET  /metrics
POST /v1/command
POST /v1/batch
POST /v1/query
POST /v1/cypher
POST /v1/cypher/explain
POST /v1/transactions/begin
POST /v1/transactions/commit
POST /v1/transactions/rollback
POST /v1/cache/put
POST /v1/cache/get
POST /v1/cache/check
POST /v1/cache/explain
POST /v1/cache/invalidate
POST /v1/cache/stats
POST /v1/tenants/{tenant_id}/command
POST /v1/tenants/{tenant_id}/batch
GET  /v1/tenants/{tenant_id}/runs/{run_id}
POST /v1/tenants/{tenant_id}/graph/query
POST /v1/tenants/{tenant_id}/graph/nodes
POST /v1/tenants/{tenant_id}/graph/nodes/query
GET  /v1/tenants/{tenant_id}/graph/nodes/{node_id}
POST /v1/tenants/{tenant_id}/graph/edges
GET  /v1/tenants/{tenant_id}/graph/edges/{edge_id}
POST /v1/tenants/{tenant_id}/graph/neighbors
GET  /v1/tenants/{tenant_id}/graph/stats
GET  /v1/tenants/{tenant_id}/graph/verify
POST /v1/tenants/{tenant_id}/graph/rebuild-indexes
POST /v1/tenants/{tenant_id}/graph/vector/search
POST /v1/tenants/{tenant_id}/graph/vector/hybrid
POST /v1/tenants/{tenant_id}/graph/vector/designate
POST /v1/tenants/{tenant_id}/graph/epistemic-neighbors
POST /v1/tenants/{tenant_id}/graph/algorithms/ppr
POST /v1/tenants/{tenant_id}/graph/algorithms/components
POST /v1/tenants/{tenant_id}/graph/algorithms/pagerank
POST /v1/tenants/{tenant_id}/graph/algorithms/communities
POST /v1/tenants/{tenant_id}/graph/spatial/designate
POST /v1/tenants/{tenant_id}/graph/spatial/radius
POST /v1/tenants/{tenant_id}/graph/spatial/bbox
POST /v1/tenants/{tenant_id}/graph/fulltext/designate
POST /v1/tenants/{tenant_id}/graph/fulltext/search
POST /v1/tenants/{tenant_id}/graph/bulk/nodes
POST /v1/tenants/{tenant_id}/graph/bulk/edges
GET  /v1/diagnostics/slow_queries
GET  /v1/diagnostics/config
POST /v1/tenants/{tenant_id}/context/pack
```

`GET /health/` and `GET /ready/` are trailing-slash aliases for deployment
probes.

### MCP tools

The `/mcp` endpoint exposes these tools (via JSON-RPC `tools/list` and `tools/call`):

| Tool | Description |
|------|-------------|
| `thg.graph.query` / `thg.graph.explain` / `thg.graph.neighbors` | Bounded native graph reads and plan inspection |
| `thg.graph.schema` / `thg.graph.index_status` | Graph schema and index-health reads |
| `thg.algorithm.ppr` (alias: `thg.algo.ppr`) / `thg.algorithm.components` (`thg.algo.components`) / `thg.algorithm.pagerank` (`thg.algo.pagerank`) / `thg.algorithm.communities` (`thg.algo.communities`) | Graph algorithms: PPR, connected components, PageRank, label-propagation communities |
| `thg.fulltext.search` (alias: `thg.graph.fulltext.search`) / `thg.spatial.radius` (`thg.graph.spatial.radius`) / `thg.spatial.bbox` (`thg.graph.spatial.bbox`) | Full-text and spatial read surfaces |
| `thg.vector.search` | HNSW nearest-neighbor search on vector properties |
| `thg.vector.hybrid` | Hybrid search blending vector similarity with graph proximity |
| `thg.vector.designate` | Register a vector property for HNSW indexing (write) |
| `thg.epistemic.neighbors` | Confidence-weighted epistemic traversal by edge type |
| `thg.fulltext.designate` (alias: `thg.graph.fulltext.designate`) / `thg.spatial.designate` (`thg.graph.spatial.designate`) / `thg.bulk.nodes` (`thg.graph.bulk.nodes`) / `thg.bulk.edges` (`thg.graph.bulk.edges`) | Write-mode-only designation and bulk ingest tools |
| `thg.admin.verify` | Admin-only index-integrity verification; rebuild remains on the HTTP graph route |

The public query surface is now split cleanly:

- `/v1/query` is the product-facing native subset for `node_match` and `neighbors`.
- `/v1/cypher` and `/v1/cypher/explain` are the bounded OpenCypher-compatible surface for read queries plus transaction-scoped `CREATE`/`MERGE`/`SET`/`DELETE` writes.
- `/v1/tenants/{tenant_id}/graph/query` remains the older debug bridge and should not be treated as the product route.

`GET /v1/diagnostics/config` returns the static runtime config snapshot, including
startup-only tenant override details. Runtime mutation of tenant config is not
supported in this slice.

## Railway template

Railway can deploy this repository directly as one web service:

- Build with the included `Dockerfile`.
- Use `/ready` as the health check.
- Attach one persistent volume mounted at `/app/data/rusty-red`.
- Keep `RUSTY_RED_MODE=embedded`.
- Keep `RUSTY_RED_REQUIRE_VOLUME=true` so `/ready` fails if the volume is missing.
- For public ingress, set `RUSTY_RED_REQUIRE_AUTH=true` and provide scoped `RUSTY_RED_API_TOKENS`.
- Replace the badge placeholder once Railway assigns the final public template URL.

The template should not require Redis or a second service. Redis variables are
still accepted for explicit legacy deployments, but they are not part of the
standalone Rusty Red template path.

## Downstream integrations

This repository is the upstream source for versioned Rusty Red releases.
Product integrations can consume it as a downstream `git subtree` and keep
their own deployment adapters or private overlays downstream. The included
`.github/workflows/sync-downstream.yml` can open downstream sync PRs after
pushes to `main` when `DOWNSTREAM_SYNC_REPOSITORY` and `DOWNSTREAM_SYNC_TOKEN`
are configured in repository settings.

See `docs/downstream-sync.md` for the setup contract and the local subtree sync
command.

## Docker defaults

The bundled image runs `rusty-red-graph-server` with these important defaults:

```text
RUSTY_RED_HOST=[::]
RUSTY_RED_MODE=embedded
RUSTY_RED_DATA_DIR=/app/data/rusty-red
RUSTY_RED_REQUIRE_VOLUME=true
RUSTY_RED_DURABILITY=aof_everysec
RUSTY_RED_SNAPSHOT_INTERVAL_WRITES=1000
RUSTY_RED_REQUIRE_AUTH=false
RUSTY_RED_MCP_ENABLED=true
RUSTY_RED_MCP_READ_ONLY=true
RUSTY_RED_MCP_ALLOW_ADMIN=false
```

## Compatibility command server

The product server is the recommended deployment target. A smaller compatibility
HTTP command server also lives in `crates/thg-server`:

```bash
cargo run -p thg-server -- --host 127.0.0.1 --port 7379
```

Endpoints:

```text
GET  /health
GET  /ready
GET  /v1/state/hash
GET  /v1/runs/{id}
POST /v1/command
POST /v1/batch
```

## Python compatibility bindings

The root crate exposes native helper functions through PyO3 for Python 3.12+.
These bindings are not required for the Railway template. The most important
algorithm helper is:

```python
def push_ppr(
    adjacency: dict[int, list[tuple[int, float]]],
    seeds: dict[int, float],
    *,
    alpha: float = 0.15,
    epsilon: float = 1e-4,
    max_pushes: int = 200_000,
) -> dict[int, float]: ...
```

## Algorithm reference

Andersen, R., Chung, F., and Lang, K. (2006). Local Graph Partitioning using PageRank Vectors. FOCS 2006.

## Benchmarks

Single-threaded, single-seed PPR queries on random sparse graphs (average
degree 4, alpha 0.15, epsilon 1e-4), captured on an M1 Max via
`tests/test_benchmarks.py`:

| Nodes | Native | Python | Speedup |
|-------|--------|--------|---------|
| 50K   | 0.0024s | 0.0318s | 13.2x |
| 200K  | 0.0034s | 0.1753s | 51.3x |
| 1M    | 0.0023s | 0.9573s | 413.9x (acceptance gate: must be >= 20x) |


The fixture is generated with seed 42 for reproducibility. Numbers vary across hardware; the 20x floor is enforced on whatever runner executes the test.

The native impl uses lazy on-demand neighbor extraction: ACL Push typically touches ~1/(epsilon*alpha) ~ 67k nodes for production params, so converting only those (not the full adjacency dict) eliminates the dominant FFI cost.

## License

MIT.
