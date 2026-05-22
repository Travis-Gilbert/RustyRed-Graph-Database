# ADR 0001 — Vendor `rustyred.proto` for hermetic Docker / Railway builds

Status: Accepted
Date: 2026-05-22
Decision drivers: Travis Gilbert

## Context

The `rustyred-server` crate compiles `rustyred.v1` protobuf definitions
into Rust tonic bindings at build time. The `.proto` file is sourced
from a git submodule:

```
proto/                                  -> submodule `theorem-protos`
└── rustyred/v1/rustyred.proto
```

`crates/rustyred-server/build.rs` reads from that path during `cargo
build`. Locally, the file exists because the developer ran
`git submodule update --init`. On Railway — and any other CI/CD that
performs a default `git clone` without recursive submodule fetch — the
file is missing. The build fails with the message:

> Cannot find theorem-protos at proto/rustyred/v1/rustyred.proto.
> Run `git submodule update --init` before building.

This is a hermeticity gap: the Docker build context the Railway template
sees does not match the build context the developer sees, even though
they nominally run "the same" build.

RustyRed is intended to ship as a Railway template. The template path
must build deterministically from a fresh public clone with no extra
steps.

## Decision

Maintain an in-tree vendored copy of the proto at
`vendor/proto/rustyred/v1/rustyred.proto`, kept byte-identical to the
submodule via a sync script and a CI check. `build.rs` prefers the
vendored copy and falls back to the submodule for developers actively
editing the upstream proto.

Concretely:

1. The vendored file is committed to the repository and copied into the
   Docker build context.
2. `scripts/sync-vendored-proto.sh` copies `proto/rustyred/v1/rustyred.proto`
   into `vendor/proto/rustyred/v1/rustyred.proto` and records the source
   commit hash in `vendor/proto/SOURCE_COMMIT`.
3. `.github/workflows/vendored-proto-up-to-date.yml` runs
   `scripts/sync-vendored-proto.sh --check` on every PR that touches
   `proto/**`, `vendor/proto/**`, or the script itself, and fails the PR
   if the vendored copy drifts from the submodule.
4. `crates/rustyred-server/build.rs` looks up the proto in this order:
   - `<workspace>/vendor/proto/rustyred/v1/rustyred.proto` (preferred,
     hermetic path)
   - `<workspace>/proto/rustyred/v1/rustyred.proto` (submodule fallback,
     used by developers editing the upstream contract)
5. The Dockerfile adds `COPY vendor ./vendor` to the build context. It
   does not `COPY proto ./proto`.

The first vendored snapshot is taken at `theorem-protos` commit
`b64a414950ff4d08e3be772c8a5de03665c11e39` — the submodule HEAD at the
time of this ADR. There are no upstream tags yet, so this commit acts
as the de facto pin.

## Alternatives considered

### Option A — Vendor the proto (chosen)

- **Upside:** Zero Railway-side magic. Docker build context is hermetic.
  Reproducible builds for any consumer who clones the public repo.
  Survives any future change in Railway's submodule handling.
- **Risk:** Two sources of truth for the proto. Mitigated by the sync
  script and the CI guard. Adds one file to the repo and one workflow
  to CI.
- **Validation:** `cargo build -p rustyred-server` succeeds with
  `proto/` absent. `docker build .` succeeds without a submodule init
  step. The CI check fails on intentional drift, passes after sync.

### Option B — Initialize the submodule inside the Dockerfile

- **Upside:** Single source of truth (the submodule).
- **Rejected because:** Requires either `COPY .git` (which leaks the
  entire git history into the image layers and is wasteful), or an
  HTTPS clone of `theorem-protos` during the builder stage. The HTTPS
  approach adds a network dependency to the build and breaks air-gapped
  rebuilds, slowing CI for marginal benefit over Option A.

### Option C — Commit the generated tonic bindings

- **Upside:** Removes the proto dependency from build time entirely.
  Fastest Docker builds.
- **Rejected because:** Generated code becomes hand-maintained. Every
  proto change requires a manual regen commit. Regeneration drift would
  not be caught for months at a time. The tonic codegen API also evolves
  faster than the proto itself, making the generated artifact tied to
  a specific `tonic_build` version rather than to the contract.

### Option D — Railway-side submodule flag

- **Upside:** Zero repo changes.
- **Rejected because:** Railway does not currently expose a documented
  build-time submodule toggle, and tying the template to undocumented
  behavior is fragile. Option A is independent of platform.

## Consequences

### Positive

- Railway builds succeed from a fresh clone.
- Any consumer (developer, CI runner, downstream subtree fork) can build
  without setting up submodules.
- The downstream sync workflow (`scripts/sync-downstream-subtree.sh`,
  which uses `git subtree pull --squash`) carries `vendor/proto/` to
  downstream consumers automatically without script changes.
- The submodule remains intact for developers who edit the upstream
  proto contract; the fallback path in `build.rs` lets them test
  changes before running the sync script.

### Negative

- The vendored proto must be kept in sync with the submodule. The CI
  guard enforces this on every PR. If a developer edits
  `proto/rustyred/v1/rustyred.proto` without running
  `scripts/sync-vendored-proto.sh`, their PR fails.
- `vendor/proto/SOURCE_COMMIT` records the source commit, but the
  vendored copy is not cryptographically tied to it. The sync script
  rewrites both atomically; the CI check verifies both match the
  current submodule HEAD.

### Operational

- The local dev workflow no longer requires `git submodule update --init`
  for first build. The README "Build (local development)" section
  reflects this.
- Developers actively editing the upstream proto must:
  1. `git submodule update --init` (one-time per clone)
  2. Edit `proto/rustyred/v1/rustyred.proto`
  3. Run `scripts/sync-vendored-proto.sh`
  4. Commit both the submodule pointer bump and the vendored copy

## Reversibility

Fully reversible. To revert:

1. Delete `vendor/proto/`.
2. Restore `build.rs` to use the submodule path only.
3. Restore the Dockerfile's `COPY proto ./proto` line (and ensure CI
   initializes submodules before building).
4. Remove `scripts/sync-vendored-proto.sh` and the CI workflow.

The submodule itself is untouched by this ADR; reverting only removes
the in-tree mirror.

## Related

- `crates/rustyred-server/build.rs` — proto source resolution logic.
- `scripts/sync-vendored-proto.sh` — sync + check script.
- `.github/workflows/vendored-proto-up-to-date.yml` — CI guard.
- `Dockerfile` — `COPY vendor ./vendor` line.
- `vendor/proto/SOURCE_COMMIT` — recorded source commit.
- `docs/plans/railway-template-publication.md` — the plan this ADR
  records.
