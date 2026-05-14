# theseus_native / Rusty Red Graph Database

Rust + PyO3 accelerators for Theseus retrieval, plus the Rusty Red Graph
Database runtime. `push_ppr` remains the retrieval accelerator; `thg-core` is
the shared Database-as-Harness command executor and graph-store core;
`thg-product-server` is the Railway/product HTTP server for Rusty Red; and
`thg-mcp` is the first-class MCP agent port over the graph APIs.

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

## Rusty Red Graph Database product server

Rusty Red is the productized THG runtime profile: it keeps the THG command model
for existing harness flows while adding first-class graph node, edge, adjacency,
exact scalar property index, stats, verify, and MCP routes. The code should stay
shared through `thg-core` rather than being copied into a second implementation:
Context Theorem can use the same graph-store components internally, while Rusty
Red packages them as a standalone database service. It is not a raw Redis
protocol, RedisGraph compatibility layer, FalkorDB replacement, or complete
OpenCypher/GQL engine yet. Redis-compatible storage is the current durable
backing store; Rusty Red owns the graph API and index semantics above that
store.

Run the product server locally:

```bash
cd theseus_native
RUSTY_RED_REDIS_URL=redis://127.0.0.1:6379 cargo run -p thg-product-server
```

Core routes:

```text
GET  /health
GET  /ready
GET  /openapi.json
GET  /.well-known/mcp/thg.json
POST /mcp
POST /v1/tenants/{tenant_id}/command
POST /v1/tenants/{tenant_id}/batch
GET  /v1/tenants/{tenant_id}/runs/{run_id}
POST /v1/tenants/{tenant_id}/graph/nodes
POST /v1/tenants/{tenant_id}/graph/nodes/query
GET  /v1/tenants/{tenant_id}/graph/nodes/{node_id}
POST /v1/tenants/{tenant_id}/graph/edges
GET  /v1/tenants/{tenant_id}/graph/edges/{edge_id}
POST /v1/tenants/{tenant_id}/graph/neighbors
GET  /v1/tenants/{tenant_id}/graph/stats
GET  /v1/tenants/{tenant_id}/graph/verify
POST /v1/tenants/{tenant_id}/context/pack
```

The OpenAPI document is served at `/openapi.json`. It exists because Rusty Red
is exposed through HTTP and MCP even though the underlying storage engine is a
database-style service. The OpenAPI contract is for the HTTP API; MCP tool,
resource, and prompt metadata are discovered through the MCP endpoint and
well-known manifests.

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
