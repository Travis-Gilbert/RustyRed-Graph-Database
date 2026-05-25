FROM rust:1.88-bookworm AS builder

WORKDIR /app
ARG CARGO_BUILD_JOBS=2

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
# vendor/proto/ holds the in-tree snapshot of rustyred.v1 protos that
# crates/rustyred-server/build.rs compiles via tonic_build. Copying it
# before crates/ and src/ avoids invalidating the much larger Rust layer
# cache when only the proto changes. See docs/adr/0001-vendored-proto-for-railway-build.md.
COPY vendor ./vendor
COPY crates ./crates
COPY src ./src

RUN CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS}" cargo build -p rustyred-server --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/rustyred-server /usr/local/bin/rusty-red-graph-server

# Security-by-default. RUSTY_RED_REQUIRE_AUTH=true means /v1/* and /mcp
# refuse unauthenticated requests; operators must provision scoped
# tokens via RUSTY_RED_API_TOKENS (see README + SECURITY.md). For
# trusted-network or single-tenant deployments, set REQUIRE_AUTH=false
# at the Railway environment level — never bake it back into the image.
ENV RUSTY_RED_HOST="[::]" \
    RUSTY_RED_MODE="embedded" \
    RUSTY_RED_DATA_DIR="/app/data/rusty-red" \
    RUSTY_RED_REQUIRE_VOLUME=true \
    RUSTY_RED_DURABILITY="aof_everysec" \
    RUSTY_RED_SNAPSHOT_INTERVAL_WRITES="1000" \
    RUSTY_RED_REQUIRE_AUTH=true \
    RUSTY_RED_KEY_PREFIX="rusty-red:tenant" \
    RUSTY_RED_SERVICE_NAME="rusty-red-graph-database" \
    RUSTY_RED_API_TITLE="Rusty Red Graph Database API" \
    RUSTY_RED_MCP_ENABLED=true \
    RUSTY_RED_MCP_READ_ONLY=true \
    RUSTY_RED_MCP_ALLOW_ADMIN=false

EXPOSE 8380

CMD ["rusty-red-graph-server"]
