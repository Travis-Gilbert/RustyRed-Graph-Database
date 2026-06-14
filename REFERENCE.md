# REFERENCE.md — pinned sources for the CRDT substrate build

Per the grounding contract in `Theorem/docs/spec-0-crdt-substrate.md`: external
protocols and CRDT semantics are lossy in model memory. This file pins what the
DB-repo lane (Parts 3/5/7 of SPEC-RUSTYRED-CRDT) binds to, so the binding is to
what is actually there, not reconstructed.

## Spec lineage

- **SPEC-RUSTYRED-CRDT.md** (the execution handoff being built) supersedes the
  op-based four-set framing in `Theorem/docs/spec-0-crdt-substrate.md`: it picks
  a delta-state CRDT over the typed graph, HLC clocks, `yrs`, and reuse of
  `versioned_graph`'s `AutoConfidence` confidence-merge. spec-0 SC-A1..A4 are
  satisfied by the Downloads-spec acceptance (add-wins via the existing
  `EdgeRecord.tombstone`, not a 2P-set). Build to the Downloads spec; map tests
  back to spec-0 success criteria.

## Pinned dependencies (this repo)

- **yrs = 0.27.0** (`Cargo.lock`; declared `yrs = "0.27"` in
  `crates/rustyred-server/Cargo.toml`). The deployed `yjs_sync.rs` binds to it.
  Part 5 extends that binding (code buffers, per-span provenance, awareness).
  The yrs version is the authority for the provenance representation and the
  awareness frame; API confirmed by compiling against this exact version.

## Protocol lineage

- The `yjs_sync.rs` 3-tag frame protocol (`0x00` PULL state-vector / `0x01`
  PULL_REPLY diff / `0x02` UPDATE apply+broadcast) is a deliberate 1:1 subset of
  the y-websocket / AFFiNE sync-gateway protocol, mapped onto BlockSuite's
  `DocSource`. The frontend half is `RustyRedDocSource` in the
  Open-Flint-Atlas civic-editor bundle; the shared contract is the planner
  folder's SCHEMA-CONTRACT.md. Part 5 adds `0x03` AWARENESS (broadcast-only,
  never persisted). Part 3 mirrors the same 3-tag shape for graph deltas at
  `/v1/tenants/:tenant_id/sync/graph/:room_id`.

## Cross-repo wire-type seam (Part 3)

The two trees have separate clean-room cores (`rustyred-thg-core` in Theorem,
`rustyred-core` here). `StampedBatch` / `StampedMutation` / `VersionVector`
authored by Codex in `rustyred-thg-core/src/crdt/merge.rs` are ported verbatim
into this repo's `rustyred-core` for the Part 3 transport. Encoding: serde_json,
matching the existing `GraphMutationBatch` write path. Wire shape to be frozen by
Codex once `merge.rs` compiles; recorded in coordination room `room:crdt-substrate`
decision `record_a05847db14d39d04`.
