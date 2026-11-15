# Technical Design: Wire Protocol Stabilization & External Client SDK / Godot Contract

## 1. Architectural Invariants & Genre-Agnostic Compliance
- **Zero Genre Assumptions**: All stabilized wire symbols (`mmo-schema`), prediction/replica helpers (`mmo-client-wasm`), and SDK documentation remain strictly generic over 3D kinematics, numeric entity GUIDs, opaque scalar `attribute_id: u16` entries, bitmask `state_flags: u32`, and untyped `interaction_type`/`action_id` identifiers.
- **Wire Backward Compatibility**: All existing binary layouts in `crates/schema/src/client.rs`, `bitpack.rs`, and `quantize.rs` stay bit-for-bit identical. Adding `PROTOCOL_VERSION = 1`, re-exports, documentation comments, and golden byte-vector tests preserves full runtime wire parity across Rust, JS/WASM, and third-party clients.

## 2. Binary Envelope & Wire Codecs (`crates/schema`)

### 2.1 Framing Header & Re-Exports
Every client-gateway packet on TCP, QUIC, WebTransport, and WebSocket uses the exact 8-byte little-endian envelope (`HEADER_SIZE = 8`):
- Bytes `[0..2]`: `magic: u16` LE = `0x4D4D` (wire bytes `[0x4D, 0x4D]`, ASCII `"MM"`).
- Bytes `[2..4]`: `opcode: u16` LE (`0x0001..=0x001A`).
- Bytes `[4..8]`: `payload_len: u32` LE, strictly validated against `MAX_PACKET_PAYLOAD_SIZE` (`65536` bytes / 64 KiB) during `ClientPacket::decode_bytes` and `ClientPacket::decode_frame`.
- `pub const PROTOCOL_VERSION: u32 = 1;` formalizes the baseline value carried by `AuthRequestPacket::client_version`.
- `crates/schema/src/lib.rs` re-exports `PACKET_MAGIC`, `HEADER_SIZE`, `MAX_PACKET_PAYLOAD_SIZE`, `PROTOCOL_VERSION`, `Opcode`, `ClientPacket`, and `ClientCodecError` at crate root so SDK consumers do not need deep module paths.

