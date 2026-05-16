# Rusty Red Graph Database

Embedded property graph database with a first-class MCP agent port,
multi-tenancy, HNSW vector search, confidence-weighted epistemic edges,
and a graph-version-aware AI cache. Written in Rust.

Designed for AI agents and GraphRAG workloads, not for replacing Neo4j.

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/new/template/RUSTY_RED_GRAPH_DATABASE_TEMPLATE_ID?utm_medium=integration&utm_source=button&utm_campaign=rusty-red-graph-database)

Template ID pending: after creating or publishing the Railway template, replace `RUSTY_RED_GRAPH_DATABASE_TEMPLATE_ID` in the badge URL with the Railway template code.

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
- **50x to 400x** faster Personalized PageRank than Python (ACL local-push algorithm, exposed via PyO3)

## What you can't do yet

These are on the roadmap, in roughly this priority order:

1. Incoming and undirected Cypher relationship patterns, plus the rest of full OpenCypher/GQL coverage
2. `OPTIONAL MATCH`, `WITH`, `UNION`, `CALL`, `ORDER BY`, `SKIP`, `DISTINCT`
3. `SUM` / `AVG` / `MIN` / `MAX` aggregations
4. `REMOVE` clauses
5. CSV/JSONL `LOAD CSV` syntactic form (JSONL bulk endpoints exist already)
6. Distributed snapshot replication
7. Spatial S2 cell index (H3 ships today)

## Crate structure

| Crate | Purpose |
|-------|---------|
| `thg-core` | Graph store engine, command executor, HNSW vector index, epistemic edges |
| `thg-mcp` | MCP agent port: tool dispatch, resource reads, prompt surface |
| `thg-product-server` | HTTP server, query surface, graph cache, auth, OpenAPI |
| `thg-server` | Standalone THG command server (non-product) |
| `thg-resp-server` | RESP protocol shim (limited, not a Redis replacement) |
| root crate | PyO3 bindings for `push_ppr` and `ThgCoreExecutor` |

The Rusty Red repository at
`https://github.com/Travis-Gilbert/rusty-red-graph-database` is maintained as a
`theseus_native` subtree export from the Theseus repository. The source of truth
for edits remains this directory unless the extraction strategy changes
deliberately.

## Build (local development)

Requires Rust 1.78+ and `maturin >= 1.7`.

```bash
python3 -m pip install --user maturin
cd theseus_native
maturin develop --release
```

This builds an `abi3-py312` wheel and installs it into the active Python environment. After this, `from theseus_native import push_ppr` works in any Python 3.12+ interpreter that shares the venv.

## Product server

The product server runs in `RUSTY_RED_MODE=embedded` with RedCore
RAM-first storage and local AOF/snapshot persistence. It exposes graph
operations, vector search, epistemic traversal, Cypher queries, and the
graph-version cache over HTTP and MCP.

`RUSTY_RED_MODE=redis` is available for legacy THG state commands only.

Run the product server locally:

```bash
cd theseus_native
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

Core routes:

```text
GET  /health
GET  /ready
GET  /openapi.json
GET  /.well-known/mcp/thg.json
POST /mcp
POST /v1/command
POST /v1/batch
POST /v1/query
POST /v1/cypher
POST /v1/cypher/explain
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
POST /v1/tenants/{tenant_id}/graph/epistemic/neighbors
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

### MCP tools

The `/mcp` endpoint exposes these tools (via JSON-RPC `tools/list` and `tools/call`):

| Tool | Description |
|------|-------------|
| `thg.graph.query` / `thg.graph.explain` / `thg.graph.neighbors` | Bounded native graph reads and plan inspection |
| `thg.graph.schema` / `thg.graph.index_status` | Graph schema and index-health reads |
| `thg.algorithm.ppr` / `thg.algorithm.components` / `thg.algorithm.pagerank` / `thg.algorithm.communities` | Graph algorithms, with communities reported honestly as label propagation |
| `thg.fulltext.search` / `thg.spatial.radius` / `thg.spatial.bbox` | Full-text and spatial read surfaces |
| `thg.vector.search` | HNSW nearest-neighbor search on vector properties |
| `thg.vector.hybrid` | Hybrid search blending vector similarity with graph proximity |
| `thg.vector.designate` | Register a vector property for HNSW indexing (write) |
| `thg.epistemic.neighbors` | Confidence-weighted epistemic traversal by edge type |
| `thg.fulltext.designate` / `thg.spatial.designate` / `thg.bulk.nodes` / `thg.bulk.edges` | Write-mode-only designation and bulk ingest tools |
| `thg.admin.verify` | Admin-only index-integrity verification; rebuild remains on the HTTP graph route |

The public query surface is now split cleanly:

- `/v1/query` is the product-facing native subset for `node_match` and `neighbors`.
- `/v1/cypher` and `/v1/cypher/explain` are the bounded OpenCypher-compatible surface for read queries plus transaction-scoped `CREATE`/`MERGE`/`SET`/`DELETE` writes.
- `/v1/tenants/{tenant_id}/graph/query` remains the older debug bridge and should not be treated as the product route.

