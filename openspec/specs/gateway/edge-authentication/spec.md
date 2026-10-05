# Edge Authentication, JWT/OIDC, & Session Handoff Specification

## Purpose
Specifies cryptographic admission control, JWT token validation, zero-cost packet dropping for unauthenticated sessions, strict identity-to-session binding (anti-spoofing), and secure session provisioning at the edge gateway.

---

## Requirements

### Requirement: Edge Authentication Handshake Protocol
The schema protocol SHALL define binary packets `AuthRequest` and `AuthResponse` enabling clients to submit cryptographic identity tokens and receive assigned session credentials.

#### Scenario: Client initiates authentication handshake
- **GIVEN** a newly connected client over TCP, QUIC, or WebTransport
- **WHEN** the client sends an `AuthRequest` containing a valid JWT token string and client protocol version
- **THEN** the gateway SHALL parse and validate the token
- **AND** if valid, the gateway SHALL assign/resolve an initial simulation node (via Director or configured fallback node)
- **AND** register the session in `SessionRouter`
- **AND** respond with `AuthResponse` containing `success = true`, assigned `session_id`, resolved `player_guid`, and destination `assigned_node`

#### Scenario: Malformed authentication frame
- **GIVEN** an unauthenticated connection
- **WHEN** a malformed frame or unparseable `AuthRequest` is received
- **THEN** the gateway SHALL reject the request, emit an `AuthResponse` with `success = false` and error code `INVALID_TOKEN`, and terminate the connection

---

### Requirement: Cryptographic JWT & Handover Token Verification
The gateway SHALL verify standard JWT claims (`exp`, `iss`, `aud`, `sub`) and cryptographic signatures (HMAC-SHA256, EdDSA, or RS256) or valid `HandoverTicket` HMACs. The validator SHALL explicitly enforce allowed signature algorithms and strictly forbid the `none` algorithm.

#### Scenario: Token signature verification succeeds
- **GIVEN** an `AuthRequest` with a valid JWT signed by the authorized identity provider
- **WHEN** the gateway validates the signature and checks that `exp` is in the future, `iss`/`aud` match gateway configuration, and algorithm is approved
- **THEN** authentication SHALL succeed
- **AND** the subject claim SHALL be extracted and mapped to the player's authoritative `Guid`

#### Scenario: Expired or tampered token rejected
- **GIVEN** an `AuthRequest` with an expired token, tampered signature, or unapproved algorithm (`none`)
- **WHEN** the gateway validates the token
- **THEN** authentication SHALL fail with `EXPIRED_TOKEN`, `INVALID_SIGNATURE`, or `INVALID_TOKEN`
- **AND** the connection SHALL NOT be admitted to the routing table
- **AND** metric `mmo_gateway_auth_failures_total` SHALL be incremented with the corresponding failure reason

---

### Requirement: Zero-Cost Unauthenticated Datagram & Stream Dropping
The gateway SHALL enforce zero-trust packet filtering at the transport boundary before packets reach routing or simulation logic.

#### Scenario: Unauthenticated datagrams dropped
- **GIVEN** a transport session in `Unauthenticated` state
- **WHEN** an unreliable UDP datagram (e.g. movement packet) arrives from the client
- **THEN** the gateway SHALL drop the datagram immediately via an atomic state check (`Ordering::Acquire`) before decoding packet headers or executing routing table lookups
- **AND** metric `mmo_gateway_unauthenticated_packets_dropped_total` SHALL be incremented

#### Scenario: Unauthenticated stream attempts non-auth actions
- **GIVEN** a transport session in `Unauthenticated` state
- **WHEN** the client transmits a reliable stream frame other than `AuthRequest` (e.g. Interaction, Trade, Chat)
- **THEN** the gateway SHALL reject the frame and abort the connection

---

### Requirement: Strict Identity-to-Session Binding (Anti-Spoofing)
The gateway packet dispatcher SHALL verify that any packet containing an entity identifier (`acting_guid`) originates from the session authenticated for that GUID.

#### Scenario: Packet with matching GUID is dispatched
- **GIVEN** an authenticated session for GUID `player_100`
- **WHEN** a packet arrives on that session with `acting_guid == player_100`
- **THEN** the gateway SHALL route the packet to the authoritative simulation node

#### Scenario: Spoofed packet rejected
- **GIVEN** an authenticated session for GUID `player_100`
- **WHEN** a packet arrives on that session with `acting_guid == player_200`
- **THEN** the gateway SHALL reject and drop the packet
- **AND** metric `mmo_gateway_spoofed_packets_dropped_total` SHALL be incremented

---

### Requirement: Secure Session Provisioning & Initial Node Resolution
Upon successful authentication, the gateway SHALL provision a secure session, resolve the simulation node, and map it in the routing table before spatial forwarding begins.

#### Scenario: Session registered upon authentication
- **GIVEN** successful validation of an `AuthRequest`
- **WHEN** the session is provisioned
- **THEN** the gateway SHALL resolve the destination simulation node
- **AND** register the `session_id`, `guid`, and assigned node in `SessionRouter`
- **AND** subsequently transition the atomic session state to `Authenticated` with `Ordering::Release`
- **AND** start admitting incoming datagrams and reliable stream packets for routing
