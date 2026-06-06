# Releasing RustyRed

Nothing in this repository publishes automatically. Every outward action below
is a deliberate maintainer step. CI (`.github/workflows/ci.yml`) only builds,
lints, and tests; it never publishes.

The workspace ships a single version, sourced from `[workspace.package].version`
in the root `Cargo.toml` (currently **0.9.1**). Bump it there once; every crate
inherits it.

## 1. Pre-flight

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Confirm the version is correct:

```bash
grep -m1 '^version' Cargo.toml        # [workspace.package] -> 0.9.1
```

## 2. Tag the release (prebuilt binaries + container image)

Pushing a `vX.Y.Z` tag triggers two workflows:

- `release.yml` builds `rustyred-server` for macOS (arm64 + x86_64), Linux
  (x86_64), and Windows, and attaches the archives to the GitHub Release.
- `docker-publish.yml` builds the production `Dockerfile` and pushes
  `ghcr.io/travis-gilbert/rustyred:X.Y.Z` and `:latest` to GHCR.

```bash
git tag v0.9.1
git push origin v0.9.1
```

You can also run either workflow manually from the Actions tab
(`workflow_dispatch`).

## 3. One-line installers (curl|sh, Homebrew, npm) — optional

The installer ecosystem is handled by `cargo-dist`. Config is staged under
`[workspace.metadata.dist]` in `Cargo.toml`.

```bash
cargo install cargo-dist        # provides the `dist` binary
dist init                       # reads the staged config, pins dist-version
git add Cargo.toml .github/workflows/release.yml
git commit -m "ci(release): adopt cargo-dist installers"
git push && git tag v0.9.1 && git push origin v0.9.1
```

`dist init` regenerates `release.yml` with the curl|sh installer, a Homebrew
formula, and an npm shim. It supersedes the hand-written binary-build workflow.

## 4. Publish to crates.io — permanent, do last

Only two crates are published; the five binaries are marked `publish = false`.
crates.io publishes are **irreversible** (you cannot unpublish, only yank).
Publish in dependency order:

```bash
cargo publish -p rustyred-core      # the engine; must go first
cargo publish -p rusty_red_native   # the root facade; depends on rustyred-core
```

`cargo publish` enforces `--locked` semantics and rejects a dirty tree, so
commit everything first. After publishing, `cargo add rustyred-core` works for
downstream users and docs.rs builds the API reference.

## What is intentionally not published here

`rustyred-server`, `rustyred-mcp`, `rustyred-compat-server`,
`rustyred-resp-server`, and `rustyred-search` are `publish = false`: they are
deployment/binary crates, distributed via the container image and the GitHub
Release binaries, not crates.io.
