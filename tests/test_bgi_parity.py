from __future__ import annotations

import pytest

theseus_native = pytest.importorskip("theseus_native")

pytestmark = pytest.mark.skipif(
    not hasattr(theseus_native, "bgi_compact_receipts_json"),
    reason="installed theseus_native wheel does not include BGI parity exports",
)


def test_bgi_native_parity_benchmarks_match_python_reference() -> None:
    from apps.notebook.benchmarks.bgi_native_parity import run_all_parity_benchmarks

    report = run_all_parity_benchmarks(
        iterations=2,
        native_module=theseus_native,
    )

    assert report["native_available"] is True
    assert report["all_parity_passed"] is True
    assert {item["name"] for item in report["benchmarks"]} == {
        "egraph_receipt_summary",
        "datalog_receipt_summary",
        "fact_pack_hash",
        "receipt_compaction",
    }


def test_bgi_native_egraph_and_datalog_exports_are_executable() -> None:
    import json

    egraph = json.loads(theseus_native.bgi_egraph_extract_context_pack_json(json.dumps({
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

    datalog = json.loads(theseus_native.bgi_datalog_derive_core_json(json.dumps([
        {"relation": "claim", "entity_id": "claim-1", "attributes": {"status": "proposed"}, "fact_id": "f1"},
        {"relation": "object", "entity_id": "obj-1", "attributes": {"title": "Same"}, "fact_id": "f2"},
        {"relation": "object", "entity_id": "obj-2", "attributes": {"title": "same"}, "fact_id": "f3"},
    ])))
    assert datalog["engine"] == "rust-datafrog-core"
    assert datalog["derived_count"] >= 3
