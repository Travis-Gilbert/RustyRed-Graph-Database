"""Smoke test: confirm the wheel is importable after maturin develop."""

import pytest


def test_import_push_ppr():
    from theseus_native import push_ppr
    assert callable(push_ppr)


def test_empty_seeds_returns_empty():
    from theseus_native import push_ppr
    result = push_ppr({0: [(1, 1.0)], 1: [(0, 1.0)]}, {})
    assert result == {}


def test_signature_kwargs_only():
    """alpha, epsilon, max_pushes must be keyword-only."""
    from theseus_native import push_ppr
    with pytest.raises(TypeError):
        push_ppr({0: [], 1: []}, {0: 1.0}, 0.15, 1e-4, 200_000)  # positional kwargs forbidden


def test_chain_graph_matches_python_reference():
    """4-node chain: 0-1-2-3 with weight 1.0 edges, seed at 0.

    The Python reference in apps/notebook/sparse_ppr.py:push_ppr is the
    ground truth. With alpha=0.15, epsilon=1e-4 the algorithm should
    capture mass on every node. Node 1 dominates (it is the most central
    bridge); node 3 is the most distant.
    """
    from theseus_native import push_ppr

    adj = {
        0: [(1, 1.0)],
        1: [(0, 1.0), (2, 1.0)],
        2: [(1, 1.0), (3, 1.0)],
        3: [(2, 1.0)],
    }
    result = push_ppr(adj, {0: 1.0}, alpha=0.15, epsilon=1e-4)
    # Reference values from the live Python push_ppr on the same input.
    # Captured 2026-05-03 by running the live function locally; recorded
    # here to keep this test independent of apps.notebook (which the
    # crate's test env does not import).
    expected = {
        0: 0.302171130,
        1: 0.358084514,
        2: 0.238209537,
        3: 0.101239053,
    }
    assert set(result.keys()) >= set(expected.keys())
    for node, ref in expected.items():
        got = result[node]
        rel = abs(got - ref) / max(abs(ref), 1e-12)
        assert rel < 1e-4, f"node {node}: got {got}, expected {ref}, rel diff {rel}"
