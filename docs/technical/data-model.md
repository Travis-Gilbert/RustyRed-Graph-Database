# Data model

The canonical types live in `crates/rustyred-core/src/graph_store.rs`. The graph is a directed
property graph with first-class *epistemic* semantics on edges.

## Nodes

`NodeRecord`:

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Caller-supplied unique id. |
| `labels` | string[] | Zero or more labels; normalized on write. Backs the label index. |
| `properties` | JSON object | Arbitrary JSON. Scalar properties back the property index. |
| `version` | uint | Monotonic per-record version, bumped on each upsert. |
| `tombstone` | bool | Soft-delete flag. Tombstoned nodes are retained but excluded from queries. |
| `content_hash` | string? | Content address (see below). |
| `parent_hashes` | string[] | Prior content hashes, for version lineage. |

## Edges

`EdgeRecord`:

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Caller-supplied unique id. |
| `from_id` / `to_id` | string | Endpoint node ids. Both must exist and be live at write time. |
| `type` | string | Edge type (relationship name). Backs the edge-type index and adjacency. |
| `properties` | JSON object | Arbitrary JSON. |
| `version` | uint | Monotonic per-record version. |
| `tombstone` | bool | Soft-delete flag. |
| `confidence` | float? | Optional, clamped to `[0,1]`. Defaults to `1.0` when absent. |
| `epistemic_type` | enum? | One of the epistemic types below. |
| `provenance` | object? | `{ source_id?, timestamp?, method? }`. |
| `content_hash` | string? | Content address. |
| `parent_hashes` | string[] | Version lineage. |

### Epistemic edge types

`EpistemicType` (`supports`, `contradicts`, `tension`, `derives`, `cites`) makes the graph aware of
how claims relate. It drives two behaviours:

- **Epistemic neighbor traversal** — filter and walk neighbors by epistemic type, minimum
  confidence, and hop depth (`/v1/tenants/{t}/graph/epistemic-neighbors`).
- **Hybrid scoring** — `contradicts` and `tension` edges carry negative default weights
  (`-1.0` and `-0.5`), so contradictory paths *reduce* graph proximity rather than increasing it.

## Multi-tenancy

Every persistent surface is namespaced by `tenant_id`. Stored data is segregated under
`<RUSTY_RED_KEY_PREFIX>:<tenant_id>:…` (default prefix `rusty-red:tenant`). Tenant ids are sanitized
into safe key/path segments. HTTP routes carry the tenant explicitly
(`/v1/tenants/{tenant_id}/…`); root convenience routes and MCP calls fall back to a default tenant
(`RUSTY_RED_MCP_DEFAULT_TENANT`, literal `default` if unset). Per-tenant runtime overrides
(durability, snapshot interval, strict-ACID, memory quota, hybrid scoring) can be supplied at
startup via `RUSTY_RED_TENANT_CONFIG_JSON` / `RUSTY_RED_TENANT_CONFIG_PATH`.

## Content addressing & versioning

Each record can compute a stable **content address**: a `sha256:` hash over its identifying fields
(id, labels/type, properties, tombstone, and — for edges — confidence, epistemic type, and
provenance). Content addressing underpins:

- **Integrity** — `checksum()` is returned on every write (`GraphWriteResult { id, version, checksum }`).
- **Versioned graph packs** — `snapshot_content_objects`, `build_prolly_tree`, and
  `compile_graph_pack` turn a snapshot into a content-addressed Prolly-style tree plus a Git-like
  commit (`GraphCommit`) with author/message/parents.
- **Refs, diff, merge** — branches default to `main` (`DEFAULT_GRAPH_BRANCH`); `diff_graph_snapshots`
  produces `GraphDiffEntry` lists; `merge_graph_snapshots` performs three-way merges with conflict
  reporting and selectable strategies.

The whole graph also has a single monotonic `version` (in `GraphStats.version`) that increments with
each mutation; the cache and verify subsystems use it to detect staleness.

## Indexes derived from the data

| Index | Keyed by | Used for |
|-------|----------|----------|
| Label | label → node ids | `NodeQuery` by label |
| Edge type | type → edge ids | edge-type filters |
| Property | `(property, value)` → node ids | exact property match |
| Adjacency (out/in) | `(node_id, edge_type)` → edge ids | neighbor expansion |
| Vector | `(label, property)` → HNSW index | vector / hybrid search |
| Full-text | `(label, property)` → BM25 index | full-text search |
| Spatial | `(label, lat_prop, lon_prop, resolution)` → H3 cells | radius / bbox queries |

## Stats & integrity

- `GraphStats` reports `version`, totals for nodes/edges/labels/edge-types/property-keys/property
  indexes, and estimated `memory_bytes` against `memory_quota_bytes`.
- `verify()` returns a `VerifyReport { ok, stats, problems[] }`; `rebuild_indexes()` repairs derived
  indexes and returns before/after verify reports. Exposed at `…/graph/verify` and
  `…/graph/rebuild-indexes`.
