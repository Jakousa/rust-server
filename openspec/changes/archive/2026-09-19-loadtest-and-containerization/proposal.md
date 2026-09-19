## Why

While the distributed MMO engine passes unit, integration, and micro-benchmarks, it lacks automated stress load generation against a multi-process cluster and containerized local infrastructure that mirrors production. We need a headless bot swarm generator to evaluate network throughput, boundary handovers, and latency under high concurrent load, packaged inside Docker Compose so the distributed topology can be spun up and verified without manual multi-terminal orchestration.

## What Changes

- Implement `crates/loadtest`: a headless bot swarm simulator that spawns hundreds to thousands of concurrent virtual clients over TCP/binary client framing (`mmo-schema::client`) to the Edge Gateway.
- Implement bot AI routines: random-walk navigation in 3D space, boundary crossing generators to force octant handovers, ping/pong latency monitors, and snapback error counters.
- Provide a rich CLI reporting interface displaying real-time metrics: active connections, tx/rx packet throughput, p50/p95/p99 round-trip latency, and kinematic rejection rates.
- Implement full production service loops and environment variable configuration in `crates/node/src/main.rs` (gRPC worker server, director registration, heartbeat loop, 20 Hz tick loop) and `crates/gateway/src/main.rs` (connection accept loop).
- Create a multi-stage `Dockerfile` optimizing Rust compilation cache and producing slim production images for all cluster services (`mmo-director`, `mmo-node`, `mmo-gateway`, `mmo-loadtest`).
- Create `docker-compose.yml` declaring an isolated bridge network with 1 Director, 2 Simulation Nodes, 1 Edge Gateway, and an on-demand `loadtest` profile.

## Capabilities

### New Capabilities
- `loadtest/runner`: Headless bot swarm orchestrator generating concurrent client traffic, simulating 3D spatial wandering, and reporting performance histograms and error rates.
- `deployment/containerization`: Multi-container topology packaging for cluster components (Director, Nodes, Gateway, Loadtester) with Docker and Docker Compose.

### Modified Capabilities
<!-- None: core server specifications remain normative and unchanged. -->

## Impact

- New workspace crate: `crates/loadtest`.
- Root configuration additions: `Dockerfile`, `.dockerignore`, `docker-compose.yml`.
- Dependencies: `clap` (CLI parsing for loadtest binary), `tokio`, `bytes`, `rand`, `hdrhistogram` (for accurate latency percentiles).
