# Implementation Tasks: Drain & Dispatch AoI Broadcast Jobs & Wire Gateway Backhaul

- [x] 1. Schema gRPC Additions for Gateway & Node Services
  - [x] 1.1 Add `DeliverBroadcastJobs` RPC and messages (`SessionPacketBatch`, `DeliverBroadcastJobsRequest`, `DeliverBroadcastJobsResponse` with `disconnected_sessions`) to `GatewayService` in `crates/schema/proto/cluster.proto`.
  - [x] 1.2 Add `ForwardClientPacket` RPC and messages (`ForwardClientPacketRequest`, `ForwardClientPacketResponse`) to `NodeService` in `crates/schema/proto/cluster.proto`.
  - [x] 1.3 Verify schema compilation and code generation with `cargo check -p mmo-schema`.

- [x] 2. Gateway Ingress & Egress Implementation
  - [x] 2.1 Implement `deliver_broadcast_jobs` in `GatewayServiceImpl` (`crates/gateway/src/server.rs`) forwarding to `SessionSink` and populating `disconnected_sessions`.
  - [x] 2.2 Wire `crates/gateway/src/main.rs` packet receiver loop to forward packets to destination nodes via `BackhaulPool`.
  - [x] 2.3 Implement persistent backhaul channel workers in `crates/gateway/src/main.rs` dispatching `ForwardClientPacket` gRPC calls to nodes with `record_success()` / `record_failure()`.
  - [x] 2.4 Verify with `cargo test -p mmo-gateway`.

- [x] 3. Node Ingress & Authoritative Tick Loop Draining
  - [x] 3.1 Implement `forward_client_packet` on `NodeServiceImpl` in `crates/node/src/network.rs` with payload length checks, spoofing checks, and `MovementCorrection` snapback generation on move rejection.
  - [x] 3.2 Update `crates/node/src/main.rs` 20 Hz tick loop to call `sim_guard.drain_broadcast_jobs()` on every tick.
  - [x] 3.3 Add asynchronous decoupled broadcast dispatcher worker in `crates/node/src/main.rs` delivering batches to `GatewayServiceClient` and handling `disconnected_sessions` cleanup.
  - [x] 3.4 Verify with `cargo test -p mmo-node`.

- [x] 4. Integration Tests & Quality Gates
  - [x] 4.1 Add integration test verifying tick loop memory remains bounded and drained jobs arrive at gateway session sinks.
  - [x] 4.2 Run `cargo fmt --all -- --check`.
  - [x] 4.3 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 4.4 Run `cargo test --workspace`.
