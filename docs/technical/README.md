# RustyRed GraphDB — Technical Documentation

Technical reference for **RustyRed GraphDB `0.6.0`**, derived from the source tree rather than
marketing copy. Two audiences are covered:

- **Integration developers** — building against the HTTP, gRPC, or MCP surfaces.
- **Operators** — deploying, configuring, and running the release.

RustyRed is an in-memory graph **and** vector database with append-only-file/snapshot
persistence, multi-tenant isolation, confidence-weighted *epistemic* edges, HNSW vector search,
BM25 full-text, H3 spatial indexing, a bounded Cypher surface, a graph-state-aware cache, and a
first-class MCP agent port. It is written in Rust and ships as a single service with no Redis
sidecar required.

## Map

| Document | For | Covers |
|----------|-----|--------|
| [Architecture](architecture.md) | Both | Crates, request flow, the RedCore storage engine, persistence & recovery |
| [Data model](data-model.md) | Devs | Nodes, edges, epistemic types, properties, versioning, content addressing, tenancy |
| [HTTP API](http-api.md) | Devs | Complete REST reference with `curl` examples and the error model |
| [Query surface](query-surface.md) | Devs | Structured `/v1/query`, the Cypher subset, and the graph-aware cache |
| [gRPC API](grpc-api.md) | Devs | `rustyred.v1.GraphDatabase` service and RPC catalog |
| [MCP agent port](mcp.md) | Devs | JSON-RPC methods, tool/resource catalog, read-only & admin gating |
| [Deployment](deployment.md) | Operators | Docker, Railway, ports, volumes, format upgrades, build features |
| [Configuration](configuration.md) | Operators | Every environment variable, auth model, scopes |
| [Observability](observability.md) | Operators | Prometheus metrics, diagnostics, health/readiness |

## At a glance

- **Latest version:** `0.6.0` (workspace `Cargo.toml`), Rust edition 2021, MSRV 1.85.
- **Default storage mode:** `embedded` (RedCore native engine; in-memory graph + AOF + snapshots).
- **Default HTTP/gRPC port:** `8380` (HTTP and gRPC share one listener).
- **Default RESP port:** `6380` (experimental scaffold — see [Architecture](architecture.md)).
- **Auth:** Bearer tokens with scopes; `RUSTY_RED_REQUIRE_AUTH` defaults to **true**.
- **On-disk format version:** `1` (`CURRENT_FORMAT_VERSION`); migrate with `rustyred-upgrade-format`.

> Source of truth for configuration is `crates/rustyred-server/src/config.rs`; for the data model,
> `crates/rustyred-core/src/graph_store.rs`. Where this documentation and the README disagree, the
> code wins.
