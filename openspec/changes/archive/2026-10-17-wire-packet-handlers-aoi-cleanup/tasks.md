# Implementation Tasks: Wire Missing Packet Handlers, World Object AoI Replication, and Observer Cleanup

- [x] 1. Protobuf Schema & Packet Forwarding Upgrades
  - [x] 1.1 Add `uint64 session_epoch = 4;` to `ForwardClientPacketRequest` in `crates/schema/proto/cluster.proto`.
  - [x] 1.2 Update downstream callers initializing `ForwardClientPacketRequest` in `crates/gateway/src/main.rs:109` and `crates/node/tests/it/aoi_drain_dispatch.rs:151, 182`.
  - [x] 1.3 Wire `session_epoch` check using `sim.is_valid_upstream_epoch(guid, req.session_epoch)` under the write lock in `NodeServiceImpl::forward_client_packet` (`crates/node/src/network.rs`).
  - [x] 1.4 Validate entity existence and migration lease in `forward_client_packet`.
  - [x] 1.5 Handle `PlaceObject` in `forward_client_packet`: match on `handle_place_object` result and return error response on failure.
  - [x] 1.6 Refactor `TickSimulation::handle_interact_object` to take `&mut self` and dispatch `Event::BeforeInteract` (checking `ctx.is_canceled()`) and `Event::OnInteract` with mutable entity access.
  - [x] 1.7 Handle `WorldObjectResync` in `forward_client_packet`: enqueue `AoiBroadcastJob` into `sim.pending_broadcast_jobs` on success.
  - [x] 1.8 Wire `ModularPacketRouter` dispatch for custom plugin packets (`ClientPacket::Interaction`) with anti-spoofing validation in `forward_client_packet`.
  - [x] 1.9 Add unit tests for epoch fencing, placement failure, interaction events, resync queueing, and modular dispatch.

- [x] 2. World Object Spatial AoI Streaming in TickSimulation::step
  - [x] 2.1 In `TickSimulation::step` (`crates/node/src/sim.rs`), invoke `self.aoi.update_world_object_observers(&self.entity_store, &self.sectors)` and append resulting jobs to `self.pending_broadcast_jobs`.
  - [x] 2.2 Add unit test in `sim.rs` verifying that world object spawn/delta jobs are generated during simulation tick.

- [x] 3. Observer Eviction on Session Takeover
  - [x] 3.1 In `AoiManager::register_observer` (`crates/node/src/aoi.rs`), check if `self.guid_to_session` already contains `player_guid`. If an `old_session_id != session_id` exists, remove `old_session_id` from `self.observers`. Also remove old reverse mapping if `session_id` existed.
  - [x] 3.2 Add unit test in `crates/node/src/aoi.rs` verifying that taking over an existing player GUID's session cleanly evicts the old `ObserverState`.

- [x] 4. Complete Entity Despawn and Outbound Migration Cleanup
  - [x] 4.1 Add `SectorManager::clear_rate_limit(&mut self, player_guid: Guid) -> bool` in `crates/node/src/sector.rs`.
  - [x] 4.2 In `TickSimulation::despawn_entity` (`crates/node/src/sim.rs`), purge `self.aoi.unregister_by_guid(guid)`, `self.last_migration_ticks.remove(&guid)`, `self.session_epochs.remove(&guid)`, and `self.sectors.clear_rate_limit(guid)`.
  - [x] 4.3 In outbound migration cleanup (`crates/node/src/network.rs`), ensure all state stores (`kinematic_states`, `aoi`, `last_migration_ticks`, `session_epochs`, `rate_limits`, `instances`) are cleaned up in both `execute_entity_migration` and `execute_batch_entity_migration`.
  - [x] 4.4 Add unit tests verifying complete state cleanup on entity despawn and migration.

- [x] 5. Unified Damage & Effect Pipeline
  - [x] 5.1 Create `apply_combat_effect` helper in `crates/node/src/combat_pipeline.rs` unifying alive check, mitigation, damage/healing, death events, action interrupts, and attribute sync.
  - [x] 5.2 Refactor single-target local cast in `crates/node/src/ability_dispatcher.rs` to validate target alive before resource deduction, and use `apply_combat_effect`.
  - [x] 5.3 Refactor batch AoE local targets in `crates/node/src/ability_dispatcher.rs` to skip dead targets and use `apply_combat_effect`.
  - [x] 5.4 Refactor `execute_remote_ability` in `crates/node/src/network.rs` to use `apply_combat_effect`.
  - [x] 5.5 Add unit tests in `ability_dispatcher.rs` verifying local damage mitigation, death events, and action interrupts.

- [x] 6. Quality Gates & Verification
  - [x] 6.1 Format check: `cargo fmt --all -- --check`.
  - [x] 6.2 Clippy: `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 6.3 Test suite: `cargo test --workspace`.
