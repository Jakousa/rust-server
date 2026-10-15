# Drain & Dispatch AoI Broadcast Jobs & Wire Gateway Backhaul Proposal

## Context & Problem Statement
In `crates/node/src/main.rs`, the fixed 20 Hz simulation tick loop executes:
```rust
let mut sim_guard = sim.write().await;
sim_guard.step_tick();
```
Inside `step_tick` (`crates/node/src/sim.rs:627`), Area-of-Interest (AoI) updates, enters, updates, and exits are calculated and queued into `pending_broadcast_jobs`:
```rust
let mut aoi_jobs = self.aoi.update_observers(&self.entity_store, &self.octree, self.tick_counter, now_ms);
self.pending_broadcast_jobs.append(&mut aoi_jobs);
```
However, `sim.drain_broadcast_jobs()` is **never called anywhere in `main.rs`** (it is only called in isolated test fixtures).
As a result:
1. Every tick, `self.pending_broadcast_jobs` accumulates new broadcast jobs indefinitely, creating an unbounded heap memory leak at 20 Hz (1,200 jobs/minute per active observer).
2. Authoritative AoI entity state updates, spawns, and despawns are never actually dispatched from the simulation node daemon to the gateway or clients.
3. In `crates/gateway/src/main.rs`, incoming client packets received by the routed packet worker are dropped with `// In production, forward to destination node via internal channel` instead of being forwarded via `BackhaulPool`.

## Proposed Solution
1. **Node Tick Loop Draining & Memory Leak Fix**:
   - In `crates/node/src/main.rs`, invoke `sim_guard.drain_broadcast_jobs()` on every tick iteration within the catchup loop.
   - If jobs are drained, pass them to an asynchronous dispatcher or delivery channel to prevent blocking the tick loop.

2. **Cluster Delivery Protobuf Interface (`GatewayService`)**:
   - Add `DeliverBroadcastJobs` RPC and messages (`DeliverBroadcastJobsRequest`, `SessionPacketBatch`, `DeliverBroadcastJobsResponse`) to `crates/schema/proto/cluster.proto` on `GatewayService`.
   - Implement `deliver_broadcast_jobs` in `GatewayServiceImpl` (`crates/gateway/src/server.rs`), dispatching encoded binary packets directly to each target session's `SessionSink` (`datagram_tx` for ephemeral deltas, `stream_tx` for reliable packets).

3. **Node AoI Backhaul Delivery Worker**:
   - In `crates/node/src/main.rs`, establish connection pooling / client caching for `GatewayServiceClient` (using `GATEWAY_GRPC_ADDR`, defaulting to `"http://127.0.0.1:50050"`).
   - Offload drained broadcast batches onto a non-blocking queue/worker so the 20 Hz simulation tick loop is never blocked by network I/O.

4. **Gateway Ingress Backhaul Forwarding**:
   - In `crates/gateway/src/main.rs`, update the background packet consumer loop to forward incoming movement, ability, interaction, and client ACK packets to the destination simulation node via `server.backhaul_pool().send_packet(pkt)`.
   - Ensure `backhaul_pool.get_or_create(&pkt.node_address)` initializes the channel and spawns a persistent forwarder task connecting to the node cluster.

## Scope & Non-Goals
- **In Scope**:
  - `crates/node/src/main.rs`: Drain broadcast jobs on every tick; wire gateway delivery worker.
  - `crates/schema/proto/cluster.proto`: Define `DeliverBroadcastJobs` RPC on `GatewayService` and `ForwardClientPacket` RPC on `NodeService`.
  - `crates/gateway/src/server.rs`: Implement `deliver_broadcast_jobs` on `GatewayServiceImpl`.
  - `crates/gateway/src/main.rs`: Forward incoming routed packets via `BackhaulPool`.
  - `crates/node/src/network.rs`: Implement `forward_client_packet` on `NodeServiceImpl`.
  - End-to-end tests validating no memory leak in tick loop and live packet propagation between gateway and node.
- **Non-Goals**:
  - Replacing the underlying gRPC cluster transport with raw RDMA/DPDK.
  - Changing AoI spatial partitioning or sparse octree algorithms.
