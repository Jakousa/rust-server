# Wire Missing Packet Handlers, World Object AoI Replication, and Observer Cleanup Proposal

## Context & Problem Statement
Code review of simulation and networking pathways in `crates/node` (Issue #99) identified critical omissions and memory/entity state leaks across live tick processing, packet routing, and reconnection/migration pathways:

1. **Discarded Packet Results & Unwired `ModularPacketRouter` in `NodeServiceImpl::forward_client_packet` (`crates/node/src/network.rs:839-968`)**:
   - `PlaceObject`: `sim.handle_place_object(guid, &po)` discards `Result<Guid, PlacementError>` and always returns `success: true`.
   - `InteractObject`: `sim.handle_interact_object(guid, &io)` discards the validation result and never dispatches `Event::BeforeInteract` or `Event::OnInteract` to plugins.
   - `WorldObjectResync`: `sim.handle_world_object_resync(guid, &resync)` returns `Option<WorldObjectSpawnPacket>`, which is dropped instead of being queued in `sim.pending_broadcast_jobs`.
   - Catch-all `_ => Ok(...)`: Custom plugin packets are silently ignored instead of being dispatched through `sim.packets` (`ModularPacketRouter`).
   - `TickSimulation::is_valid_upstream_epoch` is defined to fence stale packets after session handover, but is never called in `forward_client_packet`.

2. **World Object AoI Replication Never Invoked in `TickSimulation::step` (`crates/node/src/sim.rs:624-627`)**:
   - `AoiManager::update_world_object_observers` implements spatial sector visibility diffing for world objects (`WorldObjectSpawn`, `WorldObjectStateDelta`, `WorldObjectDespawn`), but `TickSimulation::step` only invokes `update_observers` and never invokes `update_world_object_observers`.

3. **Orphaned Observer Leak on Session Takeover (`crates/node/src/aoi.rs:390-399`)**:
   - In `AoiManager::register_observer`, when a `player_guid` already has a session in `guid_to_session`, the old `session_id` is overwritten without removing it from `self.observers`. The orphaned `ObserverState` continues running octree queries and generating broadcast packets every tick forever.

4. **Entity Despawn & Migration Cleanup Leaks (`crates/node/src/sim.rs:482-497`, `crates/node/src/sector.rs:85`, `crates/node/src/network.rs`)**:
   - `TickSimulation::despawn_entity` and both single & batch outbound entity migration remove the entity from `entity_store`, `octree`, `instances`, and `plugin_context`, but fail to clean up `self.aoi.unregister_by_guid(guid)`, `self.last_migration_ticks.remove(&guid)`, `self.session_epochs.remove(&guid)`, or `SectorManager::rate_limits`.

5. **Inconsistent Local vs. Remote Damage Pipeline (`crates/node/src/ability_dispatcher.rs:212-226, 417-434` vs `crates/node/src/network.rs:1069-1118`)**:
   - Local hits in `ability_dispatcher.rs` apply unmitigated base damage directly to `HealthComponent::take_damage` without checking `StatsComponent::mitigate_damage`, without triggering `ActionQueue` damage interrupts, without syncing `AttributeSet`, and without emitting `EntityDiedEvent`. Furthermore, target death is not checked prior to resource deduction in single-target abilities.

## Proposed Solution

1. **Robust Packet Forwarding & Routing**:
   - Add `uint64 session_epoch = 4;` to `ForwardClientPacketRequest` in `crates/schema/proto/cluster.proto`, updating gateway and integration test callers.
   - In `forward_client_packet`, check `sim.is_valid_upstream_epoch(guid, req.session_epoch)` unconditionally under the write lock, rejecting stale packets.
   - Enforce anti-spoofing (`req.guid == interact.source_guid`) and verify entity existence and absence of active migration lease.
   - Match and handle `PlaceObject`: return `success: false` with the error message on `PlacementError`.
   - Update `handle_interact_object` to take `&mut self` and dispatch `Event::BeforeInteract` (with veto handling) and `Event::OnInteract` via `self.hooks.dispatch_with_entities`.
   - On `WorldObjectResync`, if `Some(spawn)` is returned, enqueue an `AoiBroadcastJob` into `sim.pending_broadcast_jobs`.
   - In `ClientPacket::Interaction`, extract module/sub-opcode and dispatch through `sim.packets` (`ModularPacketRouter`).

2. **World Object AoI Replication in TickSimulation::step**:
   - In `TickSimulation::step`, invoke `self.aoi.update_world_object_observers(&self.entity_store, &self.sectors)` and append resulting jobs to `self.pending_broadcast_jobs`.

3. **Evict Orphaned Observers in AoiManager::register_observer**:
   - In `AoiManager::register_observer`, if `self.guid_to_session.get(&player_guid)` returns an `old_session_id != session_id`, remove `old_session_id` from `self.observers`. Also guarantee bidirectional consistency if `session_id` was previously mapped to an obsolete GUID.

4. **Complete Entity Despawn and Migration Cleanup**:
   - Add `SectorManager::clear_rate_limit(&mut self, player_guid: Guid) -> bool`.
   - In `TickSimulation::despawn_entity` and in both single and batch outbound entity migration, cleanly purge `self.aoi.unregister_by_guid(guid)`, `self.last_migration_ticks.remove(&guid)`, `self.session_epochs.remove(&guid)`, and `self.sectors.clear_rate_limit(guid)`.

5. **Unified Damage & Effect Pipeline**:
   - Create a unified `apply_combat_effect` helper function in `crates/node/src/combat_pipeline.rs` applying damage mitigation (`StatsComponent::mitigate_damage`), health updates, death check & event (`EntityDiedEvent`), action interrupts (`ActionQueue::interrupt`), and attribute synchronization (`AttributeSet`).
   - Validate target is alive before deducting resources in single-target casts, skip dead targets in AoE batches, and use `apply_combat_effect` uniformly across all local and remote damage execution.

## Scope & Non-Goals
- **In Scope**:
  - `crates/schema`: `cluster.proto` update for `session_epoch` in `ForwardClientPacketRequest`.
  - `crates/gateway`: Update `main.rs` backhaul caller to pass `session_epoch`.
  - `crates/node`: `network.rs`, `sim.rs`, `aoi.rs`, `sector.rs`, `ability_dispatcher.rs`, `combat_pipeline.rs`.
  - Unit and integration tests covering all 5 areas.
- **Non-Goals**:
  - Rewriting the spatial index octree or sector storage format.
  - Modifying external gateway connection multiplexing.
