# Gateway Security Hardening Specification

## Purpose
Specifies strict maximum payload framing limits, bounded outbound streaming queues with backpressure and saturation policies, and Slowloris / connection concurrency mitigations for the Edge Gateway.

---

## Requirements

### Requirement: Maximum Packet Frame Payload Size Enforcement
The schema decoder SHALL enforce an upper limit of `MAX_PACKET_PAYLOAD_SIZE` (65,536 bytes / 64 KiB) on incoming wire frames in both `ClientPacket::decode_frame` and `ClientPacket::decode_bytes`.

#### Scenario: Packet exceeding payload limit is rejected immediately
- **GIVEN** an incoming packet frame with valid magic header bytes `0x4D4D` (`[0x4D, 0x4D]`)
- **WHEN** the declared `payload_len` in the frame header exceeds `MAX_PACKET_PAYLOAD_SIZE` (e.g. 65,537 bytes or 4 GiB)
- **THEN** `ClientPacket::decode_frame` and `ClientPacket::decode_bytes` SHALL return `Err(ClientCodecError::PacketTooLarge { size, max })`
- **AND** the buffer SHALL NOT wait for the remaining bytes to arrive
- **AND** the transport connection SHALL be terminated by the caller

#### Scenario: Valid packet within payload limit is accepted
- **GIVEN** an incoming packet frame with valid magic header bytes and `payload_len <= MAX_PACKET_PAYLOAD_SIZE`
- **WHEN** full payload bytes are present in the buffer
- **THEN** `ClientPacket::decode_frame` and `ClientPacket::decode_bytes` SHALL parse the packet successfully

---

### Requirement: Bounded Outbound Session Streams
The gateway session sink SHALL use bounded channels for reliable stream transmission across TCP, QUIC, and WebTransport to prevent unbounded memory growth from slow or paused clients.

#### Scenario: Downstream stream channel backpressure
- **GIVEN** a client connection registered with a bounded stream sink of capacity 1024
- **WHEN** reliable messages (social broadcast, trade, interaction response) are pushed to the session sink faster than the network socket flushes
- **THEN** after the 1024-message buffer is filled, subsequent `try_send` attempts SHALL fail with channel saturation
- **AND** metric `mmo_gateway_session_stream_full_dropped` SHALL be incremented
- **AND** memory usage SHALL remain bounded by channel capacity

#### Scenario: Active disconnect on sustained saturation
- **GIVEN** a client session whose reliable outbound channel is persistently saturated
- **WHEN** reliable packets cannot be enqueued
- **THEN** the gateway SHALL tear down the session and release all associated socket resources

---

### Requirement: Slowloris & Connection Concurrency Mitigation
The edge gateway TCP listener SHALL limit maximum concurrent active sessions and enforce an initial handshake timeout on all new connections.

#### Scenario: Max concurrent connection ceiling reached
- **GIVEN** an edge gateway configured with a maximum concurrent connection ceiling `max_connections`
- **WHEN** incoming connections reach `max_connections`
- **THEN** subsequent connection attempts SHALL either be dropped, rejected with an immediate close, or wait until a connection slot becomes available via concurrency semaphore
- **AND** metric `mmo_gateway_connections_rejected_max_capacity` SHALL be incremented

#### Scenario: Idle unauthenticated connection timeout (Slowloris)
- **GIVEN** a newly connected TCP socket that opens a connection
- **WHEN** the socket sends no valid packet within the handshake timeout duration (e.g. 10 seconds)
- **THEN** the gateway SHALL abort the connection
- **AND** metric `mmo_gateway_handshake_timeout` SHALL be incremented
