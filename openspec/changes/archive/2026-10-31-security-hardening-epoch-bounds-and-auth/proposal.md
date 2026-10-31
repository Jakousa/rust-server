# Security Hardening: Movement Validation, Social Bus Auth, Epoch Fencing, NaN/TTL Bounds

## Context & Motivation
A security review of `crates/node`, `crates/gateway`, `crates/director`, `crates/schema`, and `crates/storage` identified multiple attack vectors:
1. **Movement Speed Exploit**: Clients sending rapid bursts of packets (e.g. every 2ms) receive a minimum 50ms movement delta time on every packet, enabling up to 100x speed hacks. Packets with `sequence == 0` bypass replay and monotonicity checks.
2. **Social Bus Eavesdropping & Spoofing**: Clients can subscribe to arbitrary `whisper:{target_id}` topics or spoof `sender_guid` when sessions are unbound.
3. **Ghost Duplication & Resurrection**: Despawn and migration remove epoch entries, allowing late in-flight movement packets to auto-hydrate duplicate player instances on the old node. The gateway also hardcodes `session_epoch = 0`.
4. **NaN/Inf Poisoning**: Missing float finiteness checks in packet codecs (`MovementPacket`, `PlaceObjectPacket`), quantization, Director heartbeats, split reports, remote ability range checks, and ghost updates allow client-directed state corruption and range check bypasses.
5. **TTL Overflows & Unbounded Buffers**: Millisecond timestamp calculations with unchecked multiplication overflow `i64` on large inputs, while pre-auth handshake buffers can grow unbounded until memory exhaustion.

## Scope
- `crates/schema`: Strict `.is_finite()` validation in `Vec3`, `MovementPacket`, `PlaceObjectPacket`, and `quantize_yaw_u8`.
- `crates/node`: Replenishing time-credit movement bucket in `TickSimulation`, sequence monotonicity enforcement, epoch tombstoning on migration and despawn, ghost record sanitization, and remote ability position validation.
- `crates/gateway`: Strict session-ownership enforcement in `GatewayInterceptHandler` for `SocialMessage` and `SocialSubscribe`, propagation of `session_epoch` in `SessionRouter`, `RoutedPacket`, and `ForwardClientPacketRequest`, and bounded pre-auth buffer enforcement (`MAX_PACKET_PAYLOAD_SIZE`).
- `crates/director`: Sanitization of `HeartbeatRequest` telemetry against non-finite floats, bounding box validation in `report_split`, bounded TTL calculations and bounded nonce cache in `RealmFederationAuthority`.
- `crates/storage`: Bounded inputs and checked arithmetic for TTL calculations in `MarketplaceService`, `EscrowCoordinator`, and `MemoryCache`.

## Non-Goals
- Changing external wire protocol formats or packet schemas.
- Modifying game-specific mechanics or rules.
