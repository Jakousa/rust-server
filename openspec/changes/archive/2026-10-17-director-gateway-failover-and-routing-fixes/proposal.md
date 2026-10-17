# Director Failover Address Resolution, Background Reapers, Octree Boundary Routing & QUIC Interceptors Proposal

## Context & Problem Statement
Auditing `crates/director` and `crates/gateway` (Issue #102) uncovered several critical cluster coordination, spatial routing, and transport handling bugs:

1. **Hardcoded Pod DNS on Node Failover (`crates/director/src/service.rs:247-259`, `registry.rs:142-162`)**:
   - When `NodeRegistry::check_dead_nodes` evicts expired nodes, it discards their registered `listen_addr` and returns only `Vec<u32>`.
   - `DirectorServiceImpl::broadcast_route_reassignments` then synthesizes hardcoded Kubernetes StatefulSet DNS strings (`format!("mmo-node-{}.mmo-node:50052", ev.previous_node_id.saturating_sub(1))`) instead of using the actual registered addresses from `NodeRegistry`.
   - Additionally, `DirectorServiceImpl::deregister_node` (`service.rs:607-653`) recovers orphaned partitions on graceful node deregistration without broadcasting route reassignments to active gateways.

2. **Missing Background Reapers & Hardcoded Federation Secret (`crates/director/src/main.rs:24-56`, `service.rs:66-135`)**:
   - `DirectorServiceImpl::reap_idle_instances()` (`service.rs:195`) and `DirectorServiceImpl::check_and_recover_stale_gateways()` (`service.rs:206`) are never invoked in `crates/director/src/main.rs`'s background detector loop.
   - `DirectorServiceImpl` constructors hardcode `"realm-default"` and `b"cluster-realm-federation-default-secret-key"` instead of reading `REALM_ID` and `FEDERATION_HMAC_SECRET` from environment variables.

3. **One-Way X-Axis Boundary Smoothing, Branch Bounds Desync, and Unused `spatial_index` (`crates/director/src/coordinator.rs`)**:
   - `smooth_adjacent_boundaries` (`lines 646-707`) only checks one-way X-axis adjacency (`(ba.max.0 - bb.min.0).abs() < 0.001`), ignoring `bb.max.0 ≈ ba.min.0` as well as Y-axis and Z-axis adjacent partitions.
   - `GlobalOctreeNode::update_leaf_bounds` (`lines 329-355`) mutates leaf `BoundingBox` without updating parent `Branch::bounds` to enclose shifted children, which can cause `GlobalOctreeNode::find_node` (`lines 115-130`) to reject points that fell into an expanded leaf across branch boundaries.
   - `GlobalOctreeCoordinator::spatial_index: Octree` (`line 394`) inserts dummy `Guid::new(EntityType::Player, ...)` markers and is never queried.

4. **QUIC Datagram Interceptor Bypass & `active_send_stream` Overwrite (`crates/gateway/src/server.rs:1713-1755`)**:
   - In `handle_quic_connection`, the QUIC datagram loop handles `ClientPacket::Ping` without `continue;` (so `Ping` is forwarded to simulation nodes) and never calls `intercept_handler.handle_packet(session_id, &packet).await` (bypassing social/marketplace interceptors).
   - Every accepted bidirectional QUIC stream (`line 1747`) unconditionally executes `*active_send_stream.lock().await = Some(send);`, overwriting the existing server-to-client send stream even prior to authentication.

## Proposed Solution

1. **Registered Node Address Resolution & Graceful Deregistration Broadcast**:
   - Record dead node `listen_addr` values before eviction in `NodeRegistry` and populate `previous_node_address` and `new_node_address` on `PartitionRecoveryEvent` (falling back to `mmo-node-{}.mmo-node:50052` only when a registered address is empty).
   - Broadcast route reassignments in `DirectorServiceImpl::deregister_node` when partitions are reassigned.

2. **Background Reapers & Environment-Driven Federation Authority**:
   - Invoke `check_and_recover_stale_gateways` and `reap_idle_instances` inside `crates/director/src/main.rs`'s background detector loop.
   - Read `REALM_ID` and `FEDERATION_HMAC_SECRET` from environment variables when initializing `RealmFederationAuthority` in `DirectorServiceImpl`, with safe fallback for unit tests.

3. **3-Axis Bidirectional Boundary Smoothing & Exact Octree Branch Bounds**:
   - Generalize `smooth_adjacent_boundaries` across all 3 axes (`X`, `Y`, `Z`) and both adjacency directions (`ba.max ≈ bb.min` and `bb.max ≈ ba.min`).
   - Update `GlobalOctreeNode::update_leaf_bounds` so parent `Branch::bounds` is recomputed as the bounding union of its children whenever a child's bounds change.
   - Remove the unused `spatial_index: Octree` field and dummy `EntityType::Player` GUID insertions from `GlobalOctreeCoordinator`.

4. **QUIC Datagram Interceptor & Safe Stream Handling**:
   - Add `continue;` after `ClientPacket::Ping` in the QUIC datagram loop and invoke `if intercept_handler.handle_packet(session_id, &packet).await { continue; }`.
   - Write per-stream responses (`AuthResponse`, stream `Pong`) directly to the accepted bidirectional stream's `send` half, and only store a stream into `active_send_stream` if it is currently `None` and the session is authenticated.

## Scope & Non-Goals
- **In Scope**: `crates/director/src/{main.rs, service.rs, registry.rs, coordinator.rs}`, `crates/gateway/src/server.rs`, and unit tests.
- **Non-Goals**: Changing gRPC protobuf wire schemas or adding game-specific routing rules (strictly adheres to `AGENTS.md` Prime Directive).
