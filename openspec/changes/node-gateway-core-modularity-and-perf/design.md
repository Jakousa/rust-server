# Technical Design: Node, Gateway & Core Modularity, Performance & Loadtest Hardening

## 1. `crates/node` Modularity & Performance

### 1.1 Split `crates/node/src/network.rs` into `crates/node/src/network/`
- `crates/node/src/network/mod.rs`: Shared types (`EntityVelocity`, `RemoteCommandErrorCode`, `NetworkError`), `pub use` re-exports of all public items so external callers require zero import changes, and `#[cfg(test)] mod tests`.
- `crates/node/src/network/client_pool.rs`:
  ```rust
  #[derive(Clone, Default, Debug)]
  pub struct GrpcClientPool {
      channels: Arc<DashMap<String, tonic::transport::Channel>>,
  }
  ```
  - `get_channel(&self, endpoint: &str) -> Result<Channel, tonic::transport::Error>`
  - `invalidate(&self, endpoint: &str)`
  - Wire `GrpcClientPool` into `GhostBroadcaster`, `AbilityDispatcher`, `execute_entity_migration`, and `execute_batch_entity_migration`.
- `crates/node/src/network/ghost.rs`: `GhostRecord`, `GhostStore`, `BroadcastJob`, `GhostBroadcaster`.
- `crates/node/src/network/migration.rs`: `MigrationError`, `BatchMigrationReport`, `ingest_migrated_entity` (shared by `migrate_entity` and `migrate_batch_entities`), `execute_batch_entity_migration`, and `execute_entity_migration` (delegating to shared batch/phase helpers).
- `crates/node/src/network/service.rs`: `NodeServiceImpl`, `extract_initiator`, `make_remote_command`, `execute_remote_ability`.

### 1.2 MPSC Client Input Queue & Zero-Alloc `SpatialHistoryBuffer::push_frame` (`crates/node/src/sim.rs`)
- Add `QueuedClientInput { pub session_id: u64, pub entity_guid: u64, pub session_epoch: u64, pub packet: ClientPacket }` channel (`input_sender`, `input_receiver`) to `TickSimulation`, with `queue_client_input` and `drain_client_inputs(&mut self)` invoked at the start of `TickSimulation::step`.
- In `TickSimulation::step` (`crates/node/src/sim.rs:702-737`):
  - Destructure `let Self { ref entity_store, ref migrating_leases, ref mut history, tick_counter, current_time, .. } = *self;` so the lazy `entity_store.iter().filter_map(...)` iterator is passed directly into `history.push_frame(tick_counter, current_time, now_ms, iter)` without `.collect::<Vec<_>>()`.

### 1.3 Deduplicate `history.rs` & Fix Double `notify_one()` in `snapshotter.rs`
- In `crates/node/src/history.rs`:
  - Extract `fn interpolate_frames(f_a: &HistoricalTickFrame, f_b: &HistoricalTickFrame, alpha: f32) -> Vec<HistoricalEntityRecord>` and `fn raycast_interpolated_scene(...) -> Option<HistoricalHitResult>` used by both the `Instant` and `_ms` methods.
- In `crates/node/src/snapshotter.rs`:
  - Extract `fn stage_dirty_without_notify(&self, guid: Guid, snapshot: EntitySnapshot) -> bool`.
  - Remove `self.notify.notify_one()` from `enqueue_snapshot` (`line 128`) and call `self.stage_dirty_without_notify(guid, snapshot)` inside `start_worker`'s `channel.recv()` loop before flushing so `self.notify` is not left with a stale permit.

---

## 2. `crates/gateway` Modularity & Unified Auth Handshake

