"""Benchmarks for theseus_native.push_ppr.

Times the native vs Python paths on 50K, 200K, and 1M-node sparse
graphs. The 1M-node test enforces a >= 20x speedup floor (acceptance
criterion 7 in docs/plans/theseus-native-push-ppr/design-doc.md).

Smaller fixtures are not gated on a speedup ratio because their
absolute Python runtime is small enough that timer noise can dominate.
They print their numbers for reference only.

Skip semantics:
- If theseus_native is not importable, the entire module skips.
- If apps.notebook.sparse_ppr is not importable, the entire module skips.
- The 1M-node benchmark takes 30-90s on Python and 1-3s native; mark
  it with `@pytest.mark.slow` if pytest is configured to filter slow
  tests, otherwise it always runs.
"""

from __future__ import annotations

import os
import random
import sys
import time
from typing import Dict, List, Tuple
from unittest.mock import patch

import pytest

theseus_native = pytest.importorskip("theseus_native")

try:
    _REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    if _REPO_ROOT not in sys.path:
        sys.path.insert(0, _REPO_ROOT)
    from apps.notebook import sparse_ppr  # type: ignore[import-not-found]
except Exception:  # pragma: no cover
    sparse_ppr = None  # type: ignore[assignment]
    pytestmark = pytest.mark.skip(reason="apps.notebook.sparse_ppr not importable")


Adjacency = Dict[int, List[Tuple[int, float]]]


def _build_random_sparse_graph(num_nodes: int, avg_degree: float, seed: int) -> Adjacency:
    """Generate a sparse undirected weighted graph.

    Node IDs are non-contiguous (multiplied by a prime) to mimic Theseus
    PKs and ensure dict-keyed access patterns are realistic.
    """
    rng = random.Random(seed)
    node_ids = [(i + 1) * 31 for i in range(num_nodes)]
    adj: Adjacency = {nid: [] for nid in node_ids}
    target_edges = int(num_nodes * avg_degree // 2)
    for _ in range(target_edges):
        u, v = rng.sample(node_ids, 2)
        w = rng.uniform(0.1, 1.0)
        adj[u].append((v, w))
        adj[v].append((u, w))
    return adj


def _time_call(adj: Adjacency, seeds: Dict[int, float], disable_native: bool) -> float:
    """Run push_ppr once under the requested code path; return wall seconds."""
    env = {k: v for k, v in os.environ.items() if k != "THESEUS_DISABLE_NATIVE"}
    if disable_native:
        env["THESEUS_DISABLE_NATIVE"] = "1"
    with patch.dict(os.environ, env, clear=True):
        t0 = time.perf_counter()
        _ = sparse_ppr.push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)
        return time.perf_counter() - t0


@pytest.mark.parametrize(
    "num_nodes,avg_degree,seed_pk_index",
    [
        (50_000, 4.0, 100),
        (200_000, 4.0, 100),
    ],
)
def test_smaller_fixtures_print_numbers(num_nodes: int, avg_degree: float, seed_pk_index: int) -> None:
    """Reference benchmarks; no speedup gate. Prints native/python/ratio."""
    adj = _build_random_sparse_graph(num_nodes, avg_degree, seed=42)
    node_ids = list(adj.keys())
    seeds = {node_ids[seed_pk_index]: 1.0}

    t_python = _time_call(adj, seeds, disable_native=True)
    t_native = _time_call(adj, seeds, disable_native=False)
    ratio = t_python / max(t_native, 1e-9)
    print(
        f"\n[{num_nodes}-node, avg_deg={avg_degree}] "
        f"native={t_native:.4f}s python={t_python:.4f}s speedup={ratio:.1f}x"
    )
    # Sanity-only assertion: native must not be slower than Python.
    assert t_native <= t_python * 1.5, (
        f"native unexpectedly slower than python at {num_nodes} nodes: "
        f"native={t_native:.4f}s python={t_python:.4f}s"
    )


def test_speedup_floor_at_1m_nodes() -> None:
    """Acceptance criterion 7: native >= 20x faster than Python at 1M nodes."""
    adj = _build_random_sparse_graph(1_000_000, avg_degree=4.0, seed=42)
    node_ids = list(adj.keys())
    seeds = {node_ids[100]: 1.0}

    # Single warmup of each path to amortize allocator + JIT cache effects.
    _ = _time_call(adj, seeds, disable_native=True)
    _ = _time_call(adj, seeds, disable_native=False)

    # Best-of-3 to reduce timer noise on shared CI runners.
    t_python = min(_time_call(adj, seeds, disable_native=True) for _ in range(3))
    t_native = min(_time_call(adj, seeds, disable_native=False) for _ in range(3))

    ratio = t_python / max(t_native, 1e-9)
    print(
        f"\n[1M-node, avg_deg=4.0] "
        f"native={t_native:.4f}s python={t_python:.4f}s speedup={ratio:.1f}x"
    )
    assert ratio >= 20.0, (
        f"native speedup at 1M nodes was {ratio:.1f}x; floor is 20x. "
        f"native={t_native:.4f}s python={t_python:.4f}s"
    )
