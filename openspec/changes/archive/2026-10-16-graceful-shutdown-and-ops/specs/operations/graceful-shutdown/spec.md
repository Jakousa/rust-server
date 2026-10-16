# Graceful Shutdown, Gateway Discovery & Operations Specification

## Purpose
Specifies requirements for structured daemon shutdown (SIGTERM/SIGINT) with session draining, persistent snapshot buffer flushing, heartbeat abort prior to deregistration, dynamic multi-gateway route propagation, and distributed tracing across cluster services.

---

## Requirements

### Requirement: Cross-Platform Shutdown Signal Handling
All MMO daemons (`mmo-director`, `mmo-node`, `mmo-gateway`) SHALL intercept termination signals (`SIGINT` / Ctrl+C and `SIGTERM`) using an asynchronous signal listener.

#### Scenario: Daemon receives SIGTERM in Kubernetes
- **GIVEN** a running server daemon with healthy readiness status
- **WHEN** the process receives a `SIGTERM` or `SIGINT` signal
- **THEN** the shutdown signal future SHALL resolve immediately
- **AND** the daemon SHALL transition its readiness state to unready (`telemetry.mark_unready()`)
- **AND** the daemon SHALL initiate its graceful draining sequence.

---

### Requirement: Gateway Session Draining & Graceful Teardown
The edge gateway SHALL cleanly terminate client connections, stop accepting new connections, abort its background heartbeat task, and deregister from Director upon receiving a shutdown signal.

#### Scenario: Gateway shutdown draining
- **GIVEN** an active gateway handling connected client sessions
- **WHEN** a shutdown signal is received
- **THEN** the TCP accept loop SHALL immediately stop accepting new client connections
- **AND** the gateway heartbeat loop SHALL be cancelled before deregistration
- **AND** the gateway SHALL attempt to deregister itself from the Director via `DeregisterGateway` with a bounded 2-second timeout
- **AND** connected client sessions SHALL be drained and closed within a 3-second grace period.

---

### Requirement: Node Simulation Tick & Snapshotter Draining
The simulation node SHALL abort its heartbeat task, flush all queued entity snapshots to persistent storage, and deregister from the Director before exiting.

#### Scenario: Node shutdown flushes write-behind snapshots
- **GIVEN** an active simulation node with uncommitted dirty entity snapshots queued in memory or channel
- **WHEN** a shutdown signal is received
- **THEN** the fixed 20 Hz simulation tick loop SHALL exit
- **AND** the background heartbeat task SHALL be aborted to avoid re-registration race conditions
- **AND** the node SHALL invoke `snapshotter.shutdown()` and await the worker task with a 5-second timeout
- **AND** the node SHALL execute `snapshotter.flush_batch().await` ensuring all dirty entities are written to the database
- **AND** the node SHALL call Director's `DeregisterNode` RPC with a bounded 2-second timeout
- **AND** the gRPC server SHALL shut down cleanly.

---

### Requirement: Director Deregistration & Orphaned Partition Recovery
When a node cleanly deregisters, Director SHALL immediately recover its spatial partitions and evict its dynamic instances.

#### Scenario: Clean node deregistration triggers partition recovery
- **GIVEN** a registered node with assigned bounding boxes and active instances
- **WHEN** Director receives a `DeregisterNode` RPC request
- **THEN** Director SHALL remove the node from `NodeRegistry`
- **AND** Director SHALL invoke partition recovery on `GlobalOctreeCoordinator` and instance eviction on `InstanceRegistry` under lock order `instance_registry` -> `registry` -> `coordinator`
- **AND** the response SHALL indicate successful deregistration.

---

### Requirement: Dynamic Gateway Route Broadcast in Director Recovery
The Director dead node recovery loop SHALL broadcast route reassignments concurrently to all active edge gateways tracked in `GatewayRegistry`.

#### Scenario: Multiple active gateways receive route updates
- **GIVEN** two or more active edge gateways registered with Director (`GatewayRegistry`)
- **WHEN** the Director detects a dead node and reassigns its partitioned bounds to a new node
- **THEN** the Director SHALL iterate over all active gateways in `GatewayRegistry`
- **AND** the Director SHALL send `ReassignNodeRoutesRequest` via gRPC concurrently to each active gateway with a 2-second timeout per gateway
- **AND** if no dynamic gateways are registered, the Director SHALL fall back to `GATEWAY_GRPC_ADDR`.

---

### Requirement: Distributed Tracing Context on Packet Pipeline
Cluster packet ingestion, forwarding, and handling SHALL emit structured tracing spans for latency and debugging observability, strictly omitting credentials or raw binary payloads.

#### Scenario: Client packet span propagation
- **GIVEN** an incoming client packet received at the edge gateway
- **WHEN** the packet is decoded and forwarded to the authoritative simulation node
- **THEN** a tracing span SHALL record packet ingress, routing target, opcode, and dispatch outcome
- **AND** authentication tokens and raw payloads SHALL NOT be recorded in span attributes
- **AND** failures SHALL be recorded as span error events.
