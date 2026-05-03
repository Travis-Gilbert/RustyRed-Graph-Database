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


def test_single_seed_no_edges_keeps_alpha_mass():
    """Seed at an isolated node: alpha-fraction stays, (1-alpha) is lost."""
    from theseus_native import push_ppr
    adj = {7: []}
    result = push_ppr(adj, {7: 1.0}, alpha=0.15, epsilon=1e-4)
    # Threshold for node 7 = 1e-4 * max(0.0, 1.0) = 1e-4. Initial residual
    # is 1.0 > 1e-4, so we push exactly once: p[7] = 0.15, r[7] = 0, no
    # spread (no neighbors). The (1-alpha) = 0.85 mass is the teleport
    # sink loss. Tolerance must allow for this exact outcome.
    assert abs(result[7] - 0.15) < 1e-9


def test_disconnected_graph_does_not_leak_mass():
    """Two disconnected components; seed in one. Other component stays at 0."""
    from theseus_native import push_ppr
    adj = {
        0: [(1, 1.0)], 1: [(0, 1.0)],
        100: [(101, 1.0)], 101: [(100, 1.0)],
    }
    result = push_ppr(adj, {0: 1.0}, alpha=0.15, epsilon=1e-4)
    assert 0 in result and 1 in result
    assert 100 not in result
    assert 101 not in result


def test_non_contiguous_pks_work():
    """Theseus PKs are arbitrary ints (e.g. 142857, 9000001). Verify."""
    from theseus_native import push_ppr
    adj = {
        142857: [(9000001, 0.7)],
        9000001: [(142857, 0.7)],
    }
    result = push_ppr(adj, {142857: 1.0}, alpha=0.15, epsilon=1e-4)
    assert 142857 in result
    assert 9000001 in result
    assert result[142857] > result[9000001]


def test_max_pushes_caps_iteration():
    """Set max_pushes = 1 on a 2-node graph; exactly one push runs.

    With max_pushes=1 on the seed of a 2-node ring, the seed pops once,
    captures alpha * 1.0 = 0.15 into p[0], and spreads (1 - alpha) =
    0.85 to its sole neighbor as residual. The loop then exits because
    the push counter equals the cap. Node 1 has residual but no
    captured mass, so it is absent from the returned dict (p).
    """
    from theseus_native import push_ppr
    result = push_ppr({0: [(1, 1.0)], 1: [(0, 1.0)]}, {0: 1.0}, alpha=0.15, epsilon=1e-4, max_pushes=1)
    # After exactly one push, only node 0 has captured mass.
    assert 0 in result
    # Node 1 received residual but never got popped, so it is not in the
    # captured-mass dict p.
    assert 1 not in result


def test_empty_adjacency_with_seeds():
    """Empty adjacency dict, seeds present: behaves like isolated seeds."""
    from theseus_native import push_ppr
    result = push_ppr({}, {5: 1.0, 6: 0.5}, alpha=0.15, epsilon=1e-4)
    # Both seeds get one push each (residuals exceed threshold of 1e-4).
    assert abs(result[5] - 0.15) < 1e-9
    assert abs(result[6] - 0.5 * 0.15) < 1e-9
