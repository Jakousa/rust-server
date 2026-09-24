# Gateway Routing Specification

## Purpose

Manages client transport termination over QUIC and FlatBuffers, session tracking, and dynamic routing to authoritative simulation nodes.

## Requirements

### Requirement: Client Transport Termination
The Edge Gateway SHALL accept client connections over native QUIC UDP transport and deserialize binary game packets from multiplexed bidirectional or unidirectional streams.

#### Scenario: Client connects to gateway
- **WHEN** a client establishes a native QUIC connection over UDP with the Edge Gateway
- **THEN** the gateway SHALL negotiate the TLS 1.3 session and maintain the active client connection streams

### Requirement: Dynamic Packet Forwarding
The Edge Gateway SHALL route incoming movement and interaction packets to the specific simulation node owning the player's current octant.

#### Scenario: Seamless octant transition
- **WHEN** a player entity migrates from Node 1 to Node 2 across an octree boundary
- **THEN** the gateway SHALL update its internal routing table to forward subsequent packets to Node 2 without disconnecting the client

### Requirement: Downstream Server Packet Delivery
The Edge Gateway SHALL maintain downstream packet channels for active client connections and asynchronously write server broadcast packets (`EntityState`, `EntityDespawn`, `MovementCorrection`) to the client transport stream.

#### Scenario: Forwarding simulation broadcast to client
- **WHEN** the authoritative simulation node emits broadcast packets targeted at a client session
- **THEN** the gateway SHALL serialize and transmit the binary frames across the client socket without blocking the upstream packet reader loop
