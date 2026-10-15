# simulation/aoi Specification Delta

## Requirements

### Requirement: Broadcast Job Draining in Authoritative Simulation Loop
The simulation node daemon SHALL drain all accumulated `pending_broadcast_jobs` on every simulation tick to prevent unbounded memory growth and dispatch the jobs downstream.

#### Scenario: Simulation tick drains pending broadcast jobs
- **WHEN** an authoritative simulation tick completes in the node daemon tick loop
- **THEN** the system SHALL invoke `sim.drain_broadcast_jobs()`
- **AND** `sim.pending_broadcast_jobs` SHALL be empty after the drain
- **AND** any drained jobs SHALL be handed off to the gateway broadcast delivery backhaul without blocking the tick loop

### Requirement: Gateway Broadcast Job Ingestion & Session Sink Delivery
The Edge Gateway SHALL expose a gRPC endpoint (`DeliverBroadcastJobs`) to receive batched AoI broadcast packets from simulation nodes, forward each packet to the target client's session sink, and report disconnected sessions back to the node.

#### Scenario: Gateway receives broadcast jobs for active session
- **WHEN** a simulation node delivers a batch of client packets for an active session ID
- **THEN** the gateway SHALL resolve the corresponding `SessionSink` from its session table
- **AND** ephemeral packets (such as movement and entity state deltas) SHALL be sent to `sink.datagram_tx`
- **AND** reliable packets (such as spawns, despawns, and interactions) SHALL be sent to `sink.stream_tx`

#### Scenario: Gateway detects disconnected session in broadcast batch
- **WHEN** a simulation node delivers a batch for a session ID that is no longer present in `session_sinks`
- **THEN** the gateway SHALL record the session ID in `disconnected_sessions` of `DeliverBroadcastJobsResponse`
- **AND** upon receiving this response, the simulation node SHALL unregister the orphaned AoI observer and despawn the entity

### Requirement: Gateway Ingress Forwarding via BackhaulPool
The Edge Gateway daemon SHALL forward incoming routed client packets (movement, ability, interaction, object placement, and ACK) to the designated simulation node using `BackhaulPool` with circuit breaker state tracking.

#### Scenario: Ingress movement packet routed to node
- **WHEN** a client transmits a movement or action packet to the gateway
- **THEN** the gateway background packet consumer SHALL resolve or initialize a backhaul channel for the destination node address in `BackhaulPool`
- **AND** dispatch the packet across the channel to the authoritative simulation node
- **AND** record success on response or failure on RPC error to drive the channel circuit breaker

### Requirement: Authoritative Movement Validation & Correction Snapback
When an authoritative simulation node rejects a client movement packet due to velocity, sequence, or collision bounds, it SHALL queue a `MovementCorrection` packet targeted at that player session.

#### Scenario: Client movement violates authoritative constraints
- **WHEN** `process_client_move_with_seq` returns an error position indicating an invalid movement
- **THEN** the node SHALL construct a `MovementCorrectionPacket` with the authoritative corrected coordinates and sequence
- **AND** enqueue it for immediate delivery in the next AoI broadcast batch
