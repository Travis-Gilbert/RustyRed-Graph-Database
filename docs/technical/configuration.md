# Configuration

All configuration is environment-driven and read once at startup by
`crates/rustyred-server/src/config.rs`. Invalid configuration fails fast: the server validates on
boot and refuses to start with a descriptive error. The annotated `.env.example` at the repo root is
the operator-facing companion to this reference.

Most variables have a primary `RUSTY_RED_*` name and an accepted legacy alias (`RUSTYRED_*` /
`RUSTYRED_PRODUCT_*`); the primary name is listed here. Booleans accept `1`/`true`/`yes`/`on`.

## Networking

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_HOST` | `127.0.0.1` (`0.0.0.0` when `PORT` is set) | Bind address. Use `[::]` for all interfaces. |
| `RUSTY_RED_PORT` | `8380` | Listen port. The platform `PORT` env var (Railway) takes precedence. |
| `RUSTY_RED_ALLOWED_ORIGINS` | `http://localhost:3000` | Comma-separated CORS / MCP origin allowlist. |
| `RUSTY_RED_PUBLIC_URL` | *(unset)* | Public base URL advertised in OpenAPI and `.well-known` manifests. |

HTTP and gRPC share this one port.

## Storage & durability

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_MODE` | `embedded` | `embedded` (RedCore native), `memory`, or `redis` (legacy). |
| `RUSTY_RED_DATA_DIR` | `data/rusty-red` (`/app/data/rusty-red` in container) | Data directory / volume mount. `RAILWAY_VOLUME_MOUNT_PATH` overrides. |
| `RUSTY_RED_REQUIRE_VOLUME` | `false` (`true` when `PORT` set) | Refuse to start without a persistent volume. |
| `RUSTY_RED_VOLUME_MOUNTED` | `false` | Manual attestation that a volume is mounted (non-Railway hosts). |
| `RUSTY_RED_DURABILITY` | `aof_everysec` | `aof_everysec`, `aof_always`, `snapshot_only`, or `none`. |
| `RUSTY_RED_SNAPSHOT_INTERVAL_WRITES` | `1000` | Mutations between full snapshots. |
| `RUSTY_RED_STRICT_ACID` | `false` | Enable strict ACID (see constraints below). |
| `RUSTY_RED_CONCURRENCY` | `single_writer` | Concurrency model. |
| `RUSTY_RED_TXN_ISOLATION` | `snapshot` (`serializable` if strict-ACID) | Transaction isolation. |

**Strict-ACID constraints.** `RUSTY_RED_STRICT_ACID=true` requires `RUSTY_RED_MODE=embedded`,
`RUSTY_RED_DURABILITY=aof_always`, `RUSTY_RED_CONCURRENCY=single_writer`, and
`RUSTY_RED_TXN_ISOLATION=serializable`; any mismatch fails validation.

## Tenancy & branding

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_KEY_PREFIX` | `rusty-red:tenant` | Per-tenant keyspace prefix. |
| `RUSTY_RED_SERVICE_NAME` | `rusty-red-graph-database` | Identifies the deployment in metadata. |
| `RUSTY_RED_API_TITLE` | `Rusty Red Graph Database API` | OpenAPI title. |
| `RUSTY_RED_TENANT_MEMORY_QUOTA_BYTES` | `0` (unlimited) | Per-tenant memory ceiling. Enforced for `embedded`/`memory` only; rejected for `redis`. |
| `RUSTY_RED_TENANT_CONFIG_JSON` | *(unset)* | Inline per-tenant overrides (JSON map). |
| `RUSTY_RED_TENANT_CONFIG_PATH` | *(unset)* | Path to a per-tenant overrides JSON file. |

Per-tenant overrides may set `durability`, `snapshot_interval_writes`, `strict_acid`,
`tenant_memory_quota_bytes`, and `hybrid_scoring` for individual tenants at startup.

