# Node, Gateway & Core Modularity, Tick Performance, Error Hardening & Loadtest Wiring Proposal

## Context & Problem Statement
Auditing `crates/node`, `crates/gateway`, `crates/core`, `crates/schema`, and `crates/loadtest` (Issues #100, #101, #105) identified critical tick-loop performance bottlenecks, multi-thousand-line god modules with duplicated logic, and bypassed loadtest/error abstractions:

1. **Tick-Loop Contention, Ephemeral gRPC Channels & Per-Tick Heap Allocations (Issue #100)**:
   - `NodeServiceImpl::forward_client_packet` (`crates/node/src/network.rs:888`) acquires `self.sim.write().await` on every client packet instead of offering an MPSC ingress queue drained at the start of `TickSimulation::step`.
   - `crates/node` opens ephemeral TCP/HTTP2 gRPC connections (`NodeServiceClient::connect`, `GatewayServiceClient::connect`) on every 20Hz ghost broadcast (`GhostBroadcaster::send_sync_to_endpoint`), remote ability RPC (`AbilityDispatcher`), and entity migration (`execute_entity_migration`).
   - `TickSimulation::step` (`crates/node/src/sim.rs:702-731`) allocates a fresh `Vec<HistoricalEntityRecord>` every tick before calling `self.history.push_frame`.
   - `AttributeSet::tick` (`crates/core/src/attribute.rs:186`) clones all attribute keys into a `Vec<String>` every tick per entity.
   - `EventQueue::drain` (`crates/core/src/plugin.rs:246-261`) scans a flat `Vec<Box<dyn Any + Send + Sync>>` and reallocates a `remaining` vector on every drain call instead of indexing by `TypeId`.
   - `AabbCollisionWorld` (`crates/core/src/collision.rs:280, 341, 394`) performs an $O(N)$ linear scan and ignores `SpatialQueryFilter::collision_layers`.

2. **God Modules & Duplicated Migration, History, Auth, Snapshotter, and Raycast Logic (Issue #101)**:
   - `crates/node/src/network.rs` (**3,334 lines**) and `crates/gateway/src/server.rs` (**2,859 lines**) are monolithic god files.
   - TCP (`server.rs:1445`), QUIC (`server.rs:1794`), and WebTransport (`webtransport.rs:354`) copy-paste ~100 lines of JWT verification, route resolution, cache writes, social subscriptions, and `AuthResponsePacket` framing.
   - `crates/node/src/network.rs` duplicates entity ingestion (`migrate_entity` vs `migrate_batch_entities`) and Two-Phase Migration coordination (`execute_entity_migration` vs `execute_batch_entity_migration`).
   - `crates/node/src/history.rs` duplicates `interpolate_scene` vs `interpolate_scene_ms` and `raycast_historical` vs `raycast_historical_ms`.
   - `WriteBehindSnapshotter::enqueue_snapshot` (`snapshotter.rs:128`) and `mark_dirty` inside `start_worker` (`lines 229, 236, 251`) both call `self.notify.notify_one()`, causing a redundant empty flush cycle after every batch.
   - Ray-AABB slab intersection is implemented 3 separate times (`octree.rs:85`, `collision.rs:173`, `spatial.rs:279`).

3. **Loadtest `BotSession` Bypass, Recursive Trajectory Stack Overflow & Wire/Error Hardening (Issue #105)**:
   - `SwarmManager::run` (`crates/loadtest/src/swarm.rs:101-213`) bypasses `BotSession`, never sends `ClientAckPacket` on `EntityDeltaBatch`, and `BotSession::send_movement` (`session.rs:93`) hardcodes `sequence: 1`.
   - `TrajectoryController::step` (`crates/loadtest/src/trajectory.rs:171-174`) recurses infinitely when `hotspot_radius < 2.0` or bounds `< 2.0`.
   - `crates/schema/src/client.rs` casts string lengths `as u16` without clamping/checking `u16::MAX` and loops over `despawn_len`/`delta_len`/`get_property_map` without validating minimum remaining buffer bytes.
   - `TelemetryServer::init` (`crates/core/src/telemetry.rs:49`) calls `.expect(...)` instead of returning `TelemetryError::RecorderInit`, and `ComponentCodecRegistry::serialize_all` silently swallows serialization errors.

## Proposed Solution

1. **Modularity Split & Deduplication**:
   - Split `crates/node/src/network.rs` into `crates/node/src/network/{mod.rs, client_pool.rs, ghost.rs, migration.rs, service.rs}` and unify single/batch migration.
   - Split `crates/gateway/src/server.rs` into `crates/gateway/src/server/{mod.rs, dispatcher.rs, sink.rs, intercept.rs, grpc.rs, auth_handshake.rs}` and unify TCP/QUIC/WebTransport session authentication.
   - Consolidate Ray-AABB intersection onto `BoundingBox3D::intersect_ray_with_normal` in `crates/core/src/octree.rs`, deduplicate `history.rs` interpolation/raycast helpers, and fix the double `notify_one()` in `snapshotter.rs`.

2. **High-Frequency Tick & RPC Performance**:
   - Add `GrpcClientPool` (`DashMap<String, tonic::transport::Channel>`) in `crates/node/src/network/client_pool.rs` and wire it into `GhostBroadcaster`, `AbilityDispatcher`, and migration.
   - Add `QueuedClientInput` MPSC ingress channel to `TickSimulation` drained at the start of `TickSimulation::step`.
   - Split struct borrows in `TickSimulation::step` (`sim.rs:702`) and `AttributeSet::tick` (`attribute.rs:186`) to eliminate per-tick `Vec` allocations.
   - Bucket `EventQueue` by `TypeId` (`HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>`) in `crates/core/src/plugin.rs`.
   - Enforce `SpatialQueryFilter::collision_layers` and add spatial grid cell acceleration in `AabbCollisionWorld`.

3. **Loadtest, Schema & Error Hardening**:
   - Increment `BotSession::sequence` monotonically, send `ClientAckPacket` on `EntityDeltaBatch` in `SwarmManager::run`, and make `TrajectoryController::step` non-recursive with safe RNG bounds.
   - Add `put_u16_str` and minimum-remaining-byte checks in `crates/schema/src/client.rs`.
   - Replace `.expect()` in `TelemetryServer::init` with `TelemetryError::RecorderInit` and log component codec errors in `ComponentCodecRegistry`.

## Scope & Non-Goals
- **In Scope**: `crates/node`, `crates/gateway`, `crates/core`, `crates/schema`, `crates/loadtest`.
- **Non-Goals**: Changing gameplay rules or external client wire protocols.
