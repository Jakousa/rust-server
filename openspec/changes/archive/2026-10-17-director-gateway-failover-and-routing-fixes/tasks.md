# Implementation Tasks: Director & Gateway Failover, Octree Boundary Routing & QUIC Interceptors (Issue #102)

- [x] 1. Resolve Registered Node Addresses on Failover & Deregistration (`crates/director`)
  - [x] 1.1 Add `pub previous_node_address: String` and `pub new_node_address: String` to `PartitionRecoveryEvent` in `crates/director/src/coordinator.rs`.
  - [x] 1.2 Update `GlobalOctreeCoordinator::recover_orphaned_partitions` and `DirectorServiceImpl::check_and_recover_dead_nodes` (`crates/director/src/service.rs`) to capture dead node `listen_addr` from `NodeRegistry` before eviction and resolve `new_node_address` from `NodeRegistry::get_node` (with fallback to `mmo-node-{}.mmo-node:50052` only when `listen_addr` is empty).
  - [x] 1.3 Update `DirectorServiceImpl::broadcast_route_reassignments` (`crates/director/src/service.rs:247-259`) to use `ev.previous_node_address` and `ev.new_node_address`.
  - [x] 1.4 Update `DirectorServiceImpl::deregister_node` (`crates/director/src/service.rs:607-653`) to populate recovery addresses and invoke `self.broadcast_route_reassignments(&events, "").await`.
  - [x] 1.5 Add unit tests in `crates/director/src/service.rs` verifying custom `listen_addr` resolution during dead-node failover and graceful deregistration.

- [x] 2. Wire Background Reapers & Environment Federation Config (`crates/director`)
  - [x] 2.1 In `crates/director/src/main.rs`, invoke `svc_clone.check_and_recover_stale_gateways(gateway_timeout).await` and `svc_clone.reap_idle_instances().await` inside the periodic detector loop.
  - [x] 2.2 In `crates/director/src/service.rs`, add `federation_authority_from_env()` reading `REALM_ID` and `FEDERATION_HMAC_SECRET` from environment variables (with safe default fallback) and use it in `new`, `with_gateway_registry`, and `default_service`.
  - [x] 2.3 Add unit test in `crates/director/src/service.rs` verifying `federation_authority_from_env` behavior.

- [x] 3. Fix 3-Axis Octree Boundary Smoothing, Branch Union Bounds, and Remove Unused `spatial_index` (`crates/director/src/coordinator.rs`)
  - [x] 3.1 Remove `pub spatial_index: Octree` from `GlobalOctreeCoordinator` and delete all dummy `Guid::new(EntityType::Player, ...)` insert/remove calls in `coordinator.rs`.
  - [x] 3.2 Update `GlobalOctreeNode::update_leaf_bounds` so that when any child leaf updates its bounds, the parent `Branch::bounds` is recomputed as the bounding box union of all its children.
  - [x] 3.3 Generalize `GlobalOctreeCoordinator::smooth_adjacent_boundaries` to support X, Y, and Z axes and both neighbor orientations (`ba.max ≈ bb.min` and `bb.max ≈ ba.min`).
  - [x] 3.4 Add unit tests in `crates/director/src/coordinator.rs` verifying Y-axis, Z-axis, and reverse-order boundary shifts and asserting `find_node` routes coordinates in the shifted strip to the expanded leaf.

- [x] 4. Fix QUIC Datagram Interceptor Bypass & `active_send_stream` Overwrite (`crates/gateway/src/server.rs`)
  - [x] 4.1 In `GatewayServer::handle_quic_connection` (`crates/gateway/src/server.rs:1716-1739`), add `continue;` after replying to `ClientPacket::Ping` in the datagram loop and call `if intercept_handler.handle_packet(session_id, &packet).await { continue; }` before dispatching.
  - [x] 4.2 In `GatewayServer::handle_quic_connection` (`crates/gateway/src/server.rs:1744-1925`), write `AuthResponse` and stream `Pong` directly to the accepted bidirectional stream's `send` half rather than unconditionally overwriting `*active_send_stream.lock().await = Some(send);` on stream accept; clear `active_send_stream` on write error in `downstream_handle`.
  - [x] 4.3 Add unit/integration tests in `crates/gateway/src/server.rs` verifying QUIC datagram interception and stream isolation.

- [x] 5. Workspace Quality Gates
  - [x] 5.1 Run `cargo fmt --all -- --check`
  - [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 5.3 Run `cargo test --workspace`
