# Wire Protocol & External Client SDK Reference

This document is the authoritative specification for connecting external clients (Godot, Unreal, custom native engines, bots, or web runtimes) to the distributed MMO server platform.

All core protocol definitions live in [`crates/schema`](../crates/schema/src/lib.rs) (`mmo-schema`), edge termination in [`crates/gateway`](../crates/gateway/src/lib.rs) (`mmo-gateway`), and browser/WASM prediction & interpolation primitives in [`crates/client-wasm`](../crates/client-wasm/src/lib.rs) (`mmo-client-wasm`).

---

## 1. Crate Exports & Protocol Versioning

`mmo-schema` re-exports the core client wire primitives directly at crate root (`crates/schema/src/lib.rs`):

| Symbol | Definition | Notes |
|---|---|---|
| `PACKET_MAGIC` | `0x4D4D` (`u16`) | Little-endian bytes `[0x4D, 0x4D]` (`"MM"`) |
| `HEADER_SIZE` | `8` (`usize`) | Fixed 8-byte packet envelope |
| `MAX_PACKET_PAYLOAD_SIZE` | `65536` (`64 * 1024`, `usize`) | Hard payload cap enforced per frame |
| `PROTOCOL_VERSION` | `1` (`u32`) | Sent in `AuthRequestPacket::client_version` |
| `Opcode` | `#[repr(u16)] enum` | Discriminants `0x0001..=0x001A` |
| `ClientPacket` | `enum` | Typed wrapper + `encode` / `encode_into` / `decode` / `decode_bytes` / `decode_frame` |
| `ClientCodecError` | `enum` | Framing, size-limit, UTF-8, and varint errors |

---

## 2. Wire Envelope & Primitive Encodings

### 2.1 Packet Frame Header (`0x4D4D`)

Every packet on TCP, QUIC, WebTransport, and WebSocket shares the exact 8-byte little-endian envelope (`HEADER_SIZE = 8`):

```text
Offset   Size  Type    Description
0x00     2     u16 LE  magic = 0x4D4D (bytes [0x4D, 0x4D])
0x02     2     u16 LE  opcode (0x0001 ..= 0x001A)
0x04     4     u32 LE  payload_len (0 ..= 65536 bytes)
0x08     N     bytes   payload bytes (N = payload_len)
```

* **Byte order**: All fixed-width integers (`u16`, `i16`, `u32`, `u64`) and IEEE-754 floats (`f32`) are **little-endian**.
* **Float validation**: `Vec3::decode`, `EntityStatePacket::decode`, `WorldObjectSpawnPacket::decode`, and `PlaceObjectPacket::decode` reject `NaN` and `±Infinity` with `ClientCodecError::InvalidData`.
* **Coalesced frames**: Stream transports (TCP, QUIC/WebTransport streams, WebSocket binary messages) MAY deliver partial frames or multiple back-to-back `0x4D4D` frames in one read. Decoders loop with `ClientPacket::decode_frame(&mut buf)` until fewer than `HEADER_SIZE + payload_len` bytes remain.

### 2.2 Unsigned LEB128 Varints (`crates/schema/src/bitpack.rs`)

* `varint_u32`: `1..=5` bytes (`MAX_VARINT_U32_BYTES = 5`). Byte 5 must satisfy `byte & 0xF0 == 0` (no continuation bit and upper 4 bits zero), otherwise `ClientCodecError::InvalidData` is returned.
* `varint_u64`: `1..=10` bytes (`MAX_VARINT_U64_BYTES = 10`). Byte 10 must satisfy `byte & 0x80 == 0` and `byte & 0x7E == 0` (at most bit 0 set).
* Each byte packs 7 payload bits (least-significant group first); bit 7 (`0x80`) is `1` while additional bytes follow.
* `BitWriter` and `BitReader` pack arbitrary `1..=64` bit fields **LSB-first** for custom compact sub-byte extensions and align to the next byte boundary before writing/reading raw bytes or LEB128 varints.

### 2.3 Wire String & Property Map Encodings (`crates/schema/src/client.rs`)

Two distinct UTF-8 string encodings exist on the wire:

1. **`u16_str`** (`put_u16_str` / `get_u16_str`):
   * `len: u16 LE` followed by `len` UTF-8 bytes (sender clamps on UTF-8 char boundaries at `<= 65535`).
   * Used by `AuthRequest`, `AuthResponse`, `Social*`, and `Market*` packets.
