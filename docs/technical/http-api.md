# HTTP API

Base URL is the deployed service root; the default bind is `http://127.0.0.1:8380` locally and
`0.0.0.0:8380` (or the platform `PORT`) in a container. All request and response bodies are JSON
(`Content-Type: application/json`) unless noted.

## Authentication

When `RUSTY_RED_REQUIRE_AUTH=true` (the default), search/crawl/federation routes, `/v1/*`, `/mcp`,
`/metrics`, and diagnostics need a bearer token:

```
Authorization: Bearer <token>
```

Tokens and their scopes are configured via `RUSTY_RED_API_TOKENS`. Each endpoint requires a scope —
reads/search need `graph:read`, writes/crawls need `graph:write`, federation submits need
`federation:write`, context packing needs `context:write`, run reads need `run:read`, and
diagnostics/metrics need `admin:read`. A `*` scope grants everything. See [Configuration](configuration.md)
for the token format and the full scope list.

Responses: `401 Unauthorized` (missing/malformed bearer), `403 Forbidden` (unknown token or missing
scope).

## Tenancy

Most data endpoints are tenant-scoped at `/v1/tenants/{tenant_id}/…`. Root-level convenience
endpoints (`/v1/command`, `/v1/query`, `/v1/cypher`, `/v1/cache/*`, `/v1/transactions/*`) operate on
the default tenant (or accept an optional `tenant_id` in the body).

## Error model

Graph-store errors return `{ "error": "<code>", "message": "<text>" }`. Query-surface errors return
`{ "ok": false, "error": "<code>", "message": "<text>" }`. HTTP status is mapped from the code, e.g.:

| Condition | Status |
|-----------|--------|
| Validation / unsupported feature (`empty_graph_field`, `dimension_mismatch`, `unsupported_operation`, `unsupported_cypher_feature`, …) | `400` |
| Tenant memory quota exceeded | `429` |
| Backend unavailable (`redcore_io_error`, `redis_graph_store_error`, store mode unsupported) | `503` |
| Auth failures | `401` / `403` |

---

## System & metadata

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/health` (`/health/`) | none | Liveness. Returns `{"status":"ok"}`. |
| `GET` | `/ready` (`/ready/`) | none | Readiness — checks the store is open. Railway healthcheck path. |
| `GET` | `/openapi.json` | none | OpenAPI 3 document for the whole HTTP surface. |
| `GET` | `/.well-known/mcp/rustyred.json` | none | MCP server manifest (404 if MCP disabled). |
| `GET` | `/.well-known/agent.json` | none | Agent port manifest (404 if MCP disabled). |
| `POST` | `/mcp` | `graph:read`+ | MCP JSON-RPC endpoint — see [MCP](mcp.md). |
| `GET` | `/metrics` | `admin:read` | Prometheus text exposition — see [Observability](observability.md). |
| `GET` | `/v1/diagnostics/slow_queries` | `admin:read` | Slow-query ring buffer. |
| `GET` | `/v1/diagnostics/config` | `admin:read` | Effective static configuration. |

## Standalone RustyWeb

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/` | `graph:read` | Search home / SERP shell for the default tenant. |
| `GET` | `/search?q=...&tenant=...` | `graph:read` | HTML SERP over the tenant crawl graph. |
| `GET` | `/search.json?q=...&tenant=...` | `graph:read` | JSON graph-native search results. |
| `POST` | `/crawl` | `graph:write` | Run a bounded RustyWeb crawl, commit it locally, and optionally submit a signed Web Commons fragment to the hub. |
| `POST` | `/federate/submit` | `federation:write` | Verify and merge a signed Web Commons fragment; receipt/hash-only submissions validate as no-op compatibility payloads. |

```bash
curl -s http://localhost:8380/ready
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:8380/v1/diagnostics/config
```

## Graph: nodes & edges

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/nodes` | `graph:write` | Upsert a node. |
| `POST` | `/v1/tenants/{t}/graph/nodes/query` | `graph:read` | Query nodes by label/properties. |
| `GET` | `/v1/tenants/{t}/graph/nodes/{node_id}` | `graph:read` | Fetch a node by id. |
| `POST` | `/v1/tenants/{t}/graph/edges` | `graph:write` | Upsert an edge. |
| `GET` | `/v1/tenants/{t}/graph/edges/{edge_id}` | `graph:read` | Fetch an edge by id. |
| `POST` | `/v1/tenants/{t}/graph/neighbors` | `graph:read` | Directional neighbor expansion. |
| `GET` | `/v1/tenants/{t}/graph/stats` | `graph:read` | Graph statistics. |
| `GET` | `/v1/tenants/{t}/graph/verify` | `graph:read` | Integrity verify report. |
| `POST` | `/v1/tenants/{t}/graph/rebuild-indexes` | `graph:write` | Rebuild derived indexes. |

**Upsert a node** — body fields: `id` (required), `labels[]`, `properties`, `tombstone`. Returns
`{ id, version, checksum }`.

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  http://localhost:8380/v1/tenants/acme/graph/nodes \
  -d '{"id":"claim:1","labels":["Claim"],"properties":{"text":"Water boils at 100C"}}'
```

