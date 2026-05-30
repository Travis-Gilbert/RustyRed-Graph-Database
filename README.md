# RustyRed GraphDB

RustyRed is a remarkably fast Graph + Vector database.
It runs entirely in RAM.
Designed to help humans and their agents work well together.

Featuring GraphCache graph state-aware cache, a first-class MCP agent port, built-in-RAG both graph and vector
multi-tenancy, HNSW vector search, confidence-weighted epistemic edges, and document storage.
Written in Rust, the best way to write a database. In my humble opinion.

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/new/template/rustyred-graphdb?utm_medium=integration&utm_source=button&utm_campaign=rusty-red-graph-database)

Clicking the button deploys a single Railway service with a persistent volume, security-by-default, and a freshly generated API token. You get the same in-memory RAM-first graph database documented below — no Redis sidecar, no second service, no extra moving parts.

## What Rusty Red does

- **Graph storage** with AOF/snapshot persistence, per-tenant isolation, single-writer serializable commits, and committed read snapshots
- **Stable, versioned on-disk format** with `rustyred-upgrade-format` migrations between releases (no export/re-import on upgrade)
- **HNSW vector search** on node properties via `instant-distance`, with hybrid scoring that blends vector similarity and graph proximity
- **Inverted-index BM25 full-text search** with automatic indexing on node upserts
- **H3 spatial index** on node lat/lon properties with radius and bounding-box queries
- **Epistemic edge types** (Supports, Contradicts, Tension, Derives, Cites) with confidence-weighted traversal across configurable hop depth
- **Graph algorithms over HTTP/MCP**: PPR, connected components, PageRank, and label-propagation community detection
- **Harness Instant KG merged views**: session-fresh code deltas overlay durable tenant graph artifacts for code PPR, impact analysis, related-object lookup, search, and edge explanations
- **Git-like graph version packs**: content-addressed node/edge records compile into Prolly-style trees, commit metadata, and declarative validation artifacts without bundling private Skill Encoder logic
- **MCP agent port** with scoped auth tokens, read-only and read-write modes, tool annotations, and structured tool/resource/prompt surfaces
- **Graph-version-aware cache** (10 kinds) that detects stale entries when the underlying graph mutates
- **Bounded Cypher surface**: single-hop and outgoing multi-hop MATCH, bounded variable-length expand, path aliases, property projections, `COUNT(*) / COUNT(binding)`, and transaction-scoped `CREATE`/`MERGE`/`SET`/`DELETE`
- **JSONL bulk loader** for nodes and edges
- **Observability**: Prometheus `/metrics` (17 counters), slow-query ring buffer at `/v1/diagnostics/slow_queries`
- **HTTP transaction API**: `/v1/transactions/begin|commit|rollback` with snapshot isolation
- **Native algorithm helpers** exposed through the root PyO3 compatibility crate, including ACL local-push Personalized PageRank

---

## Deploy on Railway

### Quickstart (one-click)

1. Click the **Deploy on Railway** badge above.
2. Railway will show you the variables the template will set. The only one that matters for first-time use is `RUSTY_RED_API_TOKENS` — it is pre-filled with a freshly generated 64-character hex secret. Copy it somewhere safe; this is the bearer token your clients will use.
3. Click **Deploy**. Railway provisions the service, attaches a 1 GiB volume at `/app/data/rusty-red`, and starts the container. The health probe waits for `/ready` to return 200.
4. Open `https://<your-service>.up.railway.app/openapi.json` to verify the service is reachable, then make your first authenticated request:

```bash
curl -H "Authorization: Bearer <your-token>" \
     https://<your-service>.up.railway.app/v1/diagnostics/config
```

For the full operator guide — backups, scaling, upgrade path, troubleshooting — see [docs/railway-template.md](docs/railway-template.md).

### Manual Railway deploy (without the template)

If you want to manage the deploy yourself instead of using the template:

