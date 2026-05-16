"""Byte-parity tests for ``theseus_native::cmh``.

Confirms the Rust hashers produce identical output to the Python
fallbacks in ``apps.orchestrate.runtime.memory_canonical`` and
``apps.orchestrate.runtime.handoff_compiler``. Skipped automatically
when the installed ``theseus_native`` wheel does not include the cmh
exports (older wheels predate this addition).
"""

from __future__ import annotations

import hashlib
import json

import pytest

theseus_native = pytest.importorskip("theseus_native")

pytestmark = pytest.mark.skipif(
    not hasattr(theseus_native, "cmh_atom_id_v1"),
    reason="installed theseus_native wheel does not include cmh exports",
)


def _python_body_hash(text: str) -> str:
    normalized = " ".join(str(text or "").lower().split())
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()


def _python_atom_id_v1(workstream_id: str, kind: str, body: str) -> str:
    digest = hashlib.sha256(
        f"{workstream_id}\0{kind}\0{_python_body_hash(body)}".encode("utf-8"),
    ).hexdigest()[:32]
    return f"atom:{digest}"


def _python_handoff_state_hash_v1(canonical_json: str) -> str:
    return f"sha256:{hashlib.sha256(canonical_json.encode('utf-8')).hexdigest()}"


@pytest.mark.parametrize(
    "text",
    [
        "use Memgraph for handoff storage",
        "  Whitespace  AND CASE Should Not Matter  ",
        "",
        "pytest exit 1 — repeat",
        "unicode é á ü with NORMALIZE",
    ],
)
def test_body_hash_parity(text: str) -> None:
    rust = theseus_native.cmh_body_hash(text)
    assert rust == _python_body_hash(text)


@pytest.mark.parametrize(
    "workstream_id,kind,body",
    [
        ("workstream:demo-1", "decision", "Pin luma.gl to 9.2.6"),
        ("workstream:demo-2", "assumption", "Fixture handles SSL drop"),
        ("workstream:demo-3", "postmortem", "pytest exit 1 repeated 3x"),
        ("workstream:edge", "outcome", ""),
        ("a", "bx", "shared body"),
        ("ab", "x", "shared body"),  # null-byte separator anti-collision
    ],
)
def test_atom_id_v1_parity(
    workstream_id: str, kind: str, body: str,
) -> None:
    rust = theseus_native.cmh_atom_id_v1(workstream_id, kind, body)
    py = _python_atom_id_v1(workstream_id, kind, body)
    assert rust == py
    assert rust.startswith("atom:")
    assert len(rust) == len("atom:") + 32


def test_atom_id_v1_null_byte_separates_ws_from_kind() -> None:
    """``ws='a', kind='bx'`` MUST NOT collide with ``ws='ab', kind='x'``."""
    a = theseus_native.cmh_atom_id_v1("a", "bx", "body")
    b = theseus_native.cmh_atom_id_v1("ab", "x", "body")
    assert a != b


def test_handoff_state_hash_v1_parity() -> None:
    payload = {
        "handoff_id": "handoff:demo",
        "workstream_id": "workstream:demo",
        "task_state": "active",
        "summary": "fresh capsule",
        "files_touched": ["a.py", "b.py"],
        "decisions": [],
        "state_hash": "",
    }
    canonical_json = json.dumps(payload, sort_keys=True, default=str)
    rust = theseus_native.cmh_handoff_state_hash_v1(canonical_json)
    py = _python_handoff_state_hash_v1(canonical_json)
    assert rust == py
    assert rust.startswith("sha256:")
    assert len(rust) == len("sha256:") + 64


def test_python_callers_match_native() -> None:
    """``memory_canonical._atom_id`` and ``handoff_compiler._state_hash``
    must produce byte-identical output to the Rust reference impls.

    Per the benchmark in
    ``theseus_native/src/cmh.rs`` docstring, Python is the production
    runtime for these microsecond-scale hashers (PyO3 boundary
    overhead would dominate). This test pins the cross-language
    contract so future Rust-native federation peers can compute
    identical ids/hashes without re-deriving the algorithm.
    """
    from apps.orchestrate.runtime import memory_canonical, handoff_compiler

    py_atom = memory_canonical._atom_id(
        "workstream:p1", "decision", "Use [:DERIVED_FROM] edges",
    )
    rust_atom = theseus_native.cmh_atom_id_v1(
        "workstream:p1", "decision", "Use [:DERIVED_FROM] edges",
    )
    assert py_atom == rust_atom

    payload = {
        "handoff_id": "handoff:p1",
        "workstream_id": "workstream:p1",
        "summary": "parity test",
    }
    py_state = handoff_compiler._state_hash(payload)
    canonical_json = json.dumps(payload, sort_keys=True, default=str)
    rust_state = theseus_native.cmh_handoff_state_hash_v1(canonical_json)
    assert py_state == rust_state
