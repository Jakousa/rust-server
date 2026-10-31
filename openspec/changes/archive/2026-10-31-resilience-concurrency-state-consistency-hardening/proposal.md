# Proposal: Resilience, Error-Handling, and State-Consistency Hardening

## Motivation & Problem Statement
A comprehensive audit across `crates/storage`, `crates/gateway`, `crates/director`, `crates/node`, `crates/gameplay`, and `crates/plugins/voxel` uncovered five categories of resilience and state-consistency bugs:

1. **Non-Atomic 2PC Aborts, Lock Ordering Inversion, and Duplicate Items in Storage**:
   - `TransactionCoordinator::abort_transfer` deletes reservations and inserts dummy `(0, 0, '')` transaction logs outside of a database transaction, ignoring errors (`let _ = ...`).
   - `EscrowCoordinator::create_escrow` and `MarketplaceRepository::create_listing` acquire `SELECT ... FOR UPDATE` locks on containers without canonical ascending ordering (`min, max`), allowing deadlocks under concurrent execution.
   - `execute_trade` and `create_escrow` do not validate that `item_instance_ids` contains unique entries, risking duplicated slot assignments.

2. **Topic Subscription Race Conditions & Background Task Leaks in Gateway and Storage**:
   - `GatewayInterceptHandler::ensure_bus_listener` has a race between `contains_key` and `.insert` across an await point; duplicate tasks are spawned and replaced tasks are not aborted.
   - `GatewayInterceptHandler::remove_session` only cleans up topics starting with `"whisper:"`, leaking other bus listeners (`party`, `guild`, `custom`).
   - `SessionDropGuard::drop` never calls `intercept_handler.remove_session`, leaking topic tasks on connection disconnects.
   - `RedisSocialBus` lacks an `unsubscribe` mechanism to prune `subscribed_channels`, keeping background redis subscription tasks active indefinitely.

3. **Float Comparators and Equipment Boundary Violations in Gameplay**:
   - `ThreatTableComponent::add_threat` uses partial_cmp with unwrap and permits `f32::NAN` threat values, panicking the node tick.
   - `InventoryPacketHandler` defaults invalid equip slot indices (`>= 4`) to `Boots` instead of returning a validation error, and fails to restore unequipped items directly to the vacated inventory slot.
   - `PerceptionComponent::spotted_targets` accumulates targets endlessly without pruning dead, despawned, or out-of-range entities.

4. **Voxel Write-Behind Silent Data Loss & Entity Validation**:
   - `VoxelTickSystem::update` unconditionally marks chunks clean even if `write_behind.enqueue(...)` returns `false` due to full queue capacity.
   - `VoxelPacketHandler::handle` defaults player position to `(0, 0, 0)` when entity GUID is missing instead of rejecting the packet.

5. **Director State Inconsistency, Node Leases, and Action Interrupt Wiring**:
   - `GlobalOctreeCoordinator::handle_split` mutates `registry` and advances `topology_epoch` even if `self.root.split_at(...)` returns `false`.
   - `DirectorServiceImpl::deregister_node` passes an empty string as fallback gateway address when broadcasting route reassignments.
   - `TickSimulation::step` drops incoming `ContainerTransferNotification`s and never expires stale `migrating_leases`.
   - Migration gRPC calls lack bounded network timeouts.
   - `ActionTickSystem::update` does not evaluate `ActionInterruptFlags` (`cancel_on_movement`, `cancel_on_stun`, `cancel_on_target_lost`) and does not emit `ActionProgressEvent`.

## Proposed Changes & User Impact
- Guarantee ACID atomicity and accurate audit records during 2PC aborts in `mmo-storage`.
- Eliminate database deadlocks by enforcing canonical container lock ordering.
- Guarantee full RAII cleanup of pub/sub tasks upon disconnect in `mmo-gateway` and `mmo-storage`.
- Prevent node thread panics and state corruption by enforcing strict finite float validation, valid equip bounds, and active target pruning.
- Prevent terrain data loss during backpressure by retaining dirty chunks for retry.
- Guarantee octree and cluster topology integrity in `mmo-director` and `mmo-node`.

## Non-Goals
- Introducing game-genre specific rules into core crates.
- Changing wire packet formats or breaking client protocol compatibility.