1. Fork or clone this repository to your GitHub account.
2. Create a new Railway project pointing at your fork.
3. Railway will detect `railway.toml` and use the bundled `Dockerfile`. Healthcheck path is `/ready`.
4. Attach a volume mounted at `/app/data/rusty-red`.
5. Set the required environment variables (see [Environment variable reference](#environment-variable-reference)). At minimum:
   - `RUSTY_RED_API_TOKENS=<your-token>=graph:read|graph:write|context:read|admin:read`
   - Generate the token with `openssl rand -hex 32`.

### Persistence and the volume

RustyRed is RAM-first but durable. State lives in memory while the service runs; durability is provided by an append-only log plus periodic snapshots written to the data directory.

- **Volume requirement is enforced.** With `RUSTY_RED_REQUIRE_VOLUME=true` (the shipped default), the service refuses to start unless a persistent volume is mounted at `RUSTY_RED_DATA_DIR`. This is by design — silently running on ephemeral storage would lose data on every redeploy.
- **Railway redeploys preserve the volume.** Service recreations do not. Back up before destructive operations.
- **Backup procedure.** Stop the service or pause writes, copy the snapshot and AOF files out of the data directory, restart. Snapshots are taken every `RUSTY_RED_SNAPSHOT_INTERVAL_WRITES` writes; AOF replays the gap on restart.
- **Volume sizing.** The 1 GiB default supports meaningful exploration. For production workloads, scale the volume in the Railway service settings before ingesting bulk data.

### Auth model in one screen

The default Dockerfile ships **auth required**. `/v1/*` and `/mcp` reject unauthenticated requests; only `/health`, `/ready`, `/openapi.json`, `/metrics`, and the `.well-known/*` advertisement endpoints stay open.

Authentication is bearer-token. Tokens live in `RUSTY_RED_API_TOKENS` as a comma-separated list, each entry shaped `<secret>=<scope>|<scope>|...`. Scopes:

| Scope | Grants |
|---|---|
| `graph:read` | All read routes (query, neighbors, vector/fulltext/spatial search, stats) |
| `graph:write` | All `graph:read` plus mutating routes (Cypher writes, node/edge upserts, bulk ingest) |
| `context:read` | `/v1/tenants/{id}/context/pack` |
| `admin:read` | Verify, rebuild-indexes, diagnostics, MCP admin tool surface (only if `RUSTY_RED_MCP_ALLOW_ADMIN=true`) |
| `*` | All of the above. Operator emergency access; do not use as an application token. |

**Tokens are tenant-blind** — a `graph:write` token can write to any tenant on the instance. Multi-tenant deployments that need per-tenant isolation should either run one RustyRed instance per tenant or front the service with an external auth layer.

Full threat model, supported aliases, and reporting process: [SECURITY.md](SECURITY.md).

### Environment variable reference

Source of truth is `crates/rustyred-server/src/config.rs`. The Dockerfile defaults are intentional production defaults; Railway template variables override them at deploy time.

| Variable | Default | Required-when | Notes |
|---|---|---|---|
| `PORT` | (Railway-injected) | — | Standard Railway port. RustyRed reads `PORT` first, then falls back to `RUSTY_RED_PORT`. |
| `RUSTY_RED_HOST` | `[::]` | — | Bind address. |
| `RUSTY_RED_PORT` | `8380` | — | Used only if `PORT` is unset. |
| `RUSTY_RED_MODE` | `embedded` | — | `embedded` is the standalone product mode. `redis` is a legacy compatibility path; not used by the Railway template. |
| `RUSTY_RED_DATA_DIR` | `/app/data/rusty-red` | always | Must match the mounted volume path. |
| `RAILWAY_VOLUME_MOUNT_PATH` | (Railway-injected) | template | Satisfies `RUSTY_RED_REQUIRE_VOLUME=true`. |
| `RUSTY_RED_REQUIRE_VOLUME` | `true` | — | When true, refuse to start without a mounted volume. Keep on. |
| `RUSTY_RED_VOLUME_MOUNTED` | (unset) | — | Manual override for non-Railway deployments that mount the volume themselves. |
| `RUSTY_RED_DURABILITY` | `aof_everysec` | — | `aof_always` for strict, `aof_everysec` for default, `none` for ephemeral. |
| `RUSTY_RED_SNAPSHOT_INTERVAL_WRITES` | `1000` | — | Writes between snapshots. |
| `RUSTY_RED_STRICT_ACID` | `false` | — | When true, requires `MODE=embedded`, `DURABILITY=aof_always`, `CONCURRENCY=single_writer`, `TXN_ISOLATION=serializable`. |
| `RUSTY_RED_CONCURRENCY` | (engine default) | — | `single_writer` for strict ACID. |
| `RUSTY_RED_TXN_ISOLATION` | (engine default) | — | `serializable` for strict ACID. |
| `RUSTY_RED_REQUIRE_AUTH` | `true` | — | Keep on for any reachable endpoint. |
| `RUSTY_RED_API_TOKENS` | (empty) | when `REQUIRE_AUTH=true` | Comma-separated `<secret>=<scope>\|<scope>\|...` entries. |
| `RUSTY_RED_KEY_PREFIX` | `rusty-red:tenant` | — | Per-tenant keyspace prefix. |
| `RUSTY_RED_SERVICE_NAME` | `rusty-red-graph-database` | — | Appears in OpenAPI and `.well-known/*` metadata. |
| `RUSTY_RED_API_TITLE` | `Rusty Red Graph Database API` | — | OpenAPI title. |
| `RUSTY_RED_PUBLIC_URL` | (unset) | — | Public base URL; used in OpenAPI `servers` block. |
| `RUSTY_RED_ALLOWED_ORIGINS` | (empty) | — | CORS allowlist; comma-separated origins. |
| `RUSTY_RED_MCP_ENABLED` | `true` | — | Master switch for `/mcp`. |
| `RUSTY_RED_MCP_READ_ONLY` | `true` | — | Keeps write tools unreachable until you opt in. |
| `RUSTY_RED_MCP_ALLOW_ADMIN` | `false` | — | Exposes the admin tool surface; requires `admin:read` token. |
| `RUSTY_RED_MCP_DEFAULT_TENANT` | `default` | — | Tenant assumed for MCP calls that do not specify one. If unset, the runtime falls back to the literal string `default`; set explicitly in multi-tenant deployments. |
| `RUSTY_RED_TENANT_MEMORY_QUOTA_BYTES` | (unset) | — | Per-tenant memory ceiling. Currently enforced for `embedded` and `memory` modes. |
| `RUSTY_RED_SLOW_QUERY_NANOS` | (engine default) | — | Threshold for the slow-query ring buffer. |
| `RUSTY_RED_SLOW_QUERY_CAPACITY` | (engine default) | — | Slow-query ring buffer size; must be > 0. |
| `RUSTY_RED_SLOW_QUERY_LOG` | (engine default) | — | Whether to log slow queries in addition to the ring buffer. |
| `RUSTY_RED_FULLTEXT_BACKEND` | (hand-rolled BM25) | — | Internal switch; default is the bundled BM25. |
| `RUSTY_RED_SPATIAL_BACKEND` | `h3` | — | `s2` available behind the `s2` build feature. |
| `RUSTY_RED_TENANT_CONFIG_PATH` | (unset) | — | Path to a JSON file with per-tenant overrides at startup. |
| `RUSTY_RED_TENANT_CONFIG_JSON` | (unset) | — | Inline per-tenant override JSON; alternative to the path form. |

Legacy compatibility aliases (`RUSTYRED_PRODUCT_*`, `RUSTYRED_REDIS_*`, `RUSTYRED_MCP_*`, etc.) are accepted by `config.rs` for backward-compat; new deployments should use the `RUSTY_RED_*` names above. `RUSTY_RED_REDIS_URL` is only consulted when `RUSTY_RED_MODE=redis` (the legacy compatibility deployment path); it has no effect on the standalone `embedded` template path.

A copy-pasteable starter is at [`.env.example`](.env.example).

### Observability

- `GET /metrics` — Prometheus exposition with 17 counters covering request totals, auth rejections, tenant key namespace activity, cache hit/miss, write commits, and bulk ingest. Scrape from your monitoring stack.
- `GET /v1/diagnostics/slow_queries` — ring buffer of slow queries with timing and plan info. Capacity is `RUSTY_RED_SLOW_QUERY_CAPACITY`; threshold is `RUSTY_RED_SLOW_QUERY_NANOS`.
- `GET /v1/diagnostics/config` — current runtime config snapshot, including tenant override details.

**Alarm on:** auth-rejection rate spikes (first signal of a credential leak or brute force), unexpected sustained write-rate growth, slow-query buffer saturation, or any non-200 from `/ready`.

### Upgrade and version pinning

- **Track tagged releases**, not `main`. `main` may carry unreleased changes; tagged releases are what receive security fixes (see [SECURITY.md](SECURITY.md)).
- **On-disk format is stable across releases.** The `rustyred-upgrade-format` migration step runs at startup and rewrites the AOF/snapshot pair if needed. There is no export/re-import flow on upgrade.
- **Backup before upgrading** anyway. Volume snapshots are cheap insurance.

---

## Develop & extend

### What you can't do yet

These are on the roadmap, in roughly this priority order:

1. Incoming and undirected Cypher relationship patterns, plus the rest of full OpenCypher/GQL coverage
2. `OPTIONAL MATCH`, `WITH`, `UNION`, `CALL`, `ORDER BY`, `SKIP`, `DISTINCT`
3. `SUM` / `AVG` / `MIN` / `MAX` aggregations
4. `REMOVE` clauses
5. CSV/JSONL `LOAD CSV` syntactic form (JSONL bulk endpoints exist already)
6. Distributed snapshot replication
7. Per-query spatial backend selection; H3 ships by default and S2 is available behind the `s2` feature plus `RUSTY_RED_SPATIAL_BACKEND=s2`

### Crate structure

| Crate | Purpose |
|-------|---------|
| `rustyred-core` | Graph store engine, command executor, HNSW vector index, epistemic edges |
| `rustyred-mcp` | MCP agent port: tool dispatch, resource reads, prompt surface |
| `rustyred-server` | HTTP server, query surface, graph cache, auth, OpenAPI |
| `rustyred-compat-server` | Standalone compatibility command server |
| `rustyred-resp-server` | RESP protocol shim (limited, not a Redis replacement) |
| root crate | PyO3 compatibility bindings for native graph/search helpers |

### Build (local development)

Requires Rust 1.85+ and `maturin >= 1.7`. The repo vendors the `rustyred.v1` proto at `vendor/proto/` so a fresh clone builds without any submodule init step:

```bash
git clone https://github.com/Travis-Gilbert/RustyRed-Graph-Database.git
cd RustyRed-Graph-Database
cargo check --workspace
maturin develop --release
```

The `theorem-protos` submodule at `proto/` is optional for development; pull it only if you intend to edit the upstream proto contract:

```bash
git submodule update --init
# edit proto/rustyred/v1/rustyred.proto
scripts/sync-vendored-proto.sh   # mirrors your edits into vendor/proto/
```

CI checks that `vendor/proto/` matches the submodule on every PR via `.github/workflows/vendored-proto-up-to-date.yml`. See [docs/adr/0001-vendored-proto-for-railway-build.md](docs/adr/0001-vendored-proto-for-railway-build.md) for the rationale.

`cargo check --workspace` validates the Rust workspace (including the tonic gRPC server scaffolded against `vendor/proto/rustyred/v1/`).
`maturin develop --release` builds the root `abi3-py312` compatibility wheel into the active Python environment.

### Product server

The product server runs in `RUSTY_RED_MODE=embedded` with RedCore RAM-first storage and local AOF/snapshot persistence. It exposes graph operations, vector search, epistemic traversal, Cypher queries, and the graph-version cache over HTTP and MCP.

`RUSTY_RED_MODE=redis` is available only for legacy compatibility deployments. The normal Rusty Red service does not require Redis, FalkorDB, Memgraph, or any second Rusty Red service.

Run the product server locally:

```bash
RUSTY_RED_MODE=embedded RUSTY_RED_DATA_DIR=data/rusty-red cargo run -p rustyred-server
```

For low-memory local builds, cap Cargo's parallel compiler jobs:

```bash
CARGO_BUILD_JOBS=2 RUSTY_RED_MODE=embedded RUSTY_RED_DATA_DIR=data/rusty-red cargo run -p rustyred-server
```

For a low-memory local Docker build and run:

```bash
docker build --build-arg CARGO_BUILD_JOBS=2 -t rustyred-local .
docker run --rm \
  -p 8380:8380 \
  -v "$PWD/data/rusty-red:/app/data/rusty-red" \
  -e RUSTY_RED_VOLUME_MOUNTED=true \
  -e RUSTY_RED_REQUIRE_AUTH=false \
  rustyred-local
```

Strict local durability mode is explicit:

```bash
RUSTY_RED_MODE=embedded \
RUSTY_RED_CONCURRENCY=single_writer \
RUSTY_RED_TXN_ISOLATION=serializable \
RUSTY_RED_STRICT_ACID=true \
RUSTY_RED_DURABILITY=aof_always \
RUSTY_RED_DATA_DIR=data/rusty-red \
cargo run -p rustyred-server
```

Core routes are documented by `GET /openapi.json`. The OpenAPI document is generated from the product server crate version and currently covers every canonical route in `crates/rustyred-server/src/router.rs`.

### Canonical routes

```text
GET  /health
GET  /ready
GET  /openapi.json
GET  /.well-known/mcp/rustyred.json
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
POST /v1/tenants/{tenant_id}/graph/version/compile
POST /v1/tenants/{tenant_id}/graph/version/diff
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
POST /v1/tenants/{tenant_id}/instant-kg/status
POST /v1/tenants/{tenant_id}/instant-kg/ppr
POST /v1/tenants/{tenant_id}/instant-kg/impact
POST /v1/tenants/{tenant_id}/instant-kg/related-objects
POST /v1/tenants/{tenant_id}/instant-kg/search
POST /v1/tenants/{tenant_id}/instant-kg/explain-edge
GET  /v1/diagnostics/slow_queries
GET  /v1/diagnostics/config
POST /v1/tenants/{tenant_id}/context/pack
```

`GET /health/` and `GET /ready/` are trailing-slash aliases for deployment probes.

The public query surface splits cleanly:

- `/v1/query` is the product-facing native subset for `node_match` and `neighbors`.
- `/v1/cypher` and `/v1/cypher/explain` are the bounded OpenCypher-compatible surface for read queries plus transaction-scoped `CREATE`/`MERGE`/`SET`/`DELETE` writes.
- `/v1/tenants/{tenant_id}/graph/query` remains the older debug bridge and should not be treated as the product route.

The version routes are the public Git/provenance substrate. `/graph/version/compile` reads the current tenant graph and returns a `rustyred-versioned-graph-v1` pack: content hashes, a Prolly-style tree root, Git-like commit metadata, and declarative compiler capabilities such as a tree-root validator. `/graph/version/diff` compares a supplied base snapshot against the current graph, or against an explicit target snapshot. Full corpus-to-skill encoding, domain pack lowering, LoRA adapters, and Skill Encoder promotion policies belong downstream in Theseus/Theorem, not in this open release.

### MCP tools

The `/mcp` endpoint exposes these tools (via JSON-RPC `tools/list` and `tools/call`):

| Tool | Description |
|------|-------------|
| `rustyred.graph.query` / `rustyred.graph.explain` / `rustyred.graph.neighbors` | Bounded native graph reads and plan inspection |
| `rustyred.graph.schema` / `rustyred.graph.index_status` | Graph schema and index-health reads |
| `rustyred.graph.version.compile` (`rustyred.git.compile`) / `rustyred.graph.version.diff` (`rustyred.git.diff`) | Public content-addressed graph pack compiler and snapshot diff tools |
| `rustyred.algorithm.ppr` (alias: `rustyred.algo.ppr`) / `rustyred.algorithm.components` (`rustyred.algo.components`) / `rustyred.algorithm.pagerank` (`rustyred.algo.pagerank`) / `rustyred.algorithm.communities` (`rustyred.algo.communities`) | Graph algorithms: PPR, connected components, PageRank, label-propagation communities |
| `harness_kg_status` / `harness_kg_ppr` / `harness_kg_impact` / `harness_kg_related_objects` / `harness_kg_search` / `harness_kg_explain_edge` | Harness Instant KG tools over a RedCore tenant base graph plus an optional session delta. Legacy `RUSTY_RED_MODE=redis` returns a diagnostic because Instant KG is a native RustyRed capability. |
| `rustyred.fulltext.search` (alias: `rustyred.graph.fulltext.search`) / `rustyred.spatial.radius` (`rustyred.graph.spatial.radius`) / `rustyred.spatial.bbox` (`rustyred.graph.spatial.bbox`) | Full-text and spatial read surfaces |
| `rustyred.vector.search` | HNSW nearest-neighbor search on vector properties |
| `rustyred.vector.hybrid` | Hybrid search blending vector similarity with graph proximity |
| `rustyred.vector.designate` | Register a vector property for HNSW indexing (write) |
| `rustyred.epistemic.neighbors` | Confidence-weighted epistemic traversal by edge type |
| `rustyred.fulltext.designate` (alias: `rustyred.graph.fulltext.designate`) / `rustyred.spatial.designate` (`rustyred.graph.spatial.designate`) / `rustyred.bulk.nodes` (`rustyred.graph.bulk.nodes`) / `rustyred.bulk.edges` (`rustyred.graph.bulk.edges`) | Write-mode-only designation and bulk ingest tools |
| `rustyred.admin.verify` | Admin-only index-integrity verification; rebuild remains on the HTTP graph route |

### Compatibility command server

The product server is the recommended deployment target. A smaller compatibility HTTP command server also lives in `crates/rustyred-compat-server`:

```bash
cargo run -p rustyred-compat-server -- --host 127.0.0.1 --port 7379
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

### Python compatibility bindings

The root crate exposes native helper functions through PyO3 for Python 3.12+. These bindings are not required for the Railway template. The most important algorithm helper is:

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

### Downstream integrations

This repository is the upstream source for versioned Rusty Red releases. Product integrations can consume it as a downstream `git subtree` and keep their own deployment adapters or private overlays downstream. The included `.github/workflows/sync-downstream.yml` can open downstream sync PRs after pushes to `main` when `DOWNSTREAM_SYNC_REPOSITORY` and `DOWNSTREAM_SYNC_TOKEN` are configured in repository settings.

See [`docs/downstream-sync.md`](docs/downstream-sync.md) for the setup contract and the local subtree sync command.

### Benchmarks

Single-threaded, single-seed PPR queries on random sparse graphs (average degree 4, alpha 0.15, epsilon 1e-4), captured on an M1 Max via `tests/test_benchmarks.py`:

| Nodes | Native | Python | Speedup |
|-------|--------|--------|---------|
| 50K   | 0.0024s | 0.0318s | 13.2x |
| 200K  | 0.0034s | 0.1753s | 51.3x |
| 1M    | 0.0023s | 0.9573s | 413.9x (acceptance gate: must be >= 20x) |


The fixture is generated with seed 42 for reproducibility. Numbers vary across hardware; the 20x floor is enforced on whatever runner executes the test.

The native impl uses lazy on-demand neighbor extraction: ACL Push typically touches ~1/(epsilon*alpha) ~ 67k nodes for production params, so converting only those (not the full adjacency dict) eliminates the dominant FFI cost.

### Algorithm reference

Andersen, R., Chung, F., and Lang, K. (2006). Local Graph Partitioning using PageRank Vectors. FOCS 2006.

## License

MIT.
