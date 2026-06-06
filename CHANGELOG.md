# Changelog

All notable changes to RustyRed GraphDB are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project aims to follow
semantic versioning. The whole workspace ships a single version, sourced from
`[workspace.package].version` in the root `Cargo.toml`.

## [0.9.1] - 2026-06-05

First public launch release. Supersedes the unreleased 0.7.x / 0.8.x line.

### Fixed

- **HNSW vector search through the HTTP and MCP API.** The tenant executor's
  read-side committed snapshot was rebuilt from a node/edge-only `GraphSnapshot`,
  which dropped vector designations — so `vector/search` and `vector/hybrid`
  always returned an empty result through the API even though the writer was
  correctly indexed. The committed read-view is now a faithful clone of the
  writer's store (designations + HNSW index). Regression tests cover both the
  designate-then-upsert and upsert-then-designate orderings.

### Security

- **Constant-time bearer token comparison.** `require_scope` now compares tokens
  with `subtle::ConstantTimeEq` instead of `==`, removing a timing side channel.
- **Fail-fast on misconfigured auth.** With `RUSTY_RED_REQUIRE_AUTH=true` and no
  `RUSTY_RED_API_TOKENS`, the server now refuses to boot with a clear message
  rather than silently rejecting every authenticated request.

### Added

- Federated RustyWeb Web Commons sync (`/federate/submit`, signed fragments).
- Git-like graph version packs: content-addressed compile, diff, ref/log,
  checkout, and three-way merge over HTTP and MCP.
- Harness Instant KG merged views (code PPR, impact, related-objects, search,
  edge explanations).
- gRPC `rustyred.v1.GraphDatabase` service parity with the HTTP read surface.
- Pure-Rust ACL local-push Personalized PageRank — no Python runtime or native
  extension dependency.
- A reproducible benchmark harness (`scripts/bench/run.sh`) and methodology
  ([docs/benchmarks.md](docs/benchmarks.md)) for ingest rate and PPR latency.
- CI gate (`fmt` + `clippy -D warnings` + `test`), a tag-driven release workflow
  (prebuilt binaries), and a GHCR image-publish workflow.
- Packaging metadata and a `LICENSE` file so `rustyred-core` and the root facade
  can be published to crates.io. See [RELEASING.md](RELEASING.md).

### Changed

- Crawled graph batches are now indexed into search on ingest.
- Removed an unstable dense-vector dependency from the search crate.
- Hardened MCP write tools; added gRPC reads.
- Snapshot recovery now tolerates orphan edges instead of failing the load.
- The workspace moved to single-source version inheritance
  (`[workspace.package]`), aligning every crate (notably `rustyred-search`,
  previously 0.1.0) to one version.
- README rewritten to lead on the agent-native (MCP) angle, with a copy-paste
  MCP client config, a 60-second HTTP walkthrough, and a measured benchmark
  headline instead of an unbenchmarked speed claim.

[0.9.1]: https://github.com/Travis-Gilbert/RustyRed-Graph-Database/releases/tag/v0.9.1
