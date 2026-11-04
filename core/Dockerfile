FROM rust:1.90-slim-bookworm AS builder
WORKDIR /app
ENV SQLX_OFFLINE=true

# ── 1. Cache external dependencies ───────────────────────────────────────────
# Copy only manifests first — this layer is only invalidated when
# Cargo.toml/Cargo.lock change, not on every source edit.
COPY Cargo.toml Cargo.lock ./
COPY core/Cargo.toml core/Cargo.toml
COPY zkp/Cargo.toml zkp/Cargo.toml

# Build dummy sources to compile (and cache) all external dependencies.
# The actual application source is replaced in the next step.
RUN mkdir -p core/src core/benches zkp/src && \
    echo 'fn main() {}' > core/src/main.rs          && \
    echo ''             > core/src/lib.rs            && \
    echo ''             > core/benches/engine_benchmark.rs && \
    echo ''             > zkp/src/lib.rs             && \
    cargo build -p core --release --locked && \
    rm -rf core/src core/benches zkp/src

# ── 2. Build real application source ─────────────────────────────────────────
# External deps are already compiled; only changed crate sources are recompiled.
COPY .sqlx .sqlx
COPY core core
COPY zkp zkp
# touch forces cargo to see source files as newer than the dummy build artifacts.
RUN touch core/src/main.rs core/src/lib.rs zkp/src/lib.rs && \
    cargo build -p core --release --locked

# ── Runtime image ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/core /usr/local/bin/core

EXPOSE 3000
ENV RUST_LOG=info
CMD ["core"]
