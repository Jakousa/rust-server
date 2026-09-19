## Context

The distributed MMO server consists of three core distributed components: `crates/director` (cluster authority), `crates/node` (simulation workers), and `crates/gateway` (edge proxy). Communications use gRPC for internal node-to-node and node-to-director gossip, and custom binary framing (`mmo-schema::client::ClientPacket` with magic bytes `0x4D, 0x4D`) for client-gateway interactions.

## Goals / Non-Goals

**Goals:**
- Provide `crates/loadtest`, a headless virtual client swarm capable of simulating 100 to 2,000 concurrent players with realistic 3D trajectories using the native binary wire format.
- Stress test spatial octree partitioning and dynamic edge route handovers by continuously steering bots across octant division boundaries within configured cluster bounds.
- Measure and display live latency percentiles (p50, p95, p99), packet throughput, and server-side kinematic snap-backs using HDR histograms.
- Fully implement the long-running daemon entrypoints in `crates/node/src/main.rs` (gRPC worker server, director registration, heartbeat loop, 20 Hz tick loop) and `crates/gateway/src/main.rs` (connection accept loop).
- Provide a multi-stage `Dockerfile` producing a minimal Debian-based runtime container containing all compiled server binaries.
- Provide `docker-compose.yml` defining an isolated multi-service cluster (`director`, `node-1`, `node-2`, `gateway`, and optional `loadtest`).

**Non-Goals:**
- Graphical client visualization or rendering pipelines (loadtester is purely headless).
- FlatBuffers migration (retains existing high-performance custom binary wire framing).
- Kubernetes Helm charts or production cloud Terraform configurations (Docker Compose is targeted for local production-fidelity testing).

## Decisions

### Decision 1: Single Unified Container Image with Multiple Entrypoints
- **Chosen**: A single multi-stage Docker image packaging `mmo-director`, `mmo-node`, `mmo-gateway`, and `mmo-loadtest`.
- **Rationale**: Building the workspace once in a shared build stage avoids repeating heavy cargo compilations for four separate container images. Individual services in `docker-compose.yml` invoke their specific binary via `command: ["mmo-director"]`, etc.
- **Alternatives Considered**: Four independent Dockerfiles (rejected: excessive build redundancy and cache duplication).

### Decision 2: HDR Histogram for High-Resolution Tail Latency
- **Chosen**: Use the `hdrhistogram` crate in `crates/loadtest` for ping/pong RTT tracking.
- **Rationale**: High Dynamic Range histograms record sub-millisecond precision with fixed memory overhead and accurate p95/p99 tail reporting, avoiding unbounded Vec allocation during heavy load runs.
- **Alternatives Considered**: Simple average latency or rolling circular buffers (rejected: obscures tail latency spikes).

### Decision 3: Synthetic Random-Walk & Dynamic World Bounds
- **Chosen**: Each bot executes a random-walk wander steering towards target waypoints scaled to the cluster's bounding box (e.g. crossing coordinate zero / boundary planes).
- **Rationale**: Keeps entity movements safely inside global simulation bounds while consistently forcing octant boundary crossings to exercise the edge gateway's session route handovers.

### Decision 4: Native ClientPacket Binary Protocol
- **Chosen**: Loadtest client directly encodes and decodes `ClientPacket` using `mmo_schema::client::ClientPacket::encode` and `decode_frame`.
- **Rationale**: Eliminates protocol mismatch and guarantees 100% wire compatibility with `crates/gateway`.

## Risks / Trade-offs

- **[Risk: Docker compilation duration]** → **Mitigation**: Use layer caching in `Dockerfile` copying manifests (`Cargo.toml`, `Cargo.lock`) and building dependency skeletons first.
- **[Risk: OS Socket / File Descriptor Exhaustion]** → **Mitigation**: Support CLI parameters (`--bots`, `--rate-ms`, `--stagger-ms`) in `crates/loadtest` to smoothly ramp up connections and avoid SYN flood rejections.