- Split `crates/gateway/src/server.rs` into `crates/gateway/src/server/{mod.rs, dispatcher.rs, sink.rs, intercept.rs, grpc.rs, auth_handshake.rs}` with full `pub use` re-exports in `server/mod.rs`.
- In `server/auth_handshake.rs`, implement `authenticate_and_register_session` shared across TCP (`handle_connection`), QUIC (`handle_quic_connection`), and WebTransport (`webtransport.rs:handle_h3_session`), ensuring all 3 transports consistently resolve initial node via `resolve_initial_node`, register the session in `router`, write to `cache.set_session`, subscribe to `"chat:global"` and `"whisper:{guid}"`, and encode `AuthResponsePacket`.

---

## 3. `crates/core` Performance, Collision & Error Hardening

1. **`AttributeSet::tick` (`crates/core/src/attribute.rs:184-196`)**:
   - Extract `fn compute_effective_max_for(name: &str, attr: &AttributeValue, modifiers: &[StatModifier]) -> f32` and iterate `self.attributes.iter_mut()` directly while borrowing `&self.modifiers` immutably.
2. **`EventQueue` (`crates/core/src/plugin.rs:230-277`)**:
   - Store `queues: HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>` so `drain::<E>()` removes `self.queues.remove(&TypeId::of::<E>())` in $O(1)$ lookup without touching other event types.
3. **Unified Ray-AABB Slab Math & `collision_layers` (`crates/core/src/{octree.rs, collision.rs, spatial.rs}`)**:
   - Add `pub fn intersect_ray_with_normal(&self, origin: (f32, f32, f32), dir: (f32, f32, f32), max_dist: f32) -> Option<(f32, (f32, f32, f32))>` to `BoundingBox3D` in `octree.rs` (normalizing `dir` and computing surface normal cleanly even when origin is inside the AABB).
   - Delegate `BoundingBox3D::intersects_ray`, `AabbCollisionWorld::ray_intersect_box`, and `spatial::ray_intersects_aabb` to `intersect_ray_with_normal`.
   - In `AabbCollisionWorld` (`collision.rs`), store `ObstacleRecord { bounds: BoundingBox3D, layer_mask: u32 }` (with `register_obstacle_with_layers` and default `u32::MAX`) and enforce `(filter.collision_layers & record.layer_mask) != 0` in `raycast`, `sweep_test`, and `is_point_traversable`.
4. **`TelemetryServer::init` & `ComponentCodecRegistry`**:
   - In `crates/core/src/telemetry.rs:45-56`, initialize `RECORDER_HANDLE` fallibly, mapping `install_recorder()` failure to `Err(TelemetryError::RecorderInit(e.to_string()))` instead of `.expect(...)`.
   - In `crates/core/src/plugin.rs:153-180`, log a `tracing::warn!` when component serialization/deserialization fails instead of silently dropping.

---

## 4. `crates/loadtest` & `crates/schema/src/client.rs` Hardening

1. **`BotSession` & `SwarmManager` (`crates/loadtest/src/{session.rs, swarm.rs, trajectory.rs}`)**:
   - Add `sequence: u32` to `BotSession`, incrementing on each `send_movement`.
   - In `SwarmManager::run`, increment per-bot movement `sequence` on every tick and send `ClientPacket::ClientAck` whenever `ClientPacket::EntityDeltaBatch` is received.
   - In `TrajectoryController::step` (`trajectory.rs:171-174`), replace recursive `return self.step(dt_seconds);` with a single target refresh and guard `pick_new_target` / `--tick-rate` against empty or zero ranges.
2. **Checked String Encoding & Varint Loop Bounds (`crates/schema/src/client.rs`)**:
   - Add `fn put_u16_str(dst: &mut BytesMut, s: &str)` clamping byte length to `<= u16::MAX as usize` (on a UTF-8 char boundary) and use it across all 10 packet encoders.
   - In `EntityDeltaBatchPacket::decode` (`lines 1014-1025`) and `get_property_map` (`line 1096`), validate `despawn_len <= src.remaining() / 8`, `delta_len <= src.remaining() / 9`, and `count <= src.remaining() / 2` before looping.