## Authentication

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_REQUIRE_AUTH` | `true` | Refuse unauthenticated search, crawl, federation, `/v1/*`, `/mcp`, and metrics requests. |
| `RUSTY_RED_API_TOKENS` | *(empty)* | Comma-separated `secret=scope|scope|…` entries. |

**Token format.** Each entry is `<secret>=<scopes>` (a `:` also works as the separator); scopes are
separated by `|`, space, or `+`. A secret with no scopes is granted `*` (all). Example:

```
RUSTY_RED_API_TOKENS=abc123=graph:read|graph:write,readonly456=graph:read
```

Generate a secret with `openssl rand -hex 32`. Present it as `Authorization: Bearer <secret>`.

**Scopes.** Recognized scopes:

`run:write`, `run:read`, `context:write`, `context:read`, `graph:read`, `graph:write`,
`admin:read`, `federation:write`, and the MCP-oriented aliases `rustyred:graph:read`,
`rustyred:graph:query`, `rustyred:graph:context`,
`rustyred:graph:write:propose`, `rustyred:graph:write:apply`, `rustyred:graph:index:read`,
`rustyred:graph:admin:verify`, `rustyred:events:read`, `rustyred:federation:write`. `*` grants
everything.

Aliases collapse onto base scopes — e.g. `rustyred:graph:read`/`:query`/`:index:read` satisfy
`graph:read`; `rustyred:graph:write:propose`/`:apply` satisfy `graph:write`;
`rustyred:graph:admin:verify` satisfies `admin:read`. When `RUSTY_RED_REQUIRE_AUTH=false`, all
requests run with a dev identity holding every scope.

## Web Commons federation

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_FEDERATE` | `true` | Enables the RustyWeb Web Commons lane. Default crawls become federable unless a request scope opts out. |
| `RUSTY_RED_FEDERATE_HUB_URL` | *(unset)* | Hub base URL or full `/federate/submit` URL for outbound signed fragments. If unset, crawls stay local and report `hub_url_missing`. |
| `RUSTY_RED_FEDERATE_TOKEN` | *(unset)* | Bearer token for the hub request. The hub token needs `federation:write`. |
| `RUSTY_RED_FEDERATE_PRIVATE_KEY` | *(unset)* | 32-byte Ed25519 private key as hex. Required for outbound fragment signing. |
| `RUSTY_RED_FEDERATE_PEER_ID` | derived from private key | Optional Ed25519 public key hex. Startup accepts it, but submission rejects a mismatch. |
| `RUSTY_RED_FEDERATE_PROVENANCE` | `false` | Include crawl seeds, budget, and actor id in the fragment. Content and links federate without this. |
| `RUSTY_RED_FEDERATE_SNAPSHOT_TEXT_BYTES` | `4096` | Bounded text prefix per federated content snapshot. |

Inbound hub submissions use `POST /federate/submit` with scope `federation:write`. Signed
fragments are verified natively in Rust with Ed25519, gated through the Web Commons trust rules, and
merged into the tenant graph. Legacy receipt/hash-only submissions are still accepted as validated
no-ops for compatibility, but they do not merge remote state.

## MCP agent port

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_MCP_ENABLED` | `true` | Master switch for `/mcp` and the manifests. |
| `RUSTY_RED_MCP_READ_ONLY` | `true` | When true, only read tools are exposed. |
| `RUSTY_RED_MCP_ALLOW_ADMIN` | `false` | Expose the admin tool surface (`rustyred.admin.verify`). |
| `RUSTY_RED_MCP_DEFAULT_TENANT` | `default` | Tenant assumed when an MCP call omits one. |

## Observability

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_SLOW_QUERY_NANOS` | `100000000` (100 ms) | Slow-query threshold. |
| `RUSTY_RED_SLOW_QUERY_CAPACITY` | `128` | Ring-buffer size for `/v1/diagnostics/slow_queries` (must be > 0). |
| `RUSTY_RED_SLOW_QUERY_LOG` | *(unset)* | If set, also log slow queries. |
| `RUST_LOG` | `info` | `tracing-subscriber` env filter (standard Rust logging). |

## Index backends (build-feature gated)

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_FULLTEXT_BACKEND` | bundled BM25 | Set to `tantivy` only if the `tantivy` build feature is compiled in. |
| `RUSTY_RED_SPATIAL_BACKEND` | `h3` | Set to `s2` only if the `s2` build feature is compiled in. |

## Legacy redis mode

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUSTY_RED_REDIS_URL` | `redis://127.0.0.1:6379` | Only consulted when `RUSTY_RED_MODE=redis`. (`REDIS_URL` is also honored.) |

## Hybrid scoring defaults

Hybrid vector+graph scoring (`HybridScoringConfig`) defaults to `alpha = 0.5`,
`confidence_weighted_graph_distance = true`, and edge-type weights that penalize disagreement
(`contradicts = -1.0`, `tension = -0.5`). Override per request on the hybrid-search endpoint or
per tenant via the tenant-config overrides.