**Upsert an edge** — body: `id`, `from_id`, `to_id`, `type` (required), `properties`, `tombstone`.
Epistemic fields (`confidence`, `epistemic_type`, `provenance`) are accepted on the edge record.

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  http://localhost:8380/v1/tenants/acme/graph/edges \
  -d '{"id":"e:1","from_id":"claim:1","to_id":"claim:2","type":"SUPPORTS",
       "confidence":0.9,"epistemic_type":"supports"}'
```

**Neighbors** — body: `node_id`, `direction` (`out`|`in`), optional `edge_type`, optional `budget`.

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  http://localhost:8380/v1/tenants/acme/graph/neighbors \
  -d '{"node_id":"claim:1","direction":"out"}'
```

## Graph: query

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/query` | `graph:read` | Structured/Cypher query against a tenant. |
| `POST` | `/v1/query` | `graph:read` | Structured query, default tenant. |
| `POST` | `/v1/cypher` | `graph:read`/`graph:write` | Run a Cypher-subset statement. |
| `POST` | `/v1/cypher/explain` | `graph:read` | Plan a Cypher statement without executing. |

See [Query surface](query-surface.md) for the structured-query operations and the supported Cypher
grammar. Cypher body: `{ "query": "...", "params": { }, "tenant_id"?, "tx_id"? }`.

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  http://localhost:8380/v1/cypher \
  -d '{"tenant_id":"acme","query":"MATCH (c:Claim) RETURN c LIMIT 10"}'
```

## Vector search

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/vector/designate` | `graph:write` | Mark `(label, property)` as a vector field of fixed `dimension`. |
| `POST` | `/v1/tenants/{t}/graph/vector/search` | `graph:read` | k-NN over a designated vector property. |
| `POST` | `/v1/tenants/{t}/graph/vector/hybrid` | `graph:read` | Blend vector similarity with graph proximity. |

Vectors are L2-normalized on insert and compared by cosine distance (HNSW via `instant-distance`).

- **Designate** body: `{ "label", "property", "dimension" }`.
- **Search** body: `{ "query": [f32…], "k"?: 10, "label"?, "property" }`.
- **Hybrid** body adds: `graph_seeds[]`, `max_hops`, `alpha?` (vector↔graph blend, `0..1`),
  `confidence_weighted_graph_distance?`, `edge_type_weights?`.

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  http://localhost:8380/v1/tenants/acme/graph/vector/search \
  -d '{"property":"embedding","label":"Claim","k":5,"query":[0.12,0.04, ... ]}'
```

## Epistemic neighbors

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/epistemic-neighbors` | `graph:read` | Walk neighbors filtered by epistemic type, confidence, and depth. |

Body: `{ "node_id", "epistemic_types"?: ["supports", …], "min_confidence"?: 0.0, "max_depth"?: 1 }`.

## Graph algorithms

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/algorithms/ppr` | `graph:read` | Personalized PageRank (ACL local-push). |
| `POST` | `/v1/tenants/{t}/graph/algorithms/pagerank` | `graph:read` | Global PageRank. |
| `POST` | `/v1/tenants/{t}/graph/algorithms/components` | `graph:read` | Connected components. |
| `POST` | `/v1/tenants/{t}/graph/algorithms/communities` | `graph:read` | Label-propagation communities. |

- **PPR** body: `{ "seeds": {"node":mass,…}, "alpha"?: 0.15, "epsilon"?: 1e-4, "max_pushes"?: 200000, "top_k"? }`.
- **PageRank** body: `{ "damping"?: 0.85, "max_iter"?: 100, "tolerance"?: 1e-6, "top_k"? }`.
- **Components** body: `{ "directed"?: false }`.
- **Communities** body: `{}`.

