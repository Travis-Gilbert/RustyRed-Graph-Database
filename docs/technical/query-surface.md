# Query surface

RustyRed exposes three ways to read and mutate the graph: a **Cypher subset**, a low-level
**command vocabulary**, and the **graph-aware cache**. Structured neighbor/match queries are also
available through the MCP `rustyred.graph.query` tool (see [MCP](mcp.md)).

## Cypher subset

`POST /v1/cypher` (and `/v1/tenants/{t}/graph/query`) parse a deliberately bounded Cypher grammar
(`crates/rustyred-server/src/cypher/grammar.pest`). Result sets default to **100** rows and are
capped at **1000**. Unsupported syntax returns `400 unsupported_cypher_feature`.

Request body:

```json
{ "tenant_id": "acme", "query": "MATCH (c:Claim) RETURN c LIMIT 10", "params": {}, "tx_id": null }
```

### Read queries

A read query is `MATCH … [WHERE …] [WITH …] RETURN … [ORDER BY …] [SKIP n] [LIMIT n]`.

**Patterns**

- Single node: `(c:Claim {status: 'open'})`
- Outgoing single hop: `(a)-[:SUPPORTS]->(b)`
- Outgoing multi-hop chain: `(a)-[:SUPPORTS]->(b)-[:CITES]->(c)`
- Variable-length expand: `(a)-[:SUPPORTS*1..3]->(b)`
- Path alias: `MATCH p = (a)-[:SUPPORTS]->(b)`

> Edge traversal is **outgoing only** (`-[:TYPE]->`). Incoming-direction chain syntax is not parsed.

**WHERE** supports a single equality predicate on a property path: `WHERE c.status = 'open'`.
Compound boolean predicates (`AND`/`OR`), inequalities, and ranges are **not** supported.

**RETURN / WITH** items may be bindings (`c`), property paths (`c.status`), or aggregates —
`COUNT`, `SUM`, `AVG`, `MIN`, `MAX` (e.g. `COUNT(*)`, `COUNT(c)`, `SUM(c.weight)`). `WITH … AS …`
projects intermediate values; `ORDER BY … ASC|DESC`, `SKIP n`, and `LIMIT n` apply afterward.

**Values & parameters** — strings (`'…'`/`"…"`), numbers, booleans, `null`, and `$param`
placeholders bound from the `params` object. A missing parameter returns `400 missing_cypher_param`.

### Write queries

Writes run inside a transaction (pass `tx_id`, or they auto-commit). Supported shapes:

- `CREATE (n:Label {props})` and `CREATE (a)-[:TYPE]->(b)`
- `MERGE (n:Label {props})` with optional `ON CREATE SET …` / `ON MATCH SET …` branches
- `MATCH … [WHERE …] SET a.prop = value` (and `SET a.prop = a.prop + value` increment)
- `MATCH … [WHERE …] DELETE n`
- `MATCH … [WHERE …] DETACH DELETE n`

### Explain

`POST /v1/cypher/explain` parses and plans a statement and returns the plan **without executing**.
Useful for validating syntax and inspecting the bounded plan.

## Structured neighbor / match (MCP tool)

The MCP `rustyred.graph.query` tool runs a bounded query with `operation`:

- `neighbors` — adjacency expansion from `node_id` in a `direction`, optionally filtered by
  `edge_type`, with a `budget`.
- `node_match` — exact scalar match by `label` and/or `properties`.

`rustyred.graph.explain` returns the corresponding plan. See [MCP](mcp.md).

## Command vocabulary

`POST /v1/command` (and `/v1/tenants/{t}/command`, or a batch via `/v1/batch`) executes a raw
command. Body: `{ "command": "RUSTYRED.…", "args": { … } }`. The response envelope is
`{ ok, command, status, payload, nodes[], edges[], events[], state_hash, error? }`.

Recognized commands (`crates/rustyred-core/src/commands.rs`):

| Command | Purpose |
|---------|---------|
| `RUSTYRED.RUN.BEGIN` / `RUSTYRED.RUN.STEP` / `RUSTYRED.RUN.GET` | Agent-run lifecycle and step recording. |
| `RUSTYRED.TOOL.SELECT` | Tool-selection step. |
| `RUSTYRED.CONTEXT.PACK` / `RUSTYRED.CONTEXT.GET` | Build / read a context-pack artifact. |
| `RUSTYRED.PATCH.PROPOSE` / `RUSTYRED.PATCH.VALIDATE` / `RUSTYRED.PATCH.COMMIT` | Memory-patch propose/validate/commit. |
| `RUSTYRED.STATE.HASH` | Return the deterministic state hash. |
| `RUSTYRED.DEBUG.CYPHER` (alias `RUSTYRED.CYPHER`) | Debug-execute a Cypher statement. |
| `RUSTYRED.GRAPH.NODE.UPSERT` / `RUSTYRED.GRAPH.EDGE.UPSERT` | Upsert a node / edge. |
| `RUSTYRED.GRAPH.NODES.QUERY` | Query nodes. |
| `RUSTYRED.GRAPH.NEIGHBORS` | Neighbor expansion. |
| `RUSTYRED.GRAPH.STATS` | Graph statistics. |
| `RUSTYRED.GRAPH.VERIFY` | Integrity verify. |
| `RUSTYRED.GRAPH.REBUILD_INDEXES` (alias `RUSTYRED.GRAPH.REBUILD`) | Rebuild derived indexes. |

Command names are case-insensitive and trimmed.

## Graph-aware cache

`POST /v1/cache/{put,get,check,explain,invalidate,stats}` implement a cache that understands graph
state, so entries go **stale automatically** when the underlying graph mutates
(`crates/rustyred-server/src/graph_cache.rs`).

**Entry kinds** (the `kind` field): `query_result`, `query_plan`, `bounded_subgraph`,
`neighbor_expansion`, `context_pack`, `retrieval_plan`, `semantic_answer_candidate`,
`modal_parse_result`, `vector_search_result`, `epistemic_traversal`.

**Keying & invalidation.** A lookup/put carries a `kind`, a `key`, and optional fingerprints that
participate in staleness: `index_manifest_hash`, `auth_scope_hash`, `retrieval_policy_hash`,
`model_version`, and `source_hashes[]`. `check` reports freshness without returning the value;
`explain` returns the hit/miss/stale reasoning.
