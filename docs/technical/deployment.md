# Deployment

The release artifact is the `rustyred-server` binary, packaged as a single container. There is no
Redis sidecar in the default (`embedded`) mode.

## Container image

The `Dockerfile` is a two-stage build:

1. **Builder** — `rust:1.88-bookworm`, installs `protobuf-compiler` (needed because
   `build.rs` compiles the vendored `.proto` with `tonic-build`), copies `vendor/`, `crates/`, and
   `src/`, then runs `cargo build -p rustyred-server --release`. `CARGO_BUILD_JOBS` (default `2`)
   bounds parallelism for memory-limited CI builders.
2. **Runtime** — `debian:bookworm-slim` with `ca-certificates`. The binary is installed as
   `/usr/local/bin/rusty-red-graph-server`. `EXPOSE 8380`. `CMD ["rusty-red-graph-server"]`.

The image ships **security-by-default** environment values: `RUSTY_RED_REQUIRE_AUTH=true`,
`RUSTY_RED_MODE=embedded`, `RUSTY_RED_DATA_DIR=/app/data/rusty-red`, `RUSTY_RED_REQUIRE_VOLUME=true`,
`RUSTY_RED_DURABILITY=aof_everysec`, `RUSTY_RED_MCP_READ_ONLY=true`,
`RUSTY_RED_MCP_ALLOW_ADMIN=false`, and `RUSTY_RED_HOST=[::]`.

```bash
docker build -t rustyred:0.9.1 .
docker run -p 8380:8380 \
  -e RUSTY_RED_API_TOKENS="$(openssl rand -hex 32)=*" \
  -e RUSTY_RED_REQUIRE_VOLUME=false \
  rustyred:0.9.1
```

> `RUSTY_RED_REQUIRE_VOLUME` is `true` in the image. For a throwaway local run without a volume,
> override it to `false` as above. In production, attach a persistent volume instead.

## Railway

`railway.toml` configures a Dockerfile build with:

- **Healthcheck path** `/ready`, timeout `300s`.
- **Restart policy** up to `3` retries.

Railway provides `PORT` (which overrides `RUSTY_RED_PORT`) and `RAILWAY_VOLUME_MOUNT_PATH` (which
satisfies `RUSTY_RED_REQUIRE_VOLUME` and sets the data directory automatically). The one-click
template provisions the service, attaches a persistent volume at `/app/data/rusty-red`, and
pre-fills `RUSTY_RED_API_TOKENS` with a freshly generated secret. The full operator runbook —
backups, scaling, upgrades, troubleshooting — lives in [`../railway-template.md`](../railway-template.md).

Verify a deploy:

```bash
curl -s https://<service>.up.railway.app/ready
curl -s -H "Authorization: Bearer <token>" \
     https://<service>.up.railway.app/v1/diagnostics/config
```

## Ports

| Port | Protocol |
|------|----------|
| `8380` (or `PORT`) | HTTP **and** gRPC (content-type routed). |
| `6380` | RESP scaffold only — not served; see [Architecture](architecture.md). |

## Persistence & backups

In `embedded` mode the working graph is in memory; durability is on the data volume as an
append-only file plus periodic snapshots (`RUSTY_RED_DURABILITY`,
`RUSTY_RED_SNAPSHOT_INTERVAL_WRITES`). Each tenant has its own subdirectory under the data dir with a
manifest, snapshot, and AOF. **Back up by snapshotting the volume** (or copying the data directory)
while or after the service is quiesced. On restart the engine loads the latest snapshot and replays
AOF frames recorded after it; recovery tolerates orphan edges.

A directory lock prevents two processes from opening the same data directory simultaneously — do not
point two instances at one volume.

## On-disk format upgrades

The on-disk format is versioned (current `CURRENT_FORMAT_VERSION = 1`). A build refuses to load a
snapshot whose manifest version is newer than it understands. When a release bumps the format,
migrate in place with the bundled tool rather than export/re-import:

```bash
rustyred-upgrade-format <data-dir> [--dry-run]
```

It walks each tenant subdirectory, reads the existing manifest, applies the migration chain
`0 → 1 → … → CURRENT_FORMAT_VERSION`, and writes a fresh manifest + snapshot. Exit codes: `0` all
tenants upgraded (or already current), `1` at least one tenant refused (locked, malformed, or
too-new), `2` bad arguments. Stop the server (release the directory lock) before running it, and run
`--dry-run` first.

## Build features

Optional Cargo features change the compiled engine:

| Feature | Effect |
|---------|--------|
| `redis-store` | Compiles the legacy Redis backend (required for `RUSTY_RED_MODE=redis`). Enabled in the server build. |
| `tantivy` | Adds the Tantivy full-text backend, selectable via `RUSTY_RED_FULLTEXT_BACKEND=tantivy`. Default builds use the bundled BM25. |
| `s2` | Adds the S2-cell spatial backend, selectable via `RUSTY_RED_SPATIAL_BACKEND=s2`. Default builds use H3. |

The default Railway/Docker image does **not** enable `tantivy` or `s2`; setting those backends
without the matching feature compiled in is rejected at startup.