## Full-text search (BM25)

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/fulltext/designate` | `graph:write` | Designate `(label, property)` for indexing. |
| `POST` | `/v1/tenants/{t}/graph/fulltext/search` | `graph:read` | BM25 search. Returns `[{node_id, score}]`. |

- **Designate** body: `{ "label", "property" }`.
- **Search** body: `{ "label"?, "property", "query", "k"?: 10 }`.

## Spatial (H3)

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/spatial/designate` | `graph:write` | Designate lat/lon properties at an H3 `resolution` (default `8`). |
| `POST` | `/v1/tenants/{t}/graph/spatial/radius` | `graph:read` | Nodes within `radius_km` of a point. |
| `POST` | `/v1/tenants/{t}/graph/spatial/bbox` | `graph:read` | Nodes within a bounding box. |

- **Designate** body: `{ "label", "lat_property", "lon_property", "resolution"?: 8 }`.
- **Radius** body: `{ "label", "lat_property", "lon_property", "lat", "lon", "radius_km" }`.
- **BBox** body: `{ "label", "lat_property", "lon_property", "min_lat", "min_lon", "max_lat", "max_lon" }`.

## Bulk ingest

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/bulk/nodes` | `graph:write` | Bulk node load. |
| `POST` | `/v1/tenants/{t}/graph/bulk/edges` | `graph:write` | Bulk edge load. |

## Versioned graph (Git-like)

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/graph/version/compile` | `graph:write` | Compile the tenant graph into a content-addressed pack + commit. |
| `POST` | `/v1/tenants/{t}/graph/version/diff` | `graph:read` | Diff two snapshots. |
| `POST` | `/v1/tenants/{t}/graph/version/ref` | `graph:write` | Update a branch/ref. |
| `POST` | `/v1/tenants/{t}/graph/version/log` | `graph:read` | Commit log for a ref. |
| `POST` | `/v1/tenants/{t}/graph/version/checkout` | `graph:read` | Resolve a ref/commit to a snapshot. |
| `POST` | `/v1/tenants/{t}/graph/version/merge` | `graph:write` | Three-way merge with conflict reporting. |

## Transactions

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/transactions/begin` | `graph:write` | Begin a transaction; returns a `tx_id`. |
| `POST` | `/v1/transactions/commit` | `graph:write` | Commit `{ "tx_id" }`. |
| `POST` | `/v1/transactions/rollback` | `graph:write` | Roll back `{ "tx_id" }`. |

Snapshot isolation by default; serializable under strict-ACID. A `tx_id` may be passed to `/v1/cypher`
to scope writes to an open transaction.

## Runs & context (agent state)

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/command` | varies | Execute a raw `RUSTYRED.*` command (see [Query surface](query-surface.md#command-vocabulary)). |
| `POST` | `/v1/tenants/{t}/batch` | varies | Execute a batch of commands. |
| `GET` | `/v1/tenants/{t}/runs/{run_id}` | `run:read` | Fetch an agent run's state. |
| `POST` | `/v1/tenants/{t}/context/pack` | `context:write` | Build a context-pack artifact. |
| `POST` | `/v1/command`, `/v1/batch` | varies | Default-tenant equivalents. |

## Graph-aware cache

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/cache/put` | `graph:write` | Store a cache entry. |
| `POST` | `/v1/cache/get` | `graph:read` | Fetch a cache entry. |
| `POST` | `/v1/cache/check` | `graph:read` | Check freshness without returning the value. |
| `POST` | `/v1/cache/explain` | `graph:read` | Explain a cache hit/miss/stale decision. |
| `POST` | `/v1/cache/invalidate` | `graph:write` | Invalidate entries. |
| `POST` | `/v1/cache/stats` | `graph:read` | Cache statistics. |

See [Query surface → graph cache](query-surface.md#graph-aware-cache) for entry kinds and the
staleness model.

## Instant-KG (Harness merged views)

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/tenants/{t}/instant-kg/status` | `graph:read` | Merged-view status for a repo/session. |
| `POST` | `/v1/tenants/{t}/instant-kg/ppr` | `graph:read` | Code PPR over the merged view. |
| `POST` | `/v1/tenants/{t}/instant-kg/impact` | `graph:read` | Impact analysis for changed objects. |
| `POST` | `/v1/tenants/{t}/instant-kg/related-objects` | `graph:read` | Related-object lookup. |
| `POST` | `/v1/tenants/{t}/instant-kg/search` | `graph:read` | Search the merged view. |
| `POST` | `/v1/tenants/{t}/instant-kg/explain-edge` | `graph:read` | Explain why an edge exists. |
