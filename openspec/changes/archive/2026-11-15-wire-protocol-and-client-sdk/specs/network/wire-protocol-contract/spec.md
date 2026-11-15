# Wire Protocol & External Client SDK Contract Specification

## Purpose
Defines the stabilized little-endian `0x4D4D` binary wire envelope, protocol version constant, top-level schema exports, byte-exact packet layouts, delta-compression and baseline ACK semantics, client-side prediction/replica helper primitives, and external engine (Godot 4) integration documentation contract.

## ADDED Requirements

### Requirement: Wire Frame Envelope and Version Constant
The schema crate (`mmo-schema`) SHALL stabilize the client wire envelope with an 8-byte little-endian header, a `PROTOCOL_VERSION` constant, and top-level crate re-exports for external and internal SDK consumers.

#### Scenario: 8-Byte `0x4D4D` Frame Encoding and Bound Enforcement
- **GIVEN** a `ClientPacket` encoded via `ClientPacket::encode` or `ClientPacket::encode_into`
- **WHEN** inspected on the wire
- **THEN** bytes `0..2` SHALL equal `PACKET_MAGIC` (`0x4D4D` little-endian `[0x4D, 0x4D]`), bytes `2..4` SHALL equal the `Opcode` discriminant (`u16` LE), bytes `4..8` SHALL equal the exact `payload_len` (`u32` LE), and `ClientPacket::decode_bytes`/`decode_frame` MUST reject any payload exceeding `MAX_PACKET_PAYLOAD_SIZE` (`65536` bytes) with `ClientCodecError::PacketTooLarge`.

#### Scenario: Top-Level Schema Re-Exports and Protocol Version
- **GIVEN** a consumer importing `mmo_schema`
- **WHEN** referencing protocol framing primitives
- **THEN** `PACKET_MAGIC`, `HEADER_SIZE`, `MAX_PACKET_PAYLOAD_SIZE`, `PROTOCOL_VERSION` (`1`), `Opcode`, `ClientPacket`, and `ClientCodecError` SHALL be exported directly at the `mmo_schema` crate root.

### Requirement: Opcode and Binary Wire Stability
All existing client protocol opcodes (`0x0001`..`0x001A`), LEB128 varint codecs, and fixed-point quantization formulas SHALL preserve deterministic byte-for-byte wire representations verified by golden byte-vector tests.

#### Scenario: Golden Byte-Vector Regression Verification
- **GIVEN** canonical instances of `AuthRequest`, `AuthResponse`, `Movement`, `Interaction`, `EntityDeltaBatch`, and `ClientAck` packets
- **WHEN** serialized via `ClientPacket::encode` and deserialized via `ClientPacket::decode`
- **THEN** the emitted byte slices SHALL match the pinned golden little-endian wire vectors byte-for-byte (including `0x4D4D` header, LEB128 varints, `u16` length-prefixed strings, quantized transforms, and IEEE-754 little-endian floats) and round-trip without data loss beyond documented fixed-point quantization precision.

### Requirement: Delta Compression and Baseline ACK Contract
Server-to-client `EntityDeltaBatch` (`0x0009`) replication and client-to-server `ClientAck` (`0x000A`) acknowledgements SHALL obey deterministic `field_mask` bit ordering and server-authoritative baseline tracking rules.

#### Scenario: Bitmask Field Ordering and Teleport Keyframe Fallback
- **GIVEN** an `EntityDeltaState` encoded inside `EntityDeltaBatch`
- **WHEN** `field_mask` bits in `delta_flags` are set
- **THEN** fields SHALL appear in ascending bit order (`MASK_KEYFRAME` `1<<0` exclusive with `MASK_DELTA_POS_{X,Y,Z}` `1<<1..3`, followed by `MASK_VELOCITY` `1<<4`, `MASK_YAW` `1<<5`, `MASK_PITCH` `1<<6`, `MASK_HEALTH` `1<<7`, `MASK_MAX_HEALTH` `1<<8`, `MASK_STATE_FLAGS` `1<<9`, `MASK_ACTION_ID` `1<<10`, `MASK_ATTRIBUTES` `1<<11`), and any displacement exceeding `MAX_DELTA_POS` (`255.0` meters) MUST emit a full `3x f32` `MASK_KEYFRAME` update.

#### Scenario: Baseline Acknowledgement Reporting
- **GIVEN** a client applying an `EntityDeltaBatch` for `server_tick`
- **WHEN** the client emits `ClientAck` (`session_id`, `observed_tick`, 32-tick `ack_mask`) over a reliable stream
- **THEN** the simulation node advances the session's compression baseline while rejecting any untrusted future `observed_tick > current_tick`.

### Requirement: Client SDK Prediction and Replica Helpers
The client SDK (`mmo-client-wasm`) SHALL expose unopinionated helper methods on `PredictionBuffer` and `ClientEntityReplica` for reconciliation diagnostics, normalized health inspection, and state-flag bitmask queries.

#### Scenario: Prediction Error Distance and Sequence Lookup
- **GIVEN** a `PredictionBuffer` with recorded `ClientInput` frames
- **WHEN** `latest_sequence()` and `error_distance(server_pos)` are queried
- **THEN** `latest_sequence()` SHALL return `Some(seq)` for the most recently pushed unacknowledged input or `None` when empty, and `error_distance(server_pos)` SHALL return the Euclidean distance `||current_sim_pos - server_pos||_2`.

#### Scenario: Zero-Safe Health Fraction and Exact Bitmask Flag Queries
- **GIVEN** a `ClientEntityReplica`
- **WHEN** `health_fraction()` and `has_state_flag(flag)` are called
- **THEN** `health_fraction()` SHALL return `0.0` when `max_health == 0` and `(health as f32 / max_health as f32).clamp(0.0, 1.0)` otherwise, and `has_state_flag(flag)` MUST return `true` if and only if `flag != 0` and all bits of `flag` are set in `state_flags`.

### Requirement: External Client and Godot Integration Contract Documentation
The repository SHALL provide up-to-date protocol and Godot 4 integration documentation matching the verified gateway transports, discovery endpoints, wire framing, codecs, and synchronization loops.

#### Scenario: Documentation Accuracy Against Gateway and Schema Implementations
- **GIVEN** `docs/WIRE_PROTOCOL_AND_CLIENT_SDK.md` and `docs/GODOT_CLIENT_INTEGRATION.md`
- **WHEN** an external developer implements a client in Godot 4 or another runtime using the documented byte layouts, gateway defaults (TCP `:8080`, WebTransport UDP `:4433` + `/cert-hash` on admin `:9091`, WebSocket `:8443`), LEB128 varint codecs, and fixed-point dequantizers
- **THEN** every opcode (`0x0001`..`0x001A`), byte offset, quantization scale, and handshake flow matches the compiled `mmo-schema`, `mmo-gateway`, and `mmo-client-wasm` behavior with zero fabricated endpoints or opcodes.