### 2.2 LEB128 Varints, Bitpacking & String Encodings
- **Unsigned LEB128 (`bitpack.rs`)**: `put_varint_u64`/`get_varint_u64` (max `MAX_VARINT_U64_BYTES = 10`, enforcing byte-10 high bits `(byte & 0x7E) == 0` so non-canonical >64-bit overflows fail fast) and `put_varint_u32`/`get_varint_u32` (max `MAX_VARINT_U32_BYTES = 5`, enforcing `(byte & 0xF0) == 0`).
- **Bitpacking Primitives (`bitpack.rs`)**: `BitWriter` and `BitReader` write/read arbitrary `1..=64` bit fields (LSB-first per bit cursor) with byte alignment and varint helpers, alongside the `bitpack_delta!` macro. Note carefully that `EntityDeltaState` itself uses byte-aligned varints, `u16` field masks, and fixed-width LE integers rather than an unaligned bitstream so external dynamic-language clients (GDScript, C#, TypeScript) can parse `EntityDeltaBatch` cleanly with a standard cursor.
- **Two Wire String Formats (`client.rs`)**: Documentation distinguishes the two string representations already established on the wire:
  1. `u16` LE byte length + UTF-8 bytes (`AuthRequest`, `AuthResponse`, `Social*`, `Market*`).
  2. `varint_u32` LEB128 byte length + UTF-8 bytes bounded by `MAX_WIRE_STRING_LEN = 4096` (`WorldObjectSpawn::owner_id`, `PlaceObject`/`WorldObject*` property maps bounded by `MAX_PROPERTIES_PER_MAP = 128`).

### 2.3 Fixed-Point Quantization (`quantize.rs`) & Delta Batches
- Position deltas: `DELTA_POS_SCALE = 128.0` (`1/128 m ≈ 7.8125 mm`), stored as `i16` LE when `|delta| <= MAX_DELTA_POS` (`255.0` m). Larger displacements or non-finite components trigger `is_teleport_delta`, emitting `delta_flags::MASK_KEYFRAME` (`1 << 0`) with full `3 × f32` LE coordinates instead of `MASK_DELTA_POS_{X,Y,Z}`.
- Velocities: `VELOCITY_SCALE = 100.0` (`0.01 m/s = 1 cm/s`), stored as `3 × i16` LE when `MASK_VELOCITY` (`1 << 4`) is set.
- Rotations: Yaw `[0, 2π)` quantized to `u8` (`256` steps ≈ `1.40625°` via `MASK_YAW = 1 << 5`); Pitch `[-π/2, π/2]` quantized to `i8` in `[-127, 127]` (`≈ 0.7087°` via `MASK_PITCH = 1 << 6`).
- Additional flags in `EntityDeltaState`: `MASK_HEALTH` (`1 << 7`), `MASK_MAX_HEALTH` (`1 << 8`), `MASK_STATE_FLAGS` (`1 << 9`), `MASK_ACTION_ID` (`1 << 10`), and `MASK_ATTRIBUTES` (`1 << 11`, varint count `<= 256` followed by `(attr_id: varint_u32 <= u16::MAX, value: varint_u32)` pairs).

### 2.4 Golden Wire Contract Tests
`crates/schema/src/client.rs` adds deterministic golden byte-array tests (`assert_eq!(&pkt.encode(), &EXPECTED_BYTES)`) and decode roundtrips for `AuthRequest`, `AuthResponse`, `Movement`, `Interaction`, `EntityDeltaBatch`, and `ClientAck`. Any accidental endianness, opcode, mask, varint, or field-order drift immediately breaks compilation/CI.

## 3. Edge Gateway Transports & Synchronization Lifecycle

### 3.1 Observed Gateway Endpoints (`crates/gateway`)
- `mmo-gateway` binary (`crates/gateway/src/main.rs`) exposes:
  - Raw TCP: `LISTEN_ADDR` (default `0.0.0.0:8080`).
  - WebTransport HTTP/3 over QUIC/UDP: `WEBTRANSPORT_ADDR` (default `0.0.0.0:4433`), generating a short-lived `<14d` ECDSA P-256 certificate whose SHA-256 dotted-hex digest is served with CORS at `GET /cert-hash` on `ADMIN_ADDR` (default `0.0.0.0:9091`).
  - WebSocket fallback over TCP (`ws://` or `wss://` when `WEBSOCKET_TLS=true`): `WEBSOCKET_ADDR` (default `0.0.0.0:8443`, `MAX_WS_BUFFER_SIZE = 256 KiB`, rejects RFC 6455 Text frames, supports coalesced binary frames).
  - Native QUIC helper API: `GatewayServer::bind_quic` / `bind_quic_self_signed` (`crates/gateway/src/quic.rs`, `crates/gateway/src/server/mod.rs`) is provided in the `mmo-gateway` library crate for embedded custom servers and integration harnesses.
  - UDP Anycast Probe: `PROBE_ADDR` (default `0.0.0.0:8081`, 64-byte big-endian `"MMOP"` RTT/load probe, distinct from `0x4D4D` client session frames).

### 3.2 Authority, Channel Routing & Anti-Cheat Invariants
- **Handshake**: Clients authenticate by sending `AuthRequest` (`0x0011`) within `EdgeAuthenticator::auth_timeout()` (default `5s` when auth is enabled). Servers reply with `AuthResponse` (`0x0012`) assigning `session_id` and `player_guid`.
- **Stream vs Datagram Dispatch (`crates/gateway/src/server/dispatcher.rs`)**: `Movement`, `MovementCorrection`, `EntityState`, `EntityDespawn`, `EntityDeltaBatch`, `Ping`, and `Pong` use unreliable datagrams when supported by the transport; `Auth*`, `ClientAck`, `Interaction*`, `WorldObject*`, `PlaceObject`, `InteractObject`, `Social*`, and `Market*` use reliable streams.
- **20 Hz Tick & Baseline ACK (`crates/node/src/aoi.rs`)**: Simulation nodes maintain per-client baseline snapshots (`10..=120` ticks deep) keyed by `server_tick`. Clients acknowledge observed ticks with `ClientAck` (`session_id`, `observed_tick`, 32-tick `ack_mask`). Nodes clamp/reject anti-cheat `observed_tick > current_tick` reports and automatically fall back to keyframes if no acknowledged baseline exists or a baseline exceeds the 40-tick (2s) window.

## 4. Client SDK Primitives (`crates/client-wasm`)
- `PredictionBuffer` (`crates/client-wasm/src/prediction.rs`):
  - `error_distance(&self, server_pos: (f32, f32, f32)) -> f32` computes the 3D Euclidean distance between the latest predicted simulation position (`current_sim_pos`) and `server_pos`.
  - `latest_sequence(&self) -> Option<u32>` returns `self.unacknowledged_inputs.back().map(|i| i.sequence)` without mutating the ring buffer.
- `ClientEntityReplica` (`crates/client-wasm/src/state.rs`):
  - `health_fraction(&self) -> f32` returns `(self.health as f32 / self.max_health as f32).clamp(0.0, 1.0)` when `self.max_health > 0`, and explicitly returns `0.0` when `self.max_health == 0` (distinct from `hp_percent()`'s `1.0` default for indestructible/uninitialized entities).
  - `has_state_flag(&self, flag: u32) -> bool` evaluates `flag != 0 && (self.state_flags & flag) == flag`, handling both single-bit flags (`1 << n`) and multi-bit masks while returning `false` for `flag == 0`.