2. **`varint_str`** (`put_string` / `get_string`):
   * `len: varint_u32` followed by `len` UTF-8 bytes; decoder rejects `len > MAX_WIRE_STRING_LEN` (`4096`).
   * Used by `WorldObjectSpawn::owner_id` and `property_map`.
3. **`property_map`** (`put_property_map` / `get_property_map`):
   * `count: varint_u32` (decoder rejects `count > MAX_PROPERTIES_PER_MAP` = `128`), followed by `count` pairs of `(key: varint_str, value: varint_str)`.

### 2.4 Fixed-Point Kinematic Quantization (`crates/schema/src/quantize.rs`)

Used inside `EntityDeltaState` (`0x0009`):

| Quantity | Wire Type | Constant / Formula | Resolution | Safe Range / Fallback |
|---|---|---|---|---|
| Position delta (`dx/dy/dz`) | `i16 LE` | `DELTA_POS_SCALE = 128.0`; `round(v * 128.0).clamp(-32768, 32767)` / inverse `q / 128.0` | `1/128 m` (`≈ 7.8125 mm`) | `[-255.0, +255.0] m` (`MAX_DELTA_POS`); larger or non-finite delta triggers `is_teleport_delta` -> full `MASK_KEYFRAME` |
| Velocity (`vx/vy/vz`) | `i16 LE` | `VELOCITY_SCALE = 100.0`; `round(v * 100.0).clamp(-32768, 32767)` / inverse `q / 100.0` | `0.01 m/s` (`1 cm/s`) | `[-327.68, +327.67] m/s` |
| Yaw (radians) | `u8` | Encode `((yaw.rem_euclid(2π) * (256 / 2π)).round() as u32 % 256) as u8`; Decode `(q / 256.0) * 2π` | `2π / 256` (`≈ 1.406°`) | `[0, 2π)` |
| Pitch (radians) | `i8` | Encode `round(pitch.clamp(-π/2, π/2) * (127 / (π/2))).clamp(-127, 127)`; Decode `(q.clamp(-127, 127) / 127.0) * (π/2)` | `π / 254` (`≈ 0.709°`) | `[-π/2, +π/2]` |

### 2.5 64-Bit Entity GUID Layout (`crates/core/src/guid.rs`)

* Bits `[63..48]`: `EntityType` (`0 = Unknown`, `1 = Player`, `2 = Npc`, `3 = DynamicObject`, `4 = WorldItem`).
* Bits `[47..32]`: `node_id` (`u16`).
* Bits `[31..0]`: sequence counter (`u32`).

---

## 3. Complete Opcode Reference (`0x0001` .. `0x001A`)

> **Channel column**: In `mmo-gateway` (`crates/gateway/src/server/dispatcher.rs`), **Ephemeral** packets route via `datagram_tx` (WebTransport/QUIC unreliable datagrams when supported), while **Reliable** packets route via `stream_tx` (bidirectional stream). On TCP and WebSocket all frames share one ordered stream, with WebSocket prioritizing `stream_tx` over `datagram_tx` (`biased; select!`).

