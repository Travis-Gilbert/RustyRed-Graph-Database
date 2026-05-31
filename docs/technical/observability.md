# Observability

## Health & readiness

| Endpoint | Auth | Meaning |
|----------|------|---------|
| `GET /health` | none | Liveness — process is up. Returns `{"status":"ok"}`. |
| `GET /ready` | none | Readiness — the tenant store opens cleanly. Returns `200` with a `ready` status, or a `503`-class body when the store is unavailable. This is the Railway healthcheck path. |

Use `/health` for liveness probes and `/ready` for readiness/traffic gating.

## Metrics

`GET /metrics` (scope `admin:read`) returns Prometheus text exposition
(`Content-Type: text/plain; version=0.0.4`). Counters use the `# HELP / # TYPE / name value` form.

Exposed series (`crates/rustyred-server/src/observability.rs`) include:

**Request & error counters**

- `rustyred_total_requests`
- `rustyred_errors`
- `rustyred_graph_mutations`

**Query counters & latency**

- `rustyred_cypher_queries`, `rustyred_cypher_latency_seconds*`
- `rustyred_query_latency*` (percentile summary)

**Algorithm counters & latency**

- `rustyred_ppr_calls`, `rustyred_pagerank_calls`, `rustyred_components_calls`,
  `rustyred_communities_calls`, `rustyred_algorithm_latency_seconds*`

**Search counters & latency**

- `rustyred_vector_search_calls`, `rustyred_vector_search_latency_seconds*`
- `rustyred_fulltext_search_calls`, `rustyred_fulltext_search_latency_seconds*`
- `rustyred_spatial_search_calls`

**Cache counters**

- `rustyred_cache_hits`, `rustyred_cache_misses`, `rustyred_cache_stale`

**Transaction counters**

- `rustyred_transactions_begun`, `rustyred_transactions_committed`,
  `rustyred_transactions_rolled_back`

```bash
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:8380/metrics
```

## Diagnostics

| Endpoint | Auth | Returns |
|----------|------|---------|
| `GET /v1/diagnostics/slow_queries` | `admin:read` | The slow-query ring buffer. |
| `GET /v1/diagnostics/config` | `admin:read` | Effective static configuration (storage mode, durability, quota support, …). |

**Slow queries.** Any operation slower than `RUSTY_RED_SLOW_QUERY_NANOS` (default 100 ms) is recorded
into a ring buffer of size `RUSTY_RED_SLOW_QUERY_CAPACITY` (default 128). Each entry carries
`recorded_at_unix_ms`, `nanos`, `kind`, `detail`, `nodes_visited`, and `edges_touched`. Set
`RUSTY_RED_SLOW_QUERY_LOG` to also emit them to the log.

```json
{ "entries": [
    { "recorded_at_unix_ms": "1717000000000", "nanos": 142000000,
      "kind": "cypher", "detail": "MATCH (c:Claim) …", "nodes_visited": 1200, "edges_touched": 3400 }
  ], "count": 1 }
```

## Logging

Logging uses `tracing` with `tracing-subscriber`, defaulting to the `info` filter. Override with the
standard `RUST_LOG` env var (e.g. `RUST_LOG=rustyred_server=debug,info`). On successful bind the
server logs `RUSTYRED_PRODUCT_READY <addr>`.
