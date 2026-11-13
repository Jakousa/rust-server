# Node Simulation Runtime Hardening Specification

## Purpose
Defines authoritative session disconnect persistence, batch migration tombstone consistency, multi-node AoE compensation aggregation, RFC 1982 sequence wraparound arithmetic, and concurrent ghost store TTL pruning on `mmo-node`.

## Requirements

### Requirement: Complete Session Disconnect Lifecycle & State Persistence
The simulation node MUST persist an authoritative snapshot, clean up session and kinematic state, tombstone the entity, and emit despawn events whenever a gateway reports disconnected sessions.

#### Scenario: Gateway reports disconnected session IDs
- **GIVEN** a running simulation node processing gateway broadcast delivery
- **WHEN** the gateway returns one or more disconnected session IDs
- **THEN** the simulation node MUST resolve the corresponding player GUID, cleanly invoke `disconnect_player_session(...)` capturing an authoritative snapshot, enqueueing it for persistence, removing kinematic and session epoch state, tombstoning the entity, and dispatching lifecycle despawn events (`OnDespawn`).

### Requirement: Batch Migration Tombstone Consistency
The donor simulation node MUST tombstone every migrated entity GUID upon confirmed batch ingestion by the recipient node.

#### Scenario: Confirmed batch migration tombstones donor entities
- **GIVEN** an active entity batch being migrated to a recipient node
- **WHEN** the recipient successfully confirms ingestion of the batch
- **THEN** the donor node MUST insert every migrated entity GUID into `tombstoned_entities` to prevent late client packets from rehydrating zombie entities on the donor partition.

### Requirement: Atomic Multi-Node Boundary AoE Compensation Aggregator
Boundary AoE abilities spanning multiple remote nodes MUST aggregate hit outcomes across all target partitions before enqueuing resource compensation.

#### Scenario: Multi-partition AoE compensation evaluation
- **GIVEN** a boundary AoE ability cast whose targets span multiple remote peer nodes with `local_hit_count == 0`
- **WHEN** all dispatched remote batch requests complete
- **THEN** resource compensation MUST be enqueued at most once, and ONLY IF zero targets were struck across all remote partitions
- **AND** if any remote partition reports at least one successful hit, no compensation or refund MUST be enqueued.

### Requirement: RFC 1982 Movement Sequence Arithmetic Across Wraparound
Client movement sequence validation MUST use RFC 1982 serial number arithmetic (`wrapping_sub` / `wrapping_add`) to remain continuous across `u32::MAX` wraparound.

#### Scenario: Sequence wraparound past u32::MAX
- **GIVEN** an active entity movement stream approaching or exceeding `u32::MAX`
- **WHEN** a client movement packet with a wrapped sequence number (e.g. sequence 0 or 1 following `u32::MAX`) is received
- **THEN** the simulation MUST accept the packet if the modular difference `sequence.wrapping_sub(last_sequence) as i32 > 0` and reject stale or replay packets where `diff <= 0`
- **AND** `process_client_move` sequence generation MUST use `wrapping_add(1)` instead of `saturating_add(1)` to avoid permanent stalls.

### Requirement: Tick-Based TTL Pruning for Replicated Ghost Records
Stale boundary ghost entities in `GhostStore` MUST be pruned automatically once their tick TTL expires.

#### Scenario: Stale ghost record TTL expiration
- **GIVEN** replicated boundary ghost entities stored in `GhostStore`
- **WHEN** a simulation tick progresses and a ghost record has not been refreshed within a configurable TTL threshold (e.g. 20 ticks / 1 second)
- **THEN** the stale ghost record MUST be pruned from `GhostStore` to prevent memory leaks and invalid ability targeting.

### Requirement: Decoupled Concurrent Ghost Store Synchronization
Incoming `GhostSyncRequest` RPCs MUST update `GhostStore` without acquiring an exclusive write lock on `TickSimulation`.

#### Scenario: Concurrent GhostSyncRequest ingestion
- **GIVEN** incoming `GhostSyncRequest` RPCs received from neighboring cluster nodes
- **WHEN** `NodeService::sync_ghosts` executes
- **THEN** it MUST synchronize ghost records directly into an independently synchronized `GhostStore` without acquiring an exclusive write lock on `TickSimulation`, eliminating lock contention against the main 20 Hz simulation loop.