`GET /v1/diagnostics/config` returns the static runtime config snapshot, including
startup-only tenant override details. Runtime mutation of tenant config is not
supported in this slice.

The OpenAPI document is served at `/openapi.json`. It exists because Rusty Red
is exposed through HTTP and MCP even though the underlying storage engine is a
database-style service. The OpenAPI contract is for the HTTP API; MCP tool,
resource, and prompt metadata are discovered through the MCP endpoint and
well-known manifests.

Railway template readiness follows the public template guidance: use a GitHub
source repo, keep the service root minimal, set `/ready` as the health check,
wire Redis only for explicit `RUSTY_RED_MODE=redis` deployments through private
networking/reference variables, attach persistent storage to stateful dependencies,
set `RUSTY_RED_REQUIRE_VOLUME=true` for embedded Railway deployments so `/ready`
fails when the mounted volume is absent, generate any public-ingress tokens with
Railway template variable functions, and replace the badge placeholder above once
Railway assigns the final template URL.

Railway can deploy this directory directly:

```bash
cd theseus_native
railway up
```

The included `Dockerfile`, `railway.toml`, and `.railwayignore` are for the
standalone Rusty Red subtree repository. The monorepo Railway template under
`railway-templates/rusty-red-graph-database/` remains useful when deploying
from the full Theseus repository.

## Build (release wheels)

CI builds Linux x86_64 manylinux2014 wheels via `.github/workflows/build_native_wheels.yml`. macOS arm64 is built locally for now (Travis's M1); CI build for Darwin is out of scope for the first cut.

Use `scripts/verify_thg_release.sh` from the repository root for the THG
runtime/product release check. The verifier intentionally uses package-targeted
Cargo release builds for the THG server binaries and uses `maturin` for the root
PyO3 extension:

```bash
scripts/verify_thg_release.sh
scripts/verify_thg_release.sh --develop  # install into the active Python env
```

Do not use `cargo build --manifest-path theseus_native/Cargo.toml --workspace
--release` as the native release check on macOS. That command attempts to link
the root PyO3 `cdylib` as a plain Cargo artifact and can fail with undefined
Python symbols even when the THG binaries and `maturin` wheel path are healthy.

## Public API

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

THG exports:

```python
from theseus_native import ThgCoreExecutor

executor = ThgCoreExecutor()
executor.execute_json('{"command":"THG.RUN.BEGIN","args":{"task":"demo"}}')
executor.state_hash()
```

Matches `apps/notebook/sparse_ppr.py:push_ppr` exactly. ACL local-push personalized PageRank: alpha is the restart probability (Theseus convention), epsilon is the per-node convergence threshold, max_pushes caps total iterations to prevent pathological walks.

## Fallback semantics

`apps/notebook/sparse_ppr.py` is the dispatcher. It tries `from theseus_native import push_ppr` first; on ImportError, or when `THESEUS_DISABLE_NATIVE=1` is set in the environment at call time, it routes to the pure-Python `_python_push_ppr` defined in the same file. The fallback exists indefinitely (per ADR 0001 follow-up) so dev environments without the wheel still function.

The wrapper logs once at WARNING level on the first import that finds the wheel missing: `theseus_native unavailable, using Python push_ppr`. Subsequent imports do not re-log.

## THG standalone HTTP server

Phase 1 standalone mode lives in `crates/thg-server`:

```bash
cd theseus_native
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

Django selects embedded or remote THG with:

```bash
THG_MODE=in_process
THG_MODE=remote_http THG_HTTP_URL=http://localhost:7379
```

## Algorithm reference

Andersen, R., Chung, F., and Lang, K. (2006). Local Graph Partitioning using PageRank Vectors. FOCS 2006.

## Benchmarks

Single-threaded, single-seed PPR queries on random sparse graphs (avg degree 4, alpha 0.15, epsilon 1e-4), captured on the developer's M1 Max via the harness in `tests/test_benchmarks.py`:

| Nodes | Native | Python | Speedup |
|-------|--------|--------|---------|
| 50K   | 0.0024s | 0.0318s | 13.2x |
| 200K  | 0.0034s | 0.1753s | 51.3x |
| 1M    | 0.0023s | 0.9573s | 413.9x (acceptance gate: must be >= 20x) |

To re-run:

```bash
cd theseus_native
python3 -m pytest tests/test_benchmarks.py -v -s
```

The fixture is generated with seed 42 for reproducibility. Numbers vary across hardware; the 20x floor is enforced on whatever runner executes the test.

The native impl uses lazy on-demand neighbor extraction: ACL Push typically touches ~1/(epsilon*alpha) ~ 67k nodes for production params, so converting only those (not the full adjacency dict) eliminates the dominant FFI cost.

## License

MIT.
