//! Continuous Agent Memory Harness (CMH) native helpers.
//!
//! Exposes two PyO3 functions used by
//! ``apps.orchestrate.runtime.memory_canonical`` and
//! ``apps.orchestrate.runtime.handoff_compiler``. The corresponding
//! Python implementations remain in place as graceful fallbacks per the
//! `push_ppr` pattern (see `apps/notebook/sparse_ppr.py`). Byte-parity
//! is enforced by `theseus_native/tests/test_cmh_parity.py`.
//!
//! Why Rust here, given Python's hashlib is already C-backed?
//!
//! Honest answer based on a 50k-iteration microbenchmark on the build
//! machine: at single-call granularity, **Python is faster than Rust**
//! for these hashers (Py 1.85us vs Rust 3.60us per call). The PyO3
//! boundary crossing costs more than the work saved because each
//! function does only two SHA256 rounds + a string concat. As a
//! result, the Python paths in
//! `apps/orchestrate/runtime/memory_canonical.py::_atom_id` and
//! `apps/orchestrate/runtime/handoff_compiler.py::_state_hash` do NOT
//! route through this module in production — they remain pure Python.
//!
//! This module is retained for three real-but-narrow reasons:
//!
//!   1. **Federation contract**: the CMH spec (§9, §11.4) expects
//!      cross-language consumers to compute the same atom_id and
//!      handoff state_hash from raw inputs. A canonical Rust impl that
//!      Python is byte-parity-tested against prevents algorithm drift
//!      between hosts even if no single host calls both.
//!
//!   2. **Future batch interface**: a hypothetical
//!      `cmh_atom_id_v1_batch(rows: Vec<(String, String, String)>)`
//!      that hashes ~hundreds of atoms in one PyO3 call would amortize
//!      the boundary overhead and become a net win for nightly
//!      bulk-canonicalize. Adding it later does not require changing
//!      the Python contract.
//!
//!   3. **Second source of truth for parity tests**:
//!      `theseus_native/tests/test_cmh_parity.py` cross-checks the
//!      Python implementation against this Rust reference so that any
//!      future refactor of the Python impl cannot silently change the
//!      output digest.
//!
//! Algorithm v1 contract (must stay byte-stable across host languages):
//!
//!   body_hash(text)
//!     = sha256_hex( normalize(text) )
//!     where normalize(text) = " ".join(text.to_lowercase().split_whitespace())
//!
//!   atom_id_v1(workstream_id, kind, body)
//!     = "atom:" + sha256_hex(workstream_id || "\0" || kind || "\0" || body_hash(body))[..32]
//!
//!   handoff_state_hash_v1(canonical_json, target_tokens, hard_cap)
//!     = "sha256:" + sha256_hex( canonical_json )
//!     where canonical_json is produced by the caller with
//!     json.dumps(payload, sort_keys=True, default=str). Target tokens
//!     and hard_cap participate in `payload` per the existing Python
//!     impl; this function does not re-canonicalize.

use pyo3::prelude::*;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Normalize body text the same way Python's
/// ``memory_canonical._body_hash`` does:
///   ``" ".join(str(text or "").lower().split())``
fn normalize_body(text: &str) -> String {
    text.split_whitespace()
        .map(|chunk| chunk.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// SHA256 hex of the normalized body text.
#[pyfunction]
pub fn cmh_body_hash(text: &str) -> String {
    sha256_hex(normalize_body(text).as_bytes())
}

/// Deterministic atom id v1 used by ``memory_canonical._atom_id``.
///
/// Encoding mirrors the Python implementation exactly so the
/// canonicalize pipeline can emit byte-identical ids regardless of
/// which language ran it.
#[pyfunction]
pub fn cmh_atom_id_v1(workstream_id: &str, kind: &str, body: &str) -> String {
    let mut keyed = String::with_capacity(workstream_id.len() + kind.len() + 66);
    keyed.push_str(workstream_id);
    keyed.push('\0');
    keyed.push_str(kind);
    keyed.push('\0');
    keyed.push_str(&sha256_hex(normalize_body(body).as_bytes()));
    let digest = sha256_hex(keyed.as_bytes());
    let prefix: String = digest.chars().take(32).collect();
    format!("atom:{prefix}")
}

/// HandoffArtifact state hash v1. The caller passes the canonical JSON
/// string (sorted keys, str fallback) so this function is purely a
/// digest step. Keeping serialization on the Python side avoids
/// recomputing the dict layout in Rust and matches the established
/// Python contract byte-for-byte.
#[pyfunction]
pub fn cmh_handoff_state_hash_v1(canonical_json: &str) -> String {
    format!("sha256:{}", sha256_hex(canonical_json.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_hash_normalizes_whitespace_and_case() {
        let a = cmh_body_hash("Use   Memgraph  for STORAGE");
        let b = cmh_body_hash(" use memgraph for storage ");
        assert_eq!(a, b);
    }

    #[test]
    fn atom_id_is_deterministic() {
        let a = cmh_atom_id_v1("workstream:1", "decision", "Pin luma.gl 9.2.6");
        let b = cmh_atom_id_v1("workstream:1", "decision", "Pin luma.gl 9.2.6");
        assert_eq!(a, b);
        assert!(a.starts_with("atom:"));
        assert_eq!(a.len(), "atom:".len() + 32);
    }

    #[test]
    fn atom_id_separates_workstream_from_kind() {
        // The null-byte separator must prevent ws||kind concatenation
        // collisions (e.g., ws="a", kind="bx" vs ws="ab", kind="x").
        let a = cmh_atom_id_v1("a", "bx", "body");
        let b = cmh_atom_id_v1("ab", "x", "body");
        assert_ne!(a, b);
    }

    #[test]
    fn handoff_state_hash_prefixes_sha256() {
        let h = cmh_handoff_state_hash_v1("{\"a\":1}");
        assert!(h.starts_with("sha256:"));
        assert_eq!(h.len(), "sha256:".len() + 64);
    }
}
