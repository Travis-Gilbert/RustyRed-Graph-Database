FROM rust:1.88-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src ./src

RUN cargo build -p thg-product-server --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/thg-product-server /usr/local/bin/rusty-red-graph-server

ENV THG_PRODUCT_HOST="[::]" \
    RUSTY_RED_HOST="[::]" \
    RUSTY_RED_REQUIRE_AUTH=false \
    RUSTY_RED_KEY_PREFIX="rusty-red:tenant" \
    RUSTY_RED_SERVICE_NAME="rusty-red-graph-database" \
    RUSTY_RED_API_TITLE="Rusty Red Graph Database API" \
    RUSTY_RED_MCP_ENABLED=true \
    RUSTY_RED_MCP_READ_ONLY=true \
    RUSTY_RED_MCP_ALLOW_ADMIN=false

EXPOSE 8380

CMD ["rusty-red-graph-server"]
