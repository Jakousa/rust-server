# Implementation Tasks: Node, Gateway & Core Modularity, Performance & Loadtest Hardening (Issues #100, #101, #105)

- [ ] 1. `crates/core` Performance, Ray-AABB Consolidation, Collision Layers & Error Hardening
  - [ ] 1.1 Refactor `AttributeSet::tick` (`crates/core/src/attribute.rs`) to compute effective max without allocating `Vec<String>`.
  - [ ] 1.2 Refactor `EventQueue` (`crates/core/src/plugin.rs`) to bucket events by `TypeId` (`HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>`) and log warnings in `ComponentCodecRegistry` on codec failures.
  - [ ] 1.3 Implement `BoundingBox3D::intersect_ray_with_normal` in `crates/core/src/octree.rs` and delegate `BoundingBox3D::intersects_ray`, `AabbCollisionWorld::ray_intersect_box`, and `spatial::ray_intersects_aabb` to it.
  - [ ] 1.4 Add `layer_mask: u32` (`register_obstacle_with_layers`) to `AabbCollisionWorld` (`crates/core/src/collision.rs`) and enforce `SpatialQueryFilter::collision_layers` in `raycast`, `sweep_test`, and `is_point_traversable`.
  - [ ] 1.5 Replace `.expect(...)` in `TelemetryServer::init` (`crates/core/src/telemetry.rs`) with `TelemetryError::RecorderInit`.

- [ ] 2. `crates/node` Modularity Split, gRPC Connection Pool, Snapshotter & History Deduplication
  - [ ] 2.1 Split `crates/node/src/network.rs` into `crates/node/src/network/{mod.rs, client_pool.rs, ghost.rs, migration.rs, service.rs}` and unify single/batch entity migration ingestion and coordination.
  - [ ] 2.2 Wire `GrpcClientPool` into `GhostBroadcaster`, `AbilityDispatcher`, and entity migration.
  - [ ] 2.3 Add `QueuedClientInput` MPSC ingress channel to `TickSimulation` (`crates/node/src/sim.rs`) drained in `TickSimulation::step`, and pass a lazy iterator directly to `self.history.push_frame` without `.collect::<Vec<_>>()`.
  - [ ] 2.4 Deduplicate `interpolate_scene` / `interpolate_scene_ms` and `raycast_historical` / `raycast_historical_ms` in `crates/node/src/history.rs`.
  - [ ] 2.5 Fix the double `self.notify.notify_one()` in `WriteBehindSnapshotter` (`crates/node/src/snapshotter.rs`) using `stage_dirty_without_notify`.

- [ ] 3. `crates/gateway` Modularity Split & Unified Auth Handshake
  - [ ] 3.1 Split `crates/gateway/src/server.rs` into `crates/gateway/src/server/{mod.rs, dispatcher.rs, sink.rs, intercept.rs, grpc.rs, auth_handshake.rs}`.
  - [ ] 3.2 Unify TCP, QUIC, and WebTransport authentication and session registration via `auth_handshake.rs`.

- [ ] 4. `crates/loadtest` & `crates/schema` Hardening
  - [ ] 4.1 Add monotonic `sequence: u32` to `BotSession` (`crates/loadtest/src/session.rs`), send `ClientAckPacket` on `EntityDeltaBatch` in `SwarmManager::run` (`swarm.rs`), and remove recursion from `TrajectoryController::step` (`trajectory.rs`).
  - [ ] 4.2 Add `put_u16_str` helper and minimum remaining buffer checks in `EntityDeltaBatchPacket::decode` and `get_property_map` (`crates/schema/src/client.rs`).
  - [ ] 4.3 Add unit tests covering `EventQueue` `TypeId` draining, `AabbCollisionWorld` layer filtering, `TrajectoryController` small-bound steps, and `client.rs` bounds validation.

- [ ] 5. Workspace Quality Gates
  - [ ] 5.1 Run `cargo fmt --all -- --check`
  - [ ] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 5.3 Run `cargo test --workspace`
