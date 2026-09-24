## ADDED Requirements

### Requirement: Downstream Server Packet Delivery
The Edge Gateway SHALL maintain downstream packet channels for active client connections and asynchronously write server broadcast packets (`EntityState`, `EntityDespawn`, `MovementCorrection`) to the client transport stream.

#### Scenario: Forwarding simulation broadcast to client
- **WHEN** the authoritative simulation node emits broadcast packets targeted at a client session
- **THEN** the gateway SHALL serialize and transmit the binary frames across the client socket without blocking the upstream packet reader loop
