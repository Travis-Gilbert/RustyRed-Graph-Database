"""Property-based parity tests for rusty_red_native.push_ppr."""

from __future__ import annotations

import random
from typing import Dict, List, Tuple

import pytest
from ppr_reference import python_push_ppr

hypothesis = pytest.importorskip("hypothesis")
from hypothesis import HealthCheck, given, settings, strategies as st  # noqa: E402

rusty_red_native = pytest.importorskip("rusty_red_native")

Adjacency = Dict[int, List[Tuple[int, float]]]
Seeds = Dict[int, float]


def _build_random_sparse_graph(num_nodes: int, avg_degree: float, rng: random.Random) -> Adjacency:
    """Generate a sparse undirected weighted graph with arbitrary integer IDs."""
    node_ids = [(i + 1) * 31 for i in range(num_nodes)]
    adj: Adjacency = {node_id: [] for node_id in node_ids}
    target_edges = int(num_nodes * avg_degree // 2)
    for _ in range(target_edges):
        u, v = rng.sample(node_ids, 2)
        weight = rng.uniform(0.1, 1.0)
        adj[u].append((v, weight))
        adj[v].append((u, weight))
    return adj


def _build_seeds(node_ids: List[int], num_seeds: int, rng: random.Random) -> Seeds:
    chosen = rng.sample(node_ids, min(num_seeds, len(node_ids)))
    weight = 1.0 / len(chosen)
    return {node_id: weight for node_id in chosen}


def _assert_dicts_close(
    got: Dict[int, float],
    expected: Dict[int, float],
    rel_tol: float,
    abs_tol: float,
) -> None:
    keys = set(got) | set(expected)
    diffs: List[Tuple[int, float, float, float]] = []
    for key in keys:
        a = got.get(key, 0.0)
        b = expected.get(key, 0.0)
        denom = max(abs(b), abs(a), abs_tol)
        rel = abs(a - b) / denom
        if rel > rel_tol:
            diffs.append((key, a, b, rel))
    assert not diffs, (
        f"native vs python disagree on {len(diffs)} nodes "
        f"(showing up to 10): {diffs[:10]}"
    )


@settings(
    deadline=None,
    max_examples=100,
    suppress_health_check=[HealthCheck.too_slow, HealthCheck.data_too_large],
)
@given(
    seed=st.integers(min_value=0, max_value=2**32 - 1),
    num_nodes=st.sampled_from([1_000, 10_000]),
    avg_degree=st.sampled_from([2.0, 4.0, 8.0]),
    num_seeds=st.integers(min_value=1, max_value=10),
    alpha=st.sampled_from([0.05, 0.15, 0.30]),
    epsilon=st.sampled_from([1e-3, 1e-4]),
)
def test_native_matches_python_within_tolerance(
    seed: int,
    num_nodes: int,
    avg_degree: float,
    num_seeds: int,
    alpha: float,
    epsilon: float,
) -> None:
    rng = random.Random(seed)
    adj = _build_random_sparse_graph(num_nodes, avg_degree, rng)
    seeds = _build_seeds(list(adj.keys()), num_seeds, rng)

    native = rusty_red_native.push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)
    python = python_push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)

    _assert_dicts_close(native, python, rel_tol=1e-5, abs_tol=1e-9)


@settings(
    deadline=None,
    max_examples=5,
    suppress_health_check=[HealthCheck.too_slow, HealthCheck.data_too_large],
)
@given(
    seed=st.integers(min_value=0, max_value=2**32 - 1),
    avg_degree=st.sampled_from([4.0]),
    num_seeds=st.integers(min_value=1, max_value=5),
    alpha=st.sampled_from([0.15]),
    epsilon=st.sampled_from([1e-4]),
)
def test_native_matches_python_at_100k_nodes(
    seed: int,
    avg_degree: float,
    num_seeds: int,
    alpha: float,
    epsilon: float,
) -> None:
    rng = random.Random(seed)
    adj = _build_random_sparse_graph(100_000, avg_degree, rng)
    seeds = _build_seeds(list(adj.keys()), num_seeds, rng)

    native = rusty_red_native.push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)
    python = python_push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)

    _assert_dicts_close(native, python, rel_tol=1e-5, abs_tol=1e-9)


def _build_two_community_adjacency() -> Adjacency:
    adj: Adjacency = {node: [] for node in range(10)}
    edges: List[Tuple[int, int, float]] = []
    for i in range(5):
        for j in range(i + 1, 5):
            edges.append((i, j, 1.0))
    for i in range(5, 10):
        for j in range(i + 1, 10):
            edges.append((i, j, 1.0))
    edges.append((4, 5, 0.3))
    for u, v, weight in edges:
        adj[u].append((v, weight))
        adj[v].append((u, weight))
    return adj


def test_two_community_fixture_parity() -> None:
    adj = _build_two_community_adjacency()
    seeds = {0: 1.0}

    native = rusty_red_native.push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)
    python = python_push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)

    _assert_dicts_close(native, python, rel_tol=1e-5, abs_tol=1e-9)

    top5_native = {node_id for node_id, _ in sorted(native.items(), key=lambda x: -x[1])[:5]}
    assert top5_native == {0, 1, 2, 3, 4}
