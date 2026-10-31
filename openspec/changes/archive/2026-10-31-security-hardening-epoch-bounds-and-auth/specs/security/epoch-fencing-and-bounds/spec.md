# Security Hardening: Movement Validation, Social Bus Auth, Epoch Fencing, and Bounds Specification

## Purpose
Specifies authoritative movement delta time bounding, strict social bus channel access control, post-migration epoch fencing, IEEE-754 floating point sanitization, and bounded TTL arithmetic across all cluster tiers.

---

## Requirements

### Requirement: Bounded Authoritative Movement Time-Credit
The server simulation SHALL enforce that movement delta time is bounded by actual elapsed monotonic time using a replenishing time-credit bucket, and SHALL reject out-of-order and non-monotonic sequence numbers for all movement packets.

#### Scenario: High-frequency packet burst is rate-limited
- **GIVEN** an active player entity registered in `TickSimulation`
- **WHEN** client sends a burst of movement packets with minimal elapsed wall-clock time (< 50ms per packet)
- **THEN** movement distance allowed across the burst SHALL NOT exceed `max_speed * (real_elapsed_time + max_burst_allowance) * tolerance_multiplier`
- **AND** excess movement attempts exceeding accumulated time credit SHALL be rejected with snap-back without draining remaining credit.

#### Scenario: Sequence monotonicity enforcement
- **GIVEN** an initialized kinematic state with `last_sequence = S`
- **WHEN** a client submits a movement packet with sequence number `<= S` (including `sequence = 0`)
- **THEN** the movement packet SHALL be rejected with snap-back
- **AND** `last_sequence` SHALL NOT be updated.

---

### Requirement: Social Bus Authorization and Anti-Spoofing
The edge gateway SHALL authenticate and enforce session ownership on all social message publishing and subscription requests.

#### Scenario: Unbound session attempting to send social message
- **GIVEN** an unauthenticated client session with no bound GUID in `SessionRouter`
- **WHEN** the client submits a `SocialMessage` packet
- **THEN** the packet SHALL be dropped
- **AND** metric `mmo_gateway_spoofed_social_packets_dropped_total` SHALL be incremented.

#### Scenario: Unauthorized whisper eavesdropping prevented
- **GIVEN** an authenticated client session bound to `player_guid_A`
- **WHEN** the client attempts to subscribe to `whisper:{player_guid_B}` where `player_guid_B != player_guid_A` via channel type or channel name
- **THEN** the subscription attempt SHALL be dropped or rejected
- **AND** the session SHALL NOT receive private messages addressed to `player_guid_B`.

---

### Requirement: Epoch Fencing and Migration Tombstoning
The simulation node and gateway SHALL prevent duplicate entity resurrection from late in-flight packets following migration or despawn.

#### Scenario: Late in-flight movement packet after despawn or migration
- **GIVEN** a player entity that has been despawned or migrated away from Node A
- **WHEN** a late in-flight movement packet arrives at Node A
- **THEN** Node A SHALL identify the entity as tombstoned
- **AND** the packet SHALL NOT auto-hydrate a duplicate player entity on Node A
- **AND** metric `mmo_node_stale_epoch_packets_dropped_total` SHALL be incremented.

#### Scenario: Gateway propagates authoritative session epoch
- **GIVEN** an authenticated session with assigned `session_epoch = E`
- **WHEN** the gateway forwards a client packet to an authoritative simulation node
- **THEN** `ForwardClientPacketRequest.session_epoch` SHALL equal `E`.

---

### Requirement: Floating-Point Sanitization at Boundaries
All external and network boundaries SHALL reject non-finite (`NaN`, `+Infinity`, `-Infinity`) floating point numbers before processing.

#### Scenario: Malformed float in client movement packet
- **GIVEN** an incoming wire frame containing a `MovementPacket` with `NaN` or `Infinity` in position, velocity, yaw, or pitch
- **WHEN** `MovementPacket::decode` or `ClientPacket::decode_frame` processes the frame
- **THEN** the decoder SHALL return `Err(ClientCodecError::InvalidData(...))`
- **AND** the packet SHALL NOT be accepted.

#### Scenario: Non-finite floats in Director heartbeats and splits
- **GIVEN** a node heartbeat containing `NaN` or `Infinity` in `tick_duration_ms`, `cpu_percent`, or centroid coordinates
- **WHEN** `NodeRegistry::record_heartbeat` processes the heartbeat
- **THEN** non-finite values SHALL NOT update or corrupt internal EWMA metrics or centroid state.

#### Scenario: Remote ability with non-finite position coordinates
- **GIVEN** an `execute_remote_ability` invocation with `NaN` in `caster_position`
- **WHEN** range validation executes
- **THEN** the RPC SHALL reject the request with out-of-range error rather than allowing the check to pass.

---

### Requirement: Bounded TTL and Checked Arithmetic
All TTL calculations and timestamp arithmetic SHALL use checked operations and enforced upper bounds.

#### Scenario: Oversized TTL does not cause integer overflow
- **GIVEN** a marketplace listing or escrow creation request with an oversized duration (e.g. `u64::MAX`)
- **WHEN** expiry timestamps are calculated
- **THEN** the duration SHALL be capped at a safe maximum (e.g. 30 days) and computed using checked arithmetic without panics or negative timestamps.

#### Scenario: Pre-auth buffer limit and codec error enforcement
- **GIVEN** a new incoming connection prior to authentication
- **WHEN** an unauthenticated client streams bytes with invalid magic, corrupted frame data, or exceeds `MAX_PACKET_PAYLOAD_SIZE`
- **THEN** the gateway SHALL terminate the connection immediately and increment `mmo_gateway_auth_failures_total`.
