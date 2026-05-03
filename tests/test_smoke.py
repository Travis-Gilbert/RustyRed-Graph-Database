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
