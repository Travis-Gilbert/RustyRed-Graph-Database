# MCP agent port

RustyRed ships a first-class **Model Context Protocol** server so agents can read and (optionally)
write the graph through standard MCP tooling. It is implemented in `crates/rustyred-mcp` and surfaced
by `rustyred-server` at:

- `POST /mcp` — JSON-RPC 2.0 endpoint.
- `GET /.well-known/mcp/rustyred.json` — MCP server manifest.
- `GET /.well-known/agent.json` — agent-port manifest.

All three return `404` when `RUSTY_RED_MCP_ENABLED=false`.

## Transport & gating

`/mcp` is a JSON-RPC 2.0 POST endpoint. Before dispatch the server:

1. Checks the request `Origin` against `RUSTY_RED_ALLOWED_ORIGINS` (else `403`).
2. Requires a bearer token with at least `graph:read` when auth is enabled (else `401`/`403`).

**Read-only by default.** With `RUSTY_RED_MCP_READ_ONLY=true` (default), only read tools are listed
and callable; write tools (`*.designate`, `*.bulk.*`, `vector.designate`) return `mcp_read_only`.
Set it `false` to expose write tools — the caller's token still needs `graph:write`. The admin tool
(`rustyred.admin.verify`) is additionally gated by `RUSTY_RED_MCP_ALLOW_ADMIN=true` **and** an
`admin:read` scope.

## JSON-RPC methods

| Method | Result |
|--------|--------|
| `initialize` | Server info, protocol version, and capabilities (`readOnly`, `allowAdmin`). |
| `ping` | `{}` |
| `tools/list` | Tool definitions (filtered by read-only/admin mode). |
| `tools/call` | Invoke a tool by `name` with `arguments`. |
| `resources/list` | Available resources. |
| `resources/templates/list` | URI templates for parameterized resources. |
| `resources/read` | Read a resource by URI. |

```json
{ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
  "params": { "name": "rustyred.graph.neighbors",
              "arguments": { "tenant": "acme", "node_id": "claim:1", "direction": "out" } } }
```

Calls that omit `tenant` use `RUSTY_RED_MCP_DEFAULT_TENANT` (literal `default` if unset).

## Tool catalog

Read tools are always available; **[write]** tools require read-write mode + `graph:write`;
**[admin]** requires admin mode + `admin:read`. Several tools accept aliases (shown in parentheses).

### Graph

| Tool | Description |
|------|-------------|
| `rustyred.graph.neighbors` | Neighbor expansion via adjacency indexes. |
| `rustyred.graph.query` | Bounded query: `operation` = `neighbors` or `node_match`. |
| `rustyred.graph.explain` | Explain the bounded query plan without executing. |
| `rustyred.graph.schema` | Labels, edge types, stats, and capability notes. |
| `rustyred.graph.index_status` | Index health and verify drift. |

### Versioning (Git-like)

| Tool (aliases) | Description |
|----------------|-------------|
| `rustyred.graph.version.compile` (`rustyred.git.compile`) | Compile the tenant graph into a content-addressed Prolly-tree pack + commit. |
| `rustyred.graph.version.diff` (`rustyred.git.diff`) | Diff snapshots. |
| `rustyred.graph.version.ref` (`rustyred.git.ref`) | Update a ref/branch. |
| `rustyred.graph.version.log` (`rustyred.git.log`) | Commit log. |
| `rustyred.graph.version.checkout` (`rustyred.git.checkout`) | Resolve a ref/commit to a snapshot. |
| `rustyred.graph.version.merge` (`rustyred.git.merge`) | Three-way merge. |

### Algorithms

| Tool (aliases) | Description |
|----------------|-------------|
| `rustyred.algorithm.ppr` (`rustyred.algo.ppr`) | Personalized PageRank. |
| `rustyred.algorithm.components` (`rustyred.algo.components`) | Connected components. |
| `rustyred.algorithm.pagerank` (`rustyred.algo.pagerank`) | PageRank. |
| `rustyred.algorithm.communities` (`rustyred.algo.communities`) | Communities. |

### Search & geometry

| Tool (aliases) | Description |
|----------------|-------------|
| `rustyred.vector.search` | Vector k-NN. |
| `rustyred.vector.hybrid` | Hybrid vector + graph scoring. |
| `rustyred.vector.designate` **[write]** | Designate a vector property. |
| `rustyred.fulltext.search` (`rustyred.graph.fulltext.search`) | BM25 search. |
| `rustyred.fulltext.designate` (`rustyred.graph.fulltext.designate`) **[write]** | Designate a full-text property. |
| `rustyred.spatial.radius` (`rustyred.graph.spatial.radius`) | Radius query. |
| `rustyred.spatial.bbox` (`rustyred.graph.spatial.bbox`) | Bounding-box query. |
| `rustyred.spatial.designate` (`rustyred.graph.spatial.designate`) **[write]** | Designate lat/lon properties. |
| `rustyred.epistemic.neighbors` | Epistemic-typed neighbor traversal. |

### Bulk & admin

| Tool (aliases) | Description |
|----------------|-------------|
| `rustyred.bulk.nodes` (`rustyred.graph.bulk.nodes`) **[write]** | Bulk node load. |
| `rustyred.bulk.edges` (`rustyred.graph.bulk.edges`) **[write]** | Bulk edge load. |
| `rustyred.admin.verify` **[admin]** | Integrity verify. |

### Instant-KG (Harness merged views)

| Tool (aliases) | Description |
|----------------|-------------|
| `rustyred.instant_kg.status` (`harness_kg_status`) | Merged-view status. |
| `rustyred.instant_kg.ppr` (`harness_kg_ppr`) | Code PPR. |
| `rustyred.instant_kg.impact` (`harness_kg_impact`) | Impact analysis. |
| `rustyred.instant_kg.related_objects` (`harness_kg_related_objects`) | Related objects. |
| `rustyred.instant_kg.search` (`harness_kg_search`) | Search the merged view. |
| `rustyred.instant_kg.explain_edge` (`harness_kg_explain_edge`) | Explain an edge. |

## Resources

| Resource / template | URI |
|---------------------|-----|
| Latest verify report | `rustyred://tenant/{tenant}/verify/latest` |
| Node (template) | `rustyred://tenant/{tenant}/node/{node_id}` |
| Edge (template) | `rustyred://tenant/{tenant}/edge/{edge_id}` |
| Neighbors (template) | `rustyred://tenant/{tenant}/neighbors/{node_id}` |

All resources are `application/json`.
