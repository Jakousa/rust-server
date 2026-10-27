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

# Build release binaries for all workspace members
RUN cargo build --release --bins

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy compiled binaries from builder stage
COPY --from=builder /app/target/release/mmo-director /usr/local/bin/mmo-director
COPY --from=builder /app/target/release/mmo-node /usr/local/bin/mmo-node
COPY --from=builder /app/target/release/mmo-gateway /usr/local/bin/mmo-gateway
COPY --from=builder /app/target/release/mmo-loadtest /usr/local/bin/mmo-loadtest

EXPOSE 50051 50052 8080 9091 4433/udp

CMD ["mmo-director"]
