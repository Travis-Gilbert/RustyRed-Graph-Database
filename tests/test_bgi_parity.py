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
