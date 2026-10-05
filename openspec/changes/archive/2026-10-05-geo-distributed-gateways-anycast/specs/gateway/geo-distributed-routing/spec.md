# Geo-Distributed Edge Gateways & Anycast Routing Specification

## Purpose
Specifies multi-region edge gateway registration, persistent backhaul connection multiplexing, Anycast health probing, and seamless cross-gateway session handovers across the distributed cluster.

---

## Requirements

### Requirement: Gateway Registration & Health Heartbeats
The Edge Gateway SHALL register its identity, region identifier, public ingress address, and cluster gRPC address with the Director upon initialization, and periodically transmit heartbeats containing active session load and latency telemetry.

#### Scenario: Edge gateway joins cluster
- **GIVEN** a running Director service
- **WHEN** an Edge Gateway starts in region `"eu-central"` with gateway ID `"gw-eu-1"`
- **THEN** it SHALL invoke `DirectorService.RegisterGateway` providing its region, listen address, gRPC address, and initial capacity
- **AND** the Director SHALL record the gateway in its active `GatewayRegistry`

#### Scenario: Edge gateway heartbeat and stale detection
- **GIVEN** a registered Edge Gateway in the Director's registry
- **WHEN** the gateway fails to emit a heartbeat within the 3.5-second timeout window
- **THEN** the Director SHALL mark the gateway as `Degraded` or `Unreachable`
- **AND** cluster routing SHALL exclude the gateway from new session assignments

---

### Requirement: Anycast Latency & Health Probing with Anti-Amplification Padding
The Edge Gateway SHALL expose an Anycast health and latency probe endpoint that responds to external UDP/HTTP probe queries with regional identity, capacity utilization, and monotonic timestamps for RTT estimation. To prevent Distributed Denial of Service (DDoS) reflection attacks, the gateway SHALL enforce request payload padding.

#### Scenario: Client or probe measures gateway latency with valid padding
- **GIVEN** an active Edge Gateway listening on its probe port
- **WHEN** an incoming UDP probe request arrives with at least 64 bytes of request payload
- **THEN** the gateway SHALL respond with a lightweight payload containing its `gateway_id`, `region`, current `load_factor`, and echoed timestamp
- **AND** the response SHALL be generated without acquiring write locks on the session routing tables

#### Scenario: Unpadded probe request rejected
- **GIVEN** an active Edge Gateway listening on its probe port
- **WHEN** an incoming UDP probe request contains fewer than 64 bytes
- **THEN** the gateway SHALL silently drop the packet without emitting a response, preventing bandwidth amplification

---

### Requirement: Multi-Gateway Route Propagation
Simulation nodes and Directors SHALL propagate entity migration and route update events across all relevant registered edge gateways, ensuring continuous packet forwarding regardless of client ingress location.

#### Scenario: Entity migrates across node boundaries with multiple gateways
- **GIVEN** a player entity connected via Gateway A migrating from Node 1 to Node 2
- **WHEN** Node 1 executes the migration commit phase
- **THEN** Node 1 SHALL dispatch `RouteUpdateRequest` to the player's originating gateway
- **AND** if the player's originating gateway is unknown or global broadcast is enabled, Node 1 SHALL fan out the route update to all active gateways registered in the Director's `GatewayRegistry`

---

### Requirement: Persistent Backhaul Multiplexing & Data-Plane Isolation
Edge Gateways SHALL maintain a connection-pooled, persistent backhaul transport to regional simulation clusters. Control-plane commands SHALL utilize gRPC, while high-frequency data-plane client frames (kinematics, broadcast deltas) SHALL be multiplexed over QUIC streams/datagrams to prevent Head-of-Line (HoL) blocking across sessions during WAN packet loss.

#### Scenario: Client packets forwarded over backhaul pool
- **GIVEN** an established persistent QUIC backhaul channel between Gateway A and Simulation Node 1
- **WHEN** multiple connected clients transmit movement and interaction packets destined for Node 1
- **THEN** the gateway SHALL multiplex the frames over independent QUIC data streams or datagram channels
- **AND** packet loss on one client's channel SHALL NOT induce Head-of-Line blocking or delays on other sessions

---

### Requirement: Seamless Cross-Gateway Session Handover
The cluster SHALL support seamless handover of a player's active session from a source Edge Gateway to a target Edge Gateway upon network route change (Anycast shift or client roaming) without despawning the entity on the simulation node or requiring full re-authentication.

#### Scenario: Client roams to a new edge gateway
- **GIVEN** a player entity active on Node 1 receiving downstream packets via Gateway A
- **WHEN** the client establishes a connection with Gateway B presenting a valid `HandoverTicket`
- **THEN** Gateway B SHALL verify the ticket, claim the session in `SessionCacheBackend`, and issue `TakeoverSessionRouteRequest` to Node 1
- **AND** Node 1 SHALL redirect the player's downstream egress sink to Gateway B and advance the authoritative session epoch
- **AND** Gateway A SHALL receive a revocation notice to close its local client socket without triggering entity despawn on Node 1

#### Scenario: Upstream frame epoch validation on simulation node
- **GIVEN** a completed handover where Node 1 advanced the entity session epoch to `E = 2`
- **WHEN** delayed or buffered upstream frames from Gateway A arrive with `epoch = 1`
- **THEN** Node 1 SHALL drop the stale frames without processing or mutating entity state

#### Scenario: Node timeout during handover with cache rollback
- **GIVEN** Gateway B claims the session epoch in `SessionCacheBackend`
- **WHEN** the `TakeoverSessionRouteRequest` to Node 1 times out or fails due to node network error
- **THEN** Gateway B SHALL roll back or release the epoch claim in the cache and return a retryable error to the client

---

### Requirement: Session Hijacking, Replay Defense & Thrashing Protection
The gateway and simulation node SHALL reject expired, forged, or replayed handover tickets, enforcing monotonic session epochs, constant-time cryptographic verification, and token-bucket rate limiting on handover frequency.

#### Scenario: Replayed or stale handover ticket presented
- **GIVEN** a handover ticket previously consumed or superseded by a newer session epoch
- **WHEN** an attacker attempts to use the ticket to claim session control at a gateway
- **THEN** the gateway SHALL reject the handover attempt with an authentication error
- **AND** the event SHALL increment the `mmo_gateway_handover_rejected_total` security metric

#### Scenario: BGP route flapping / rapid handover thrashing
- **GIVEN** an entity GUID that has already executed a handover within the last 5 seconds
- **WHEN** another handover request arrives exceeding the token-bucket limit
- **THEN** the gateway SHALL reject the handover attempt with a rate-limit error, preventing cache lock contention and cluster thrashing