| Opcode | Name | Direction | Channel | Payload Wire Layout (after 8B `0x4D4D` header) |
|---|---|---|---|---|
| `0x0001` | `Movement` | C -> S | Ephemeral | Fixed **52 B**: `entity_id: u64 LE`, `pos: 3×f32 LE`, `vel: 3×f32 LE`, `yaw: f32 LE`, `pitch: f32 LE`, `timestamp: u64 LE`, `sequence: u32 LE` |
| `0x0002` | `MovementCorrection` | S -> C | Ephemeral | Fixed **37 B**: `entity_id: u64 LE`, `corrected_pos: 3×f32 LE`, `corrected_vel: 3×f32 LE`, `sequence: u32 LE`, `reason: u8` (`0=KinematicViolation`, `1=Reconciliation`, `2=Veto`, `3=OutOfBounds`, `>=4=Custom`) |
| `0x0003` | `Interaction` | C -> S | Reliable | Min **25 B**: `source_guid: u64 LE`, `target_guid: u64 LE`, `interaction_type: u8` (`0=Talk`, `1=Attack`, `2=UseItem`, `3=Loot`, `4=Trade`, `>=5=Custom`), `action_id: u32 LE`, `payload_len: u32 LE`, `payload: [u8; payload_len]` |
| `0x0004` | `InteractionResult` | S -> C | Reliable | Min **13 B**: `action_id: u32 LE`, `success: u8` (`0`/`1`), `error_code: u32 LE`, `payload_len: u32 LE`, `payload: [u8; payload_len]` |
| `0x0005` | `EntityState` | S -> C | Ephemeral | Fixed **60 B**: `entity_id: u64 LE`, `pos: 3×f32 LE`, `vel: 3×f32 LE`, `yaw: f32 LE`, `pitch: f32 LE`, `health: u32 LE`, `max_health: u32 LE`, `state_flags: u32 LE`, `timestamp: u64 LE` |
| `0x0006` | `EntityDespawn` | S -> C | Ephemeral | Fixed **8 B**: `entity_id: u64 LE` |
| `0x0007` | `Ping` | C -> S | Ephemeral | Fixed **8 B**: `timestamp: u64 LE` |
| `0x0008` | `Pong` | S -> C | Ephemeral | Fixed **16 B**: `client_timestamp: u64 LE`, `server_timestamp: u64 LE` |
| `0x0009` | `EntityDeltaBatch` | S -> C | Ephemeral | `server_tick: varint_u64`, `baseline_tick: varint_u64`, `despawn_count: varint_u32`, `despawns: [varint_u64; despawn_count]`, `delta_count: varint_u32`, `deltas: [EntityDeltaState; delta_count]` (see [§3.1](#31-entitydeltastate-and-delta_flags)) |
| `0x000A` | `ClientAck` | C -> S | Reliable | `session_id: varint_u64`, `observed_tick: varint_u64`, `ack_mask: u32 LE` |
| `0x000B` | `WorldObjectSpawn` | S -> C | Reliable | `object_id: varint_u64`, `prefab_id: varint_u32`, `pos: 3×f32 LE`, `rot: 4×f32 LE` (xyzw), `scale: 3×f32 LE`, `bounds_min: 3×f32 LE`, `bounds_max: 3×f32 LE`, `state_flags: varint_u32`, `owner_type: u8` (`0=Character`, `1=Guild/Group/Org`, `2=WorldEntity`, `3=System`), `owner_id: varint_str`, `has_health: u8` (if `!= 0`: `cur: varint_u32`, `max: varint_u32`), `properties: property_map`, `revision: varint_u64` |
| `0x000C` | `WorldObjectDelta` | S -> C | Reliable | `object_id: varint_u64`, `revision: varint_u64`, `has_flags: u8` (if `!= 0`: `state_flags: varint_u32`), `has_health: u8` (if `!= 0`: `cur: varint_u32`, `max: varint_u32`), `properties: property_map` |
| `0x000D` | `WorldObjectDespawn` | S -> C | Reliable | `object_id: varint_u64` |
| `0x000E` | `WorldObjectResync` | C -> S | Reliable | `object_id: varint_u64` |
| `0x000F` | `PlaceObject` | C -> S | Reliable | `prefab_id: varint_u32`, `pos: 3×f32 LE`, `rot: 4×f32 LE`, `scale: 3×f32 LE`, `properties: property_map` |
| `0x0010` | `InteractObject` | C -> S | Reliable | `object_id: varint_u64`, `action_type: varint_u32` |
| `0x0011` | `AuthRequest` | C -> S | Reliable | `token: u16_str`, `client_version: u32 LE` (`PROTOCOL_VERSION = 1`) |
| `0x0012` | `AuthResponse` | S -> C | Reliable | Min **25 B**: `success: u8`, `error_code: u32 LE` (see [§3.2](#32-authentication--market-error-codes)), `session_id: u64 LE`, `player_guid: u64 LE`, `assigned_node: u16_str`, `error_message: u16_str` |
| `0x0013` | `SocialMessage` | Bidir | Reliable | Min **29 B**: `channel_type: u8` (`0=Global`, `1=Whisper`, `2=Party`, `3=Guild`, `4=Area`), `target_id: u64 LE`, `sender_guid: u64 LE`, `sender_name: u16_str`, `content: u16_str`, `timestamp: u64 LE` |
| `0x0014` | `SocialSubscribe` | C -> S | Reliable | Min **11 B**: `channel_type: u8`, `target_id: u64 LE`, `channel_name: u16_str` |
| `0x0015` | `SocialUnsubscribe` | C -> S | Reliable | Min **11 B**: `channel_type: u8`, `target_id: u64 LE`, `channel_name: u16_str` |
| `0x0016` | `MarketListRequest` | C -> S | Reliable | `item_instance_id: u16_str`, `src_container_id: u64 LE`, `src_slot: u32 LE`, `price: u64 LE`, `duration_secs: u64 LE` |
| `0x0017` | `MarketBuyRequest` | C -> S | Reliable | `listing_id: u16_str`, `buyer_container_id: u64 LE`, `payment_container_id: u64 LE`, `price: u64 LE` |
| `0x0018` | `MarketCancelRequest` | C -> S | Reliable | `listing_id: u16_str`, `dst_container_id: u64 LE` |
| `0x0019` | `MarketQueryRequest` | C -> S | Reliable | Fixed **28 B**: `item_def_id: u32 LE`, `min_price: u64 LE`, `max_price: u64 LE`, `offset: u32 LE`, `limit: u32 LE` |
| `0x001A` | `MarketResponse` | S -> C | Reliable | Min **10 B**: `request_type: u8` (`1=List`, `2=Buy`, `3=Cancel`, `4=Query`), `success: u8`, `error_code: u32 LE`, `error_message: u16_str`, `count: u16 LE`, `listings: [MarketListingWire; count]` |

#### Optional Sub-Structs

* **`LagCompensationPayload`** (optional fixed **40 B** `InteractionPacket.payload`): `client_timestamp: u64 LE`, `origin: 3×f32 LE`, `direction: 3×f32 LE`, `max_distance: f32 LE`, `sequence: u32 LE`.
* **`MarketListingWire`**: `listing_id: u16_str`, `seller_guid: u64 LE`, `item_instance_id: u16_str`, `item_def_id: u32 LE`, `price: u64 LE`, `created_at: u64 LE`, `expires_at: u64 LE`.

### 3.1 `EntityDeltaState` and `delta_flags`

Each entry in `EntityDeltaBatchPacket::deltas` encodes `entity_id: varint_u64` and `field_mask: u16 LE`, followed by set fields in **strict ascending bit order**:

| Bit | Constant | Value | Encoded Bytes (if bit is set) |
|---|---|---|---|
| 0 | `MASK_KEYFRAME` | `0x0001` | `pos: 3×f32 LE` *(mutually exclusive with bits 1..3)* |
| 1 | `MASK_DELTA_POS_X` | `0x0002` | `dx_q: i16 LE` (dequantize `dx_q / 128.0`, ignored if `MASK_KEYFRAME` set) |
| 2 | `MASK_DELTA_POS_Y` | `0x0004` | `dy_q: i16 LE` (dequantize `dy_q / 128.0`, ignored if `MASK_KEYFRAME` set) |
| 3 | `MASK_DELTA_POS_Z` | `0x0008` | `dz_q: i16 LE` (dequantize `dz_q / 128.0`, ignored if `MASK_KEYFRAME` set) |
| 4 | `MASK_VELOCITY` | `0x0010` | `vx_q: i16 LE, vy_q: i16 LE, vz_q: i16 LE` (`q / 100.0`) |
| 5 | `MASK_YAW` | `0x0020` | `yaw_q: u8` (`(yaw_q / 256.0) * 2π`) |
| 6 | `MASK_PITCH` | `0x0040` | `pitch_q: i8` (`(clamp(pitch_q, -127, 127) / 127.0) * (π / 2)`) |
| 7 | `MASK_HEALTH` | `0x0080` | `health: varint_u32` |
| 8 | `MASK_MAX_HEALTH` | `0x0100` | `max_health: varint_u32` |
| 9 | `MASK_STATE_FLAGS` | `0x0200` | `state_flags: varint_u32` |
| 10 | `MASK_ACTION_ID` | `0x0400` | `action_id: varint_u32` |
| 11 | `MASK_ATTRIBUTES` | `0x0800` | `count: varint_u32` (`<= 256`), then `count` × `(attribute_id: varint_u32 <= 65535, value: varint_u32)` |

### 3.2 Authentication & Market Error Codes

* **`auth_error_codes`** (`0x0012`): `0 = SUCCESS`, `1 = INVALID_TOKEN`, `2 = EXPIRED_TOKEN`, `3 = INVALID_SIGNATURE`, `4 = INVALID_CLAIMS`, `5 = VERSION_MISMATCH`, `6 = TIMEOUT`, `7 = RATE_LIMITED`, `8 = CLUSTER_UNAVAILABLE`, `9 = IDENTITY_MISMATCH`.
* **`market_error_codes`** (`0x001A`): `0 = SUCCESS`, `1 = LISTING_NOT_FOUND`, `2 = UNAUTHORIZED`, `3 = INSUFFICIENT_FUNDS`, `4 = CONTAINER_FULL`, `5 = ITEM_NOT_FOUND`, `6 = ALREADY_SOLD_OR_CANCELLED`, `7 = INTERNAL_ERROR`, `8 = INVALID_PARAMETERS`.

---

## 4. Gateway Transports & Discovery (`crates/gateway`)

### 4.1 Default Listeners (`crates/gateway/src/main.rs`)

| Transport | Env Variable | Default Bind | Notes |
|---|---|---|---|
| **Raw TCP** | `LISTEN_ADDR` | `0.0.0.0:8080` | Ordered byte stream carrying `0x4D4D` frames |
| **WebTransport (HTTP/3 UDP)** | `WEBTRANSPORT_ADDR` | `0.0.0.0:4433` | Self-signed `<14d` ECDSA P-256 cert; datagrams + max 16 bi-streams |
| **WebSocket Fallback (TCP)** | `WEBSOCKET_ADDR` | `0.0.0.0:8443` | `ws://` by default; `wss://` when `WEBSOCKET_TLS=true` (`WS_TLS_CERT_PATH`/`WS_TLS_KEY_PATH` or dynamic SANs via `HOST_IP`, `PUBLIC_IP`, `WS_TLS_ALT_NAMES`) |
| **Telemetry / `/cert-hash`** | `ADMIN_ADDR` | `0.0.0.0:9091` | HTTP `GET /metrics`, `GET /healthz`, `GET /ready`, `GET /cert-hash` |
| **Anycast UDP Probe** | `PROBE_ADDR` | `0.0.0.0:8081` | 64-byte big-endian `"MMOP"` (`0x4D4D4F50`) RTT/load probe (not client framing) |
| **Raw QUIC (Library API)** | *(library-only)* | *(caller-supplied)* | `GatewayServer::bind_quic` / `bind_quic_self_signed` (`crates/gateway/src/quic.rs`) provide raw `quinn` TLS 1.3 datagram + stream transport for custom gateway hosts/tests (not bound in default `main.rs`) |

### 4.2 WebTransport `/cert-hash` Discovery

When WebTransport starts, `mmo-gateway` registers its lowercase colon-separated SHA-256 SPKI/certificate fingerprint on the telemetry server (`ADMIN_ADDR` `9091`):

```http
GET /cert-hash HTTP/1.1
```

Response (`200 OK`, `Content-Type: application/json`, `Access-Control-Allow-Origin: *`):

```json
{ "cert_hash": "aa:bb:cc:..." }
```

Browser clients decode the hex octets into a 32-byte `serverCertificateHashes` entry for `new WebTransport(url, { serverCertificateHashes })` and fall back to `ws://` / `wss://` if WebTransport fails or times out (`web/src/transport.js`).

### 4.3 WebSocket Framing Limits (`crates/gateway/src/websocket.rs`)

* `MAX_WS_BUFFER_SIZE`: `262144` bytes (256 KiB) per message / accumulation buffer, permitting coalesced multi-frame binary messages while enforcing `MAX_PACKET_PAYLOAD_SIZE` (`64 KiB`) per individual `0x4D4D` packet.
* `WS_HANDSHAKE_TIMEOUT`: `10` seconds for TLS + HTTP upgrade.
* Clients MUST use **Binary** WebSocket messages; `Message::Text` increments `mmo_gateway_framing_errors_total` and terminates the session.

---

## 5. Session Lifecycle, Synchronization & Client SDK

### 5.1 Authentication Handshake & Identity Enforcement

1. Connect over TCP, WebSocket, WebTransport (open an initial bidirectional stream for reliable handshake), or custom QUIC.
2. Immediately send `AuthRequest` (`0x0011`) with `client_version = PROTOCOL_VERSION` (`1`) and a JWT / base64 `HandoverTicket` (or non-empty/dev token in unauthenticated PoC mode).
   * When `EdgeAuthenticator` is enabled, authentication must finish within `auth_timeout` (default `5` seconds). Packets sent beforehand are dropped; failed auth sends `AuthResponse` (`success = false`) and closes the connection.
3. Read `AuthResponse` (`0x0012`) to capture:
   * `session_id`: include in all `ClientAck` (`0x000A`) packets.
   * `player_guid`: include in `Movement.entity_id`, `Interaction.source_guid`, and `SocialMessage.sender_guid`. Gateway `PacketDispatcher` (`extract_acting_guid`) rejects packets whose acting GUID does not match the authenticated session identity.

### 5.2 20 Hz Movement Loop, Prediction & Reconciliation (`crates/client-wasm/src/prediction.rs`)

* Clients send sequenced `Movement` (`0x0001`) updates at fixed intervals (20 Hz / 50 ms typical) while locally advancing kinematic state.
* `PredictionBuffer` defaults (`mmo-client-wasm`):
  * `DEFAULT_CONVERGENCE_SPEED = 15.0` m/s (exponential visual error decay)
  * `DEFAULT_HARD_SNAP_THRESHOLD = 20.0` m (teleports immediately instead of smoothing)
  * `DEFAULT_TOLERANCE = 0.05` m (ignores minor floating-point drift)
  * `DEFAULT_MAX_INPUTS = 64` (bounded unacknowledged ring buffer)
* `MovementCorrection` (`0x0002`) invokes `PredictionBuffer::reconcile(sequence, corrected_position)`: prunes inputs `<= sequence`, replays remaining inputs (`sequence > acked_seq`), and records residual visual error.
* Diagnostic helpers:
  * `PredictionBuffer::error_distance(&self, server_pos: (f32, f32, f32)) -> f32` computes `||current_sim_pos - server_pos||_2`.
  * `PredictionBuffer::latest_sequence(&self) -> Option<u32>` returns the newest buffered unacknowledged sequence number (`None` when empty).

### 5.3 Delta Compression, Baseline ACK & Temporal Interpolation (`crates/node/src/aoi.rs`, `crates/client-wasm`)

1. Simulation nodes (`20 Hz`) snapshot AoI state into `BaselineTracker` (history depth configured `10..=120` ticks, default `60`).
2. Whenever an `EntityDeltaBatch` (`0x0009`) arrives:
   * Remove all GUIDs listed in `despawns` from replica & interpolation caches.
   * For each `EntityDeltaState`: apply `MASK_KEYFRAME` (`position = delta.position`) or accumulate `dequantize_delta_i16` onto the local baseline position. Update velocity/yaw/pitch/health/max_health/state_flags/action_id/scalar attributes according to `field_mask`.
   * Send `ClientAck` (`0x000A`) reliably with `{ session_id, observed_tick: batch.server_tick, ack_mask: 0 }`.
3. Server baseline safety guarantees:
   * `BaselineTracker::process_ack` rejects future ticks (`observed_tick > current_tick`) as anti-cheat spoofing.
   * If `current_tick - confirmed_baseline_tick > 40` (2 seconds without ACK) or the requested baseline was evicted, the server automatically falls back to `MASK_KEYFRAME` full snapshots.
4. `ClientEntityReplica` (`crates/client-wasm/src/state.rs`):
   * `health_fraction(&self) -> f32`: returns `(health / max_health).clamp(0.0, 1.0)`, safely returning `0.0` when `max_health == 0` *(unlike `hp_percent()`, which defaults to `1.0`)*.
   * `has_state_flag(&self, flag: u32) -> bool`: returns `true` iff `flag != 0 && (state_flags & flag) == flag`.
   * `get_attribute(&self, attribute_id: u16) -> Option<u32>`: retrieves generic scalar attributes synced via `MASK_ATTRIBUTES`.
5. `InterpolationBuffer` (`crates/client-wasm/src/interp.rs`):
   * Buffers remote snapshots (`max_history = 32`), renders `render_delay_sec = 0.10` s behind server time, applies shortest-arc `lerp_angle` for yaw/pitch, extrapolates at most `max_extrapolation_sec = 0.20` s before clamping velocity (`clamp_timeout_sec = 0.25` s), and snaps if displacement exceeds `snapback_threshold = 20.0` m.
