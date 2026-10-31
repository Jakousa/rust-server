# System Specification: Resilience, Concurrency, and State-Consistency

## Overview
Authoritative guarantees across distributed transactions, edge concurrency, spatial topology updates, gameplay validation, and plugin write-behind backpressure.

---

### Requirement: Atomic 2PC Abort and Audit
The storage 2PC transfer coordinator MUST execute `abort_transfer` within an atomic transaction. If the transfer reservation exists, it must query the reservation's source container, target container, and item instance ID, delete the reservation row, and insert an audit record preserving the original container and item identifiers with `'aborted'` status.

#### Scenario: Abort transfer with existing reservation
- **GIVEN** a pending reservation with ID `tx-1` reserving item `item-1` between container `c1` and container `c2`
- **WHEN** `abort_transfer("tx-1", dialect)` is invoked
- **THEN** the reservation row is removed and an audit row with `tx_id = 'tx-1'`, `src_container_id = c1`, `dst_container_id = c2`, `item_instance_id = 'item-1'`, and `status = 'aborted'` is created atomically.

#### Scenario: Abort transfer when reservation does not exist
- **GIVEN** no pending reservation with ID `tx-nonexistent`
- **WHEN** `abort_transfer("tx-nonexistent", dialect)` is invoked
- **THEN** it returns `Ok(false)` without inserting an audit record.

---

### Requirement: Container Lock Ordering and Distinctness
Distributed container operations (`create_escrow`, `create_listing`, `execute_trade`) MUST acquire container row locks in strictly sorted container ID order `(min(c1, c2), max(c1, c2))` and MUST reject operations where the source and destination or escrow container IDs are identical.

#### Scenario: Operations between identical containers are rejected
- **GIVEN** a request to create escrow or marketplace listing where source container == destination container
- **WHEN** the method is invoked
- **THEN** it immediately returns `Err(StorageError::InvalidParameter)` without acquiring locks or executing queries.

#### Scenario: Concurrent opposing transfers avoid deadlock
- **GIVEN** two concurrent transactions transferring items between containers `A` and `B` in opposite directions (`A -> B` and `B -> A`)
- **WHEN** both transactions execute concurrently
- **THEN** both lock `min(A, B)` before `max(A, B)`, guaranteeing deadlock-free execution.

---

### Requirement: Duplicate Item Instance Validation
Multi-item container operations (`create_escrow`, `execute_trade`) MUST reject collections containing duplicate item instance IDs before acquiring database locks.

#### Scenario: Duplicate item IDs in trade
- **GIVEN** a trade offering items `["gem-1", "gem-1"]`
- **WHEN** `execute_trade` is called
- **THEN** it immediately returns `Err(StorageError::DuplicateItem("gem-1"))`.

---

### Requirement: Edge Gateway Session and Topic Cleanup
When an edge client session disconnects or is removed:
1. All subscribed bus listeners (including whisper and broadcast topics) MUST be unsubscribed if no remaining active sessions are listening.
2. The `SocialBus` MUST provide an `unsubscribe` method which cleans up Redis subscription tasks or in-memory channel mappings.
3. `SessionDropGuard` MUST trigger session removal upon drop, preventing listener leaks on abrupt disconnects.

---

### Requirement: Gameplay Float and Boundary Safety
1. `ThreatTableComponent` MUST reject `NaN`, infinite, or negative/zero threat additions and use `f32::total_cmp` when evaluating target rankings to prevent comparison panics.
2. `InventoryPacketHandler` MUST validate equipment slot index `<= 4` and return `MalformedPayload` for out-of-range slots. When replacing equipped items, the unequipped item MUST be returned directly to the vacated inventory slot rather than scanning from slot 0.
3. `PerceptionScanSystem` MUST prune dead, despawned, or out-of-range targets from `PerceptionComponent::spotted_targets` on each scan.

---

### Requirement: Voxel Plugin Backpressure and Entity Validation
1. If `VoxelWriteBehindService::enqueue` fails due to full coalescing queue capacity, `VoxelTickSystem` MUST NOT mark the chunk clean and MUST re-mark the chunk dirty so it retries on subsequent ticks.
2. `VoxelPacketHandler` MUST reject modification packets if `entity_guid` is not found in the entity store, returning `VoxelModifyResponsePacket` with status `1` (OutOfReach).

---

### Requirement: Director Topology Splitting and Fallback Gateway
1. `GlobalOctreeCoordinator::handle_split` MUST verify `self.root.split_at` succeeds before mutating `NodeRegistry` or advancing `topology_epoch`. If the target octant cannot be split, it returns empty assignments without modifying state.
2. `DirectorServiceImpl` MUST store a configurable fallback gateway address (from `GATEWAY_GRPC_ADDR` or default) and pass it when broadcasting route reassignments during node deregistration.

---

### Requirement: Node Migration Leases and Timeouts
1. `TickSimulation::step` MUST sweep migrating leases older than 5 seconds and forward `ContainerTransferNotification` items directly into `event_queue`.
2. Outbound migration RPCs in `mmo_node::network::migration` MUST be wrapped in a 5-second `tokio::time::timeout`. On timeout, leases must be released, rollback metrics recorded, and `MigrationError::RpcFailed` returned.
3. `ActionTickSystem::update` MUST evaluate `cancel_on_movement`, `cancel_on_stun`, and `cancel_on_target_lost` flags, dispatching `ActionInterruptedEvent` on interruption, `ActionProgressEvent` while advancing, and `ActionCompletedEvent` on completion.
