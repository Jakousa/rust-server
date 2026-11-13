# Resilience, Concurrency, and State-Consistency Specification

## Purpose
Defines authoritative guarantees across distributed 2PC transactions, container lock ordering, edge gateway session/topic cleanup, gameplay float/slot validation, voxel backpressure, director topology splitting, and node migration lease timeouts.

## Requirements

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
When an edge client session disconnects or is removed, all subscribed bus listeners MUST be unsubscribed when no active listeners remain, and `SessionDropGuard` MUST guarantee cleanup on drop.

#### Scenario: Session disconnect revokes topic subscriptions
- **GIVEN** an edge client session subscribed to whisper and broadcast topics
- **WHEN** the session disconnects or `SessionDropGuard` is dropped
- **THEN** all subscribed bus listeners with no remaining active sessions are unsubscribed via `SocialBus::unsubscribe` without leaking subscription tasks.

---

### Requirement: Gameplay Float and Boundary Safety
Gameplay components MUST reject non-finite/non-positive threat values, validate equipment slot boundaries (`<= 4`), and prune stale targets on perception scans.

#### Scenario: Threat table NaN rejection and equipment slot validation
- **GIVEN** `ThreatTableComponent`, `InventoryPacketHandler`, and `PerceptionScanSystem`
- **WHEN** `NaN` threat is added, an out-of-range equipment slot (`> 4`) is requested, or targets despawn/move out of range
- **THEN** `ThreatTableComponent` rejects `NaN` and sorts safely via `f32::total_cmp`, `InventoryPacketHandler` returns `MalformedPayload` (or swaps back into the vacated slot on valid replacement), and `PerceptionScanSystem` prunes stale targets.

---

### Requirement: Voxel Plugin Backpressure and Entity Validation
`VoxelTickSystem` MUST preserve dirty chunk state on write-behind queue backpressure, and `VoxelPacketHandler` MUST reject modifications from unknown entities.

#### Scenario: Write-behind queue full and unknown entity modification
- **GIVEN** a full `VoxelWriteBehindService` coalescing queue or a modification packet for a missing `entity_guid`
- **WHEN** `VoxelTickSystem` flushes dirty chunks or `VoxelPacketHandler` handles the packet
- **THEN** `VoxelTickSystem` re-marks the chunk dirty for retry on subsequent ticks, and `VoxelPacketHandler` returns status `1` (`OutOfReach`).

---

### Requirement: Director Topology Splitting and Fallback Gateway
`GlobalOctreeCoordinator::handle_split` MUST verify `split_at` succeeds before mutating `NodeRegistry` or advancing `topology_epoch`, and `DirectorServiceImpl` MUST provide a configurable fallback gateway address during node deregistration.

#### Scenario: Unsplittable octant and node deregistration fallback gateway
- **GIVEN** an octant that cannot be split or a node deregistering from `DirectorServiceImpl`
- **WHEN** `handle_split` or node deregistration executes
- **THEN** an unsplittable octant leaves `NodeRegistry` and `topology_epoch` unchanged, and deregistration broadcasts route reassignments using the configured fallback gateway address.

---

### Requirement: Node Migration Leases and Timeouts
`TickSimulation::step` MUST sweep expired migration leases (> 5s), wrap outbound migration RPCs in a 5-second timeout, and evaluate action cancellation flags on every tick.

#### Scenario: Migration RPC timeout and action interruption evaluation
- **GIVEN** an outbound migration RPC exceeding 5 seconds or an active `TimedAction` on a moving/stunned entity
- **WHEN** the timeout expires or `ActionTickSystem::update` runs
- **THEN** migration leases are released with rollback metrics and `MigrationError::RpcFailed`, and interrupted actions emit `ActionInterruptedEvent`.
