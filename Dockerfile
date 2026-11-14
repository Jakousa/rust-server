# syntax=docker/dockerfile:1
FROM rust:slim-bookworm AS builder

WORKDIR /app

# Install system build dependencies required by tonic-build and prost
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests and source files
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build release binaries for all workspace members using BuildKit cache mounts,
# then copy compiled binaries to /app/bin so they persist after the cache-mounted
# /app/target directory is unmounted.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --bins && \
    mkdir -p /app/bin && \
    cp /app/target/release/mmo-director \
       /app/target/release/mmo-node \
       /app/target/release/mmo-village-node \
       /app/target/release/mmo-gateway \
       /app/target/release/mmo-loadtest \
       /app/bin/

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy compiled binaries from builder stage
COPY --from=builder /app/bin/mmo-director /usr/local/bin/mmo-director
COPY --from=builder /app/bin/mmo-node /usr/local/bin/mmo-node
COPY --from=builder /app/bin/mmo-village-node /usr/local/bin/mmo-village-node
COPY --from=builder /app/bin/mmo-gateway /usr/local/bin/mmo-gateway
COPY --from=builder /app/bin/mmo-loadtest /usr/local/bin/mmo-loadtest

EXPOSE 50051 50052 8080 9091 4433/udp

CMD ["mmo-director"]
