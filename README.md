# theseus_native

Rust + PyO3 accelerators for Theseus retrieval. Currently exports one function: `push_ppr`.

## Build (local development)

Requires Rust 1.78+ and `maturin >= 1.7`.

```bash
python3 -m pip install --user maturin
cd theseus_native
maturin develop --release
```

This builds an `abi3-py312` wheel and installs it into the active Python environment. After this, `from theseus_native import push_ppr` works in any Python 3.12+ interpreter that shares the venv.

## Build (release wheels)

CI builds Linux x86_64 manylinux2014 wheels via `.github/workflows/build_native_wheels.yml`. macOS arm64 is built locally for now (Travis's M1); CI build for Darwin is out of scope for the first cut.

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

Matches `apps/notebook/sparse_ppr.py:push_ppr` exactly. ACL local-push personalized PageRank: alpha is the restart probability (Theseus convention), epsilon is the per-node convergence threshold, max_pushes caps total iterations to prevent pathological walks.

## Fallback semantics

`apps/notebook/sparse_ppr.py` is the dispatcher. It tries `from theseus_native import push_ppr` first; on ImportError, or when `THESEUS_DISABLE_NATIVE=1` is set in the environment at call time, it routes to the pure-Python `_python_push_ppr` defined in the same file. The fallback exists indefinitely (per ADR 0001 follow-up) so dev environments without the wheel still function.

The wrapper logs once at WARNING level on the first import that finds the wheel missing: `theseus_native unavailable, using Python push_ppr`. Subsequent imports do not re-log.

## Algorithm reference

Andersen, R., Chung, F., and Lang, K. (2006). Local Graph Partitioning using PageRank Vectors. FOCS 2006.

## License

MIT.
