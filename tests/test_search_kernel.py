from __future__ import annotations

import pytest

theseus_native = pytest.importorskip("theseus_native")


def test_search_normalize_urls_batch():
    assert theseus_native.search_normalize_urls_batch([
        "HTTPS://Example.com/a?utm_source=x&b=1#frag",
    ]) == ["https://example.com/a?b=1"]


def test_search_cosine_topk():
    assert theseus_native.search_cosine_topk(
        [1.0, 0.0],
        [("b", [0.0, 1.0]), ("a", [1.0, 0.0])],
        1,
    ) == [("a", 1.0)]


def test_search_fuse_scores_batch():
    rows = theseus_native.search_fuse_scores_batch(
        [{"score_components": {"lexical": 0.5, "dense": 0.5}}],
        {"lexical": 1.0, "dense": 0.5},
    )
    assert rows[0]["fused_score"] == 0.75
