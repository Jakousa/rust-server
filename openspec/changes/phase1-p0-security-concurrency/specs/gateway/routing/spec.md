## MODIFIED Requirements

### Requirement: Dynamic Packet Forwarding
The Edge Gateway SHALL route incoming movement and interaction packets to the specific simulation node owning the player's current octant using concurrent, lock-sharded session routing to eliminate atomic read lock contention on the packet dispatch path.

#### Scenario: Seamless octant transition
- **WHEN** a player entity migrates from Node 1 to Node 2 across an octree boundary
- **THEN** the gateway SHALL update its internal routing table to forward subsequent packets to Node 2 without disconnecting the client

#### Scenario: High-throughput concurrent packet dispatch
- **WHEN** multiple client connection reader tasks concurrently dispatch incoming frames
- **THEN** the gateway SHALL lookup target node channels via sharded concurrent lookups without acquiring global read-write locks across all sessions

### Requirement: Downstream Server Packet Delivery
The Edge Gateway SHALL maintain downstream packet channels for active client connections and asynchronously write server broadcast packets (`EntityState`, `EntityDespawn`, `MovementCorrection`) to the client transport stream, isolating individual client stalls so that a slow connection cannot block or delay delivery to other active sessions.

#### Scenario: Forwarding simulation broadcast to client
- **WHEN** the authoritative simulation node emits broadcast packets targeted at a client session
- **THEN** the gateway SHALL serialize and transmit the binary frames across the client socket without blocking the upstream packet reader loop

#### Scenario: Isolated client stream stall
- **WHEN** a single client link stalls or experiences TCP/QUIC stream backpressure
- **THEN** other connected sessions SHALL continue receiving downstream broadcast frames without blocking on the stalled stream

## ADDED Requirements

### Requirement: Session RAII Lifecycle Cleanup
The Edge Gateway SHALL utilize RAII cleanup guards for client connection tasks to guarantee that session routes and active downstream channels are reliably removed from the router upon connection termination or external task abort.

#### Scenario: Client task cancellation cleanup
- **WHEN** a client transport task is cancelled or terminates due to error
- **THEN** the gateway's session drop guard SHALL automatically unregister the session from the routing table and downstream channel registry
