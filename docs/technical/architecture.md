# Architecture

## Workspace layout

RustyRed is a Cargo workspace. One core engine crate is shared by every network surface, so the
HTTP server, the MCP adapter, and direct Rust callers all execute the same command logic.

| Crate | Path | Role |
|-------|------|------|
| `rusty_red_native` | `src/lib.rs` | Root facade. Re-exports the core graph algorithms and adds `push_ppr`, an integer-id ACL local-push Personalized PageRank helper. No Python or native-extension dependency. |
| `rustyred-core` | `crates/rustyred-core` | The engine: command vocabulary, executor, graph store (in-memory / RedCore / Redis), vector / spatial / full-text indexes, versioning, and the Instant-KG merged views. |
| `rustyred-server` | `crates/rustyred-server` | The product server: axum HTTP API + tonic gRPC on one port, auth, config, the Cypher surface, the graph cache, metrics, and the `rustyred-upgrade-format` tool. |
| `rustyred-search` | `crates/rustyred-search` | Standalone RustyWeb crawl/search kernel: guarded fetching, robots handling, crawl graph emission, SERP payloads, and Web Commons fragment types. |
| `rustyred-mcp` | `crates/rustyred-mcp` | Model Context Protocol adapter — turns the core into a JSON-RPC tool/resource server. |
| `rustyred-compat-server` | `crates/rustyred-compat-server` | Minimal standalone HTTP command server over the core executor (compatibility / embedding). |
| `rustyred-resp-server` | `crates/rustyred-resp-server` | Experimental Redis-RESP listener scaffold (see below). |

The release binary is `rustyred-server`. The Dockerfile builds only that crate.

## Request flow

```
HTTP / gRPC client ─┐
MCP client ─────────┤→ rustyred-server ─→ AppState ─→ rustyred-core
RESP client (exp.) ─┘    (axum + tonic)    (per-tenant   (GraphStore: InMemory │ RedCore │ Redis)
                                            stores)
```

`rustyred-server`'s `main` reads `Config::from_env()`, validates it, builds the axum router and the
tonic gRPC routes, and **merges them onto a single TCP listener**. Content-type sniffing routes
`application/grpc*` traffic to gRPC and everything else to the HTTP handlers, so both protocols
answer on the same port (default `8380`). Handlers resolve the target tenant, acquire that tenant's
graph store from `AppState`, and dispatch into `rustyred-core`.

## Storage modes

`RUSTY_RED_MODE` selects the backend (`crates/rustyred-server/src/config.rs`):

- **`embedded`** *(default)* — the **RedCore** native engine. The working graph lives in memory;
  durability comes from an append-only file (AOF) plus periodic snapshots on the data volume.
- **`memory`** — in-memory only, no persistence. For ephemeral test deployments.
- **`redis`** — legacy compatibility backend (requires the `redis-store` build feature). Not used
  by the Railway template and not recommended for new deployments.

All three implement the same `GraphStore` trait, so the API surface is identical regardless of mode.

## The RedCore storage engine

RedCore (`crates/rustyred-core/src/graph_store.rs`) is a Redis-style durability layer implemented
natively in Rust around an `InMemoryGraphStore`.

**Durability modes** (`RUSTY_RED_DURABILITY`):

| Mode | Behaviour |
|------|-----------|
| `aof_everysec` *(default)* | Append each mutation to the AOF; fsync batched roughly once per second. |
| `aof_always` | Synchronous fsync on every write. Required for strict-ACID mode. |
| `snapshot_only` | No AOF; rely on periodic snapshots. |
| `none` | No persistence (memory). |

**Snapshots.** A full snapshot is written every `RUSTY_RED_SNAPSHOT_INTERVAL_WRITES` mutations
(default `1000`), bounding AOF replay time on restart.

**On-disk artifacts.** A `manifest` records the format version, graph version, last/snapshot
transaction ids, durability mode, and the snapshot/AOF filenames. A **directory lock** prevents two
processes from opening the same data directory. The current on-disk format version is
`1` (`CURRENT_FORMAT_VERSION`); a build refuses to load a snapshot whose manifest version is newer
than it understands.

**Recovery.** On startup RedCore loads the latest snapshot, then replays AOF frames recorded after
the snapshot transaction id. Each AOF frame carries a payload checksum. Recovery tolerates orphan
edges (edges whose endpoints were not yet present) rather than aborting.

**Transactions.** Mutations apply as transactions with a monotonic `txn_id` and a `graph_version`.
The HTTP transaction API (`/v1/transactions/*`) exposes begin/commit/rollback with snapshot
isolation; strict-ACID mode (`RUSTY_RED_STRICT_ACID=true`) upgrades this to serializable,
single-writer, `aof_always` and is only valid in `embedded` mode.

## Index structures

`InMemoryGraphStore` maintains, alongside the node and edge maps:

- **Out/in adjacency** keyed by `(node_id, edge_type)` for directional neighbor lookups.
- **Label index**, **edge-type index**, and a **property index** keyed by `(property, value)`.
- **Vector designations and HNSW indexes** keyed by `(label, property)`.

Full-text (BM25) and spatial (H3) indexes are separate subsystems layered on the same store.

## Higher-level subsystems

- **Versioned graph** (`versioned_graph.rs`) — content-addressed node/edge objects compile into
  Prolly-style trees with Git-like commits, refs/branches, diff, checkout, and merge. See
  [Data model](data-model.md) and the version endpoints in [HTTP API](http-api.md).
- **Instant-KG** (`instant_kg.rs`) — "Harness" merged views that overlay session-fresh code deltas
  on a durable tenant graph for code PageRank, impact analysis, related-object lookup, search, and
  edge explanations. Protocol id `harness-instant-kg-v1`.
- **Graph cache** (`graph_cache.rs`) — a graph-state-aware result cache with ten entry kinds that
  invalidate when the underlying graph mutates. See [Query surface](query-surface.md).
- **RustyWeb / Web Commons federation** (`rustyred-search` + `router.rs`) — bounded crawls write
  Page/Domain/ContentSnapshot/LINKS_TO graph fragments locally. When federation is enabled, a
  deployment signs a bounded Web Commons fragment with Ed25519 and submits it to a hub. The hub
  verifies the signature, checks peer trust, stores accepted pages as probationary/canonical, and
  uses the versioned-graph merge primitive before committing the fragment batch.

## A note on the RESP server

`rustyred-resp-server` is a **scaffold**, not a finished feature. Its `main` binds a TCP listener
(default `127.0.0.1:6380`, override with `RUSTYRED_RESP_ADDR`) and currently accepts and drops
connections without serving them. A command-mapping helper exists but is not yet wired to the
executor and maps only the `RUSTYRED.RUN.*` / `RUSTYRED.STATE.HASH` commands. Treat RESP as
experimental and do not depend on it in production.
