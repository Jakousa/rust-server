# simulation/aoi Specification

## Purpose
Tracks per-player spatial visibility in the Sparse Octree, computes dynamic entity visibility deltas, and generates authoritative Area-of-Interest (AoI) broadcast packets.

## Requirements

### Requirement: Player Area-of-Interest Tracking
The simulation node SHALL track active player sessions as AoI observers and query the spatial Octree for all entities within each player's visibility radius.

#### Scenario: Observer queries nearby entities
- **WHEN** an authoritative simulation tick executes for an active player session
- **THEN** the system SHALL perform a 3D spherical or bounding box range query centered on the player position using their effective visibility radius

### Requirement: Visibility Delta State Generation
The simulation node SHALL compare each player's current visible entity set against their previously observed entity set to compute enters, updates, and exits.

#### Scenario: Entity enters visibility radius
- **WHEN** an entity appears within a player's visibility radius that was not present in the previous tick
- **THEN** the system SHALL generate an `EntityState` packet representing the entity's current state and record the entity in the player's observed set

#### Scenario: Entity exits visibility radius or despawns
- **WHEN** an entity that was present in the player's previous observed set is no longer found in the current visibility query
- **THEN** the system SHALL generate an `EntityDespawn` packet targeted at the observing player session and remove the entity from the player's observed set

#### Scenario: Entity moves or updates within visibility radius
- **WHEN** an observed entity's position, velocity, or health changes while remaining within the player's visibility radius
- **THEN** the system SHALL generate an updated `EntityState` broadcast packet for the observing player session

### Requirement: Dynamic Visibility Throttling
The system SHALL dynamically clamp the observer's visibility radius when the number of visible entities exceeds the configured per-observer density budget, using an $O(N)$ partial selection algorithm.

#### Scenario: High entity density throttling
- **WHEN** the count of entities within the player's default visibility radius exceeds the configured maximum entity threshold
- **THEN** the system SHALL select the nearest entities up to the density budget using $O(N)$ partitioning without performing a full $O(N \log N)$ sort across the entire candidate set, and reduce the effective visibility radius accordingly

### Requirement: Zero-Allocation Scratch Buffering
The simulation AoI manager SHALL maintain reusable internal scratch buffers for entity candidate collection and visibility delta computations to avoid allocating heap vectors per observer on every tick.

#### Scenario: Reusing scratch buffers across observers
- **WHEN** the AoI manager evaluates visibility for multiple active observers during an authoritative simulation tick
- **THEN** it SHALL clear and reuse pre-allocated scratch buffers rather than allocating new vectors per observer query

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
