"""Benchmarks for rusty_red_native.push_ppr."""

from __future__ import annotations

import random
import time
from typing import Dict, List, Tuple

import pytest
from ppr_reference import python_push_ppr

rusty_red_native = pytest.importorskip("rusty_red_native")

Adjacency = Dict[int, List[Tuple[int, float]]]


def _build_random_sparse_graph(num_nodes: int, avg_degree: float, seed: int) -> Adjacency:
    """Generate a sparse undirected weighted graph with arbitrary integer IDs."""
    rng = random.Random(seed)
    node_ids = [(i + 1) * 31 for i in range(num_nodes)]
    adj: Adjacency = {node_id: [] for node_id in node_ids}
    target_edges = int(num_nodes * avg_degree // 2)
    for _ in range(target_edges):
        u, v = rng.sample(node_ids, 2)
        weight = rng.uniform(0.1, 1.0)
        adj[u].append((v, weight))
        adj[v].append((u, weight))
    return adj


def _time_call(adj: Adjacency, seeds: Dict[int, float], use_python: bool) -> float:
    t0 = time.perf_counter()
    if use_python:
        _ = python_push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)
    else:
        _ = rusty_red_native.push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)
    return time.perf_counter() - t0


@pytest.mark.parametrize(
    "num_nodes,avg_degree,seed_pk_index",
    [
        (50_000, 4.0, 100),
        (200_000, 4.0, 100),
    ],
)
def test_smaller_fixtures_print_numbers(
    num_nodes: int,
    avg_degree: float,
    seed_pk_index: int,
) -> None:
    adj = _build_random_sparse_graph(num_nodes, avg_degree, seed=42)
    node_ids = list(adj.keys())
    seeds = {node_ids[seed_pk_index]: 1.0}

    t_python = _time_call(adj, seeds, use_python=True)
    t_native = _time_call(adj, seeds, use_python=False)
    ratio = t_python / max(t_native, 1e-9)
    print(
        f"\n[{num_nodes}-node, avg_deg={avg_degree}] "
        f"native={t_native:.4f}s python={t_python:.4f}s speedup={ratio:.1f}x"
    )
    assert t_native <= t_python * 1.5, (
        f"native unexpectedly slower than python at {num_nodes} nodes: "
        f"native={t_native:.4f}s python={t_python:.4f}s"
    )


def test_speedup_floor_at_1m_nodes() -> None:
    adj = _build_random_sparse_graph(1_000_000, avg_degree=4.0, seed=42)
    node_ids = list(adj.keys())
    seeds = {node_ids[100]: 1.0}

    _ = _time_call(adj, seeds, use_python=True)
    _ = _time_call(adj, seeds, use_python=False)

    t_python = min(_time_call(adj, seeds, use_python=True) for _ in range(3))
    t_native = min(_time_call(adj, seeds, use_python=False) for _ in range(3))

    ratio = t_python / max(t_native, 1e-9)
    print(
        f"\n[1M-node, avg_deg=4.0] "
        f"native={t_native:.4f}s python={t_python:.4f}s speedup={ratio:.1f}x"
    )
    assert ratio >= 20.0, (
        f"native speedup at 1M nodes was {ratio:.1f}x; floor is 20x. "
        f"native={t_native:.4f}s python={t_python:.4f}s"
    )
