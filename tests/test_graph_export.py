from __future__ import annotations

import numpy as np
import pytest

theseus_native = pytest.importorskip("theseus_native")


def test_graph_remap_ids_batch_is_dense_and_stable():
    result = theseus_native.graph_remap_ids_batch(np.array([42, 7, 42], dtype=np.int64))
    assert result["dense_ids"].tolist() == [0, 1, 0]
    assert result["unique_ids"].tolist() == [42, 7]


def test_graph_pack_edges_batch_filters_missing_endpoints():
    result = theseus_native.graph_pack_edges_batch(
        np.array([10, 20, 30], dtype=np.int64),
        np.array([20, 99, 10], dtype=np.int64),
        np.array([1, 2, 1], dtype=np.int32),
        {10: 0, 20: 1, 30: 2},
    )
    assert result["src"].tolist() == [0, 2]
    assert result["dst"].tolist() == [1, 0]
    assert result["rel"].tolist() == [1, 1]
