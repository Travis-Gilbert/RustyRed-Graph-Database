from __future__ import annotations

import pytest

rusty_red_native = pytest.importorskip("rusty_red_native")

pytestmark = pytest.mark.skipif(
    not hasattr(rusty_red_native, "bgi_compact_receipts_json"),
    reason="installed rusty_red_native wheel does not include BGI parity exports",
)


def test_bgi_native_egraph_and_datalog_exports_are_executable() -> None:
    import json

    egraph = json.loads(rusty_red_native.bgi_egraph_extract_context_pack_json(json.dumps({
        "expression_id": "native-test",
        "items": [
            {"id": "a", "text": "keep", "tokens": 2, "obligation_id": "o1"},
            {"id": "b", "text": "keep", "tokens": 2, "obligation_id": "o1"},
            {"id": "empty", "text": "", "tokens": 1},
        ],
    })))
    assert egraph["native_backend"] == "rust-egg-context-pack"
    assert egraph["equivalent"] is True
    assert len(egraph["extraction"]["items"]) == 1

    datalog = json.loads(rusty_red_native.bgi_datalog_derive_core_json(json.dumps([
        {"relation": "claim", "entity_id": "claim-1", "attributes": {"status": "proposed"}, "fact_id": "f1"},
        {"relation": "object", "entity_id": "obj-1", "attributes": {"title": "Same"}, "fact_id": "f2"},
        {"relation": "object", "entity_id": "obj-2", "attributes": {"title": "same"}, "fact_id": "f3"},
    ])))
    assert datalog["engine"] == "rust-datafrog-core"
    assert datalog["derived_count"] >= 3
