# Implementation Tasks: Graceful Shutdown, Gateway Discovery & Operations

- [x] 1. Core Signal Handling and Protobuf Schema RPCs
  - [x] 1.1 Implement `mmo_core::shutdown_signal()` in `crates/core/src/lib.rs` supporting Ctrl+C and Unix SIGTERM.
  - [x] 1.2 Add `DeregisterNodeRequest`, `DeregisterNodeResponse`, `DeregisterGatewayRequest`, and `DeregisterGatewayResponse` messages and RPCs to `crates/schema/proto/cluster.proto`.
  - [x] 1.3a Implement `GatewayRegistry::deregister_gateway(&mut self, gateway_id: &str) -> Option<GatewayDescriptor>` in `crates/director/src/registry.rs`.
  - [x] 1.3b Implement `deregister_node` (with partition recovery & instance eviction under lock order `instance_registry` -> `registry` -> `coordinator`) and `deregister_gateway` in `DirectorServiceImpl` (`crates/director/src/service.rs`).
  - [x] 1.4 Add unit tests for `deregister_node` and `deregister_gateway` in `crates/director/src/service.rs` and `crates/director/src/registry.rs`.
  - [x] 1.5 Verify with `cargo test -p mmo-core` and `cargo test -p mmo-schema` and `cargo test -p mmo-director`.

- [x] 2. Dynamic Gateway Discovery in Director Recovery Loop
  - [x] 2.1 Update `crates/director/src/main.rs` recovery loop to read active gateways from `svc_clone.gateway_registry`.
  - [x] 2.2 Broadcast `ReassignNodeRoutesRequest` concurrently across all registered active gateways with a 2-second timeout per gateway, falling back to `GATEWAY_GRPC_ADDR` if empty.
  - [x] 2.3 Wire graceful shutdown to `DirectorServiceServer` via tonic's `serve_with_shutdown`.
  - [x] 2.4 Add unit / integration tests in `crates/director` verifying multi-gateway route broadcast.
  - [x] 2.5 Verify with `cargo test -p mmo-director`.

- [x] 3. Graceful Shutdown & Snapshot Draining in Node Daemon
  - [x] 3.1 Refactor `crates/node/src/main.rs` tick loop to run with signal cancellation (`tokio::select!`).
  - [x] 3.2 Implement orderly node draining sequence:
    - Mark telemetry unready (`telemetry.mark_unready()`).
    - Abort background heartbeat task (`heartbeat_handle.abort()`).
    - Shutdown snapshot worker, await worker handle with a 5-second timeout, and call `snapshotter.flush_batch().await`.
    - Call Director `deregister_node` with a bounded 2-second timeout (proceed on timeout or error).
    - Wire gRPC `NodeServiceServer` to shutdown with tonic `serve_with_shutdown`.
  - [x] 3.3 Add tests verifying node clean shutdown and snapshot flushing.
  - [x] 3.4 Verify with `cargo test -p mmo-node`.

- [x] 4. Graceful Shutdown & Session Draining in Gateway Daemon
  - [x] 4.1 Update `crates/gateway/src/main.rs` TCP accept loop to select on shutdown signal / cancellation token.
  - [x] 4.2 Mark telemetry unready (`telemetry.mark_unready()`), abort gateway heartbeat task (`heartbeat_handle.abort()`), and call Director `deregister_gateway` with a 2-second timeout.
  - [x] 4.3 Implement session draining grace period and sink termination.
  - [x] 4.4 Add tests in `crates/gateway` for shutdown draining behavior.
  - [x] 4.5 Verify with `cargo test -p mmo-gateway`.

- [x] 5. Tracing Spans across Ingress Pipeline & Quality Gates
  - [x] 5.1 Add `tracing` spans to client packet forwarding in `crates/gateway/src/main.rs` and packet handling in `crates/node/src/service.rs`.
  - [x] 5.2 Run workspace formatting: `cargo fmt --all -- --check`.
  - [x] 5.3 Run workspace linter: `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 5.4 Run workspace tests: `cargo test --workspace`.
