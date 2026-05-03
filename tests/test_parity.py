"""Property-based parity test for theseus_native.push_ppr.

For every generated (adjacency, seeds, alpha, epsilon) input, the native
and Python implementations must agree within 1e-5 relative tolerance per
node. Acceptance criterion 6 in docs/plans/theseus-native-push-ppr/
design-doc.md.

The test toggles between code paths by setting THESEUS_DISABLE_NATIVE in
the environment. Reading is at call time (per Stage 2 dispatcher), so
no re-import is needed between toggles. Both code paths are exercised
inside the same hypothesis example.

Skip semantics:
- If theseus_native is not importable, the entire module skips.
- If apps.notebook.sparse_ppr is not importable (e.g. the test runs
  outside the Django repo), the entire module skips.
"""

from __future__ import annotations

import os
import random
import sys
from typing import Dict, List, Tuple
from unittest.mock import patch

import pytest

hypothesis = pytest.importorskip("hypothesis")
from hypothesis import HealthCheck, given, settings, strategies as st  # noqa: E402

# Skip the whole module if either the native wheel or the dispatcher is
# not importable. This keeps the file safe to run in any context.
theseus_native = pytest.importorskip("theseus_native")

try:
    # The dispatcher lives in the Index-API repo. It may not be on
    # sys.path when running pytest directly inside theseus_native/. We
    # prepend the repo root so the import resolves.
    _REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    if _REPO_ROOT not in sys.path:
        sys.path.insert(0, _REPO_ROOT)
    # apps.notebook.sparse_ppr is intentionally Django-free (no model
    # imports, only logging + os + collections). It loads in a bare
    # interpreter without django.setup().
    from apps.notebook import sparse_ppr  # type: ignore[import-not-found]
except Exception:  # pragma: no cover
    sparse_ppr = None  # type: ignore[assignment]
    pytestmark = pytest.mark.skip(reason="apps.notebook.sparse_ppr not importable")


Adjacency = Dict[int, List[Tuple[int, float]]]
Seeds = Dict[int, float]


def _build_random_sparse_graph(num_nodes: int, avg_degree: float, rng: random.Random) -> Adjacency:
    """Generate a sparse undirected weighted graph with avg_degree edges/node.

    Node IDs are sampled from a non-contiguous range (multiplied by a
    prime) so the test exercises arbitrary integer keys, mimicking
    Theseus PKs.
    """
    node_ids = [(i + 1) * 31 for i in range(num_nodes)]
    adj: Adjacency = {nid: [] for nid in node_ids}
    target_edges = int(num_nodes * avg_degree // 2)
    for _ in range(target_edges):
        u, v = rng.sample(node_ids, 2)
        w = rng.uniform(0.1, 1.0)
        adj[u].append((v, w))
        adj[v].append((u, w))
    return adj


def _build_seeds(node_ids: List[int], num_seeds: int, rng: random.Random) -> Seeds:
    chosen = rng.sample(node_ids, min(num_seeds, len(node_ids)))
    weight = 1.0 / len(chosen)
    return {nid: weight for nid in chosen}


def _assert_dicts_close(
    got: Dict[int, float],
    expected: Dict[int, float],
    rel_tol: float,
    abs_tol: float,
) -> None:
    """Per-key relative-tolerance check.

    Uses absolute tolerance as a floor for keys whose expected value is
    near zero (e.g., 1e-7), where a 1e-5 relative comparison would be
    spuriously strict.
    """
    keys = set(got) | set(expected)
    diffs: List[Tuple[int, float, float, float]] = []
    for k in keys:
        a = got.get(k, 0.0)
        b = expected.get(k, 0.0)
        denom = max(abs(b), abs(a), abs_tol)
        rel = abs(a - b) / denom
        if rel > rel_tol:
            diffs.append((k, a, b, rel))
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

    # Native path: no env var set.
    env_native = {k: v for k, v in os.environ.items() if k != "THESEUS_DISABLE_NATIVE"}
    with patch.dict(os.environ, env_native, clear=True):
        native = sparse_ppr.push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)

    # Python path: force fallback.
    with patch.dict(os.environ, {"THESEUS_DISABLE_NATIVE": "1"}):
        python = sparse_ppr.push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)

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
    """Same parity check at 100K nodes. Bounded to 5 examples for runtime."""
    rng = random.Random(seed)
    adj = _build_random_sparse_graph(100_000, avg_degree, rng)
    seeds = _build_seeds(list(adj.keys()), num_seeds, rng)

    env_native = {k: v for k, v in os.environ.items() if k != "THESEUS_DISABLE_NATIVE"}
    with patch.dict(os.environ, env_native, clear=True):
        native = sparse_ppr.push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)
    with patch.dict(os.environ, {"THESEUS_DISABLE_NATIVE": "1"}):
        python = sparse_ppr.push_ppr(adj, seeds, alpha=alpha, epsilon=epsilon)

    _assert_dicts_close(native, python, rel_tol=1e-5, abs_tol=1e-9)


def _build_les_mis_like_adjacency() -> Adjacency:
    """Two communities + bridge, matching the fixture in
    apps/notebook/tests/test_sparse_ppr.py:_build_les_mis_like_adjacency.

    Verifying parity on the same fixture the Django suite uses anchors
    the native impl to the production test surface.
    """
    adj: Adjacency = {n: [] for n in range(10)}
    edges: List[Tuple[int, int, float]] = []
    for i in range(5):
        for j in range(i + 1, 5):
            edges.append((i, j, 1.0))
    for i in range(5, 10):
        for j in range(i + 1, 10):
            edges.append((i, j, 1.0))
    edges.append((4, 5, 0.3))
    for u, v, w in edges:
        adj[u].append((v, w))
        adj[v].append((u, w))
    return adj


def test_les_mis_fixture_parity() -> None:
    """Native and Python agree on the fixture used by the Django test suite."""
    adj = _build_les_mis_like_adjacency()
    seeds = {0: 1.0}

    env_native = {k: v for k, v in os.environ.items() if k != "THESEUS_DISABLE_NATIVE"}
    with patch.dict(os.environ, env_native, clear=True):
        native = sparse_ppr.push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)
    with patch.dict(os.environ, {"THESEUS_DISABLE_NATIVE": "1"}):
        python = sparse_ppr.push_ppr(adj, seeds, alpha=0.15, epsilon=1e-4)

    _assert_dicts_close(native, python, rel_tol=1e-5, abs_tol=1e-9)

    # Top-5 set must match the existing NETWORKX_TOP5_NODES expectation.
    top5_native = {nid for nid, _ in sorted(native.items(), key=lambda x: -x[1])[:5]}
    assert top5_native == {0, 1, 2, 3, 4}
