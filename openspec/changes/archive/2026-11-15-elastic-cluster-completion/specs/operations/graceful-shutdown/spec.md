## ADDED Requirements

### Requirement: Graceful Peer Entity Evacuation on Node Drain
When a simulation node shuts down cleanly (not due to fencing), it SHALL reject new inbound migrations (`is_draining = true`), deregister from Director so surviving peers claim its partitions, and migrate active movable entities and attached player sessions in batches to surviving peers before flushing snapshots and closing its gRPC server.

#### Scenario: Node shutdown evacuates active entities to peers before closing
- **GIVEN** an active simulation node with movable entities and active peer partitions in the cluster alongside dirty entity snapshots
- **WHEN** a shutdown signal is received
- **THEN** the fixed 20 Hz simulation tick loop SHALL exit and set `sim.is_draining = true` so inbound migrations from peers are rejected
- **AND** the background heartbeat and ghost replication tasks SHALL be aborted to avoid re-registration races
- **AND** the node SHALL call Director's `DeregisterNode` RPC with a bounded 2-second timeout so Director reassigns orphaned partitions and gateway routes
- **AND** the node SHALL extract all movable root entities (and hierarchies) via `collect_all_entities_for_drain` and migrate them in batches to surviving peers via `execute_batch_entity_migration_pooled`
- **AND** the node SHALL invoke `snapshotter.shutdown()`, await the worker task with a 5-second timeout, and execute `snapshotter.flush_batch().await` before closing the gRPC server.
