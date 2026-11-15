# Proposal: Wire Protocol Stabilization & External Client SDK / Godot Contract (#241)

## Context & Motivation
The platform already implements a genre-agnostic binary client protocol (`crates/schema/src/client.rs`), fixed-point kinematic quantization (`crates/schema/src/quantize.rs`), LEB128 varint and bitpacking primitives (`crates/schema/src/bitpack.rs`), multi-transport edge termination (`crates/gateway`), and a browser-oriented client prediction/interpolation SDK (`crates/client-wasm`). However, third-party and native engine integrations (such as Godot 4, Unreal, or custom tooling) lack a frozen wire protocol version constant, top-level schema crate re-exports, byte-level golden regression vectors, ergonomic prediction/replica inspection helpers, and an authoritative wire + Godot integration contract specification.

## Scope
1. **OpenSpec Contract & Architecture (`openspec/changes/wire-protocol-and-client-sdk/`)**:
   - Define the durable `wire-protocol-contract` capability covering wire framing, version stability, quantization, delta/ACK lifecycle, client SDK helpers, and external client documentation accuracy.
2. **Wire Protocol Stabilization (`crates/schema`)**:
   - Export `pub const PROTOCOL_VERSION: u32 = 1;` in `crates/schema/src/client.rs`.
   - Re-export `PACKET_MAGIC`, `HEADER_SIZE`, `MAX_PACKET_PAYLOAD_SIZE`, `PROTOCOL_VERSION`, `Opcode`, `ClientPacket`, and `ClientCodecError` at the top level of `crates/schema/src/lib.rs`.
   - Expand `///` / `//!` rustdoc across `crates/schema/src/client.rs`, `bitpack.rs`, and `quantize.rs` to serve as an unambiguous protocol reference (8-byte `0x4D4D` LE framing, 64 KiB payload cap, LEB128 varints & bit-level helpers, fixed-point scales/ranges, `delta_flags::MASK_*`, `ClientAck`, and `AuthRequest`/`AuthResponse`).
   - Add deterministic golden byte-vector and roundtrip stability tests for `AuthRequest`, `AuthResponse`, `Movement`, `Interaction`, `EntityDeltaBatch`, and `ClientAck`.
3. **Client SDK Helper Primitives (`crates/client-wasm`)**:
   - Add `PredictionBuffer::error_distance(&self, server_pos: (f32, f32, f32)) -> f32` and `PredictionBuffer::latest_sequence(&self) -> Option<u32>` in `crates/client-wasm/src/prediction.rs`.
   - Add `ClientEntityReplica::health_fraction(&self) -> f32` and `ClientEntityReplica::has_state_flag(&self, flag: u32) -> bool` in `crates/client-wasm/src/state.rs`.
   - Cover normal and boundary cases (empty buffer, zero `max_health`, single/combined flag masks) with unit tests.
4. **External Client SDK & Godot 4 Integration Guides (`docs/`)**:
   - Publish `docs/WIRE_PROTOCOL_AND_CLIENT_SDK.md` detailing all opcodes (`0x0001`..`0x001A`), byte layouts, gateway transports, `/cert-hash` discovery, authentication handshake, 20 Hz movement & prediction/interpolation loop, and baseline ACK lifecycle verified directly against engine source code.
   - Publish `docs/GODOT_CLIENT_INTEGRATION.md` providing an end-to-end Godot 4 quickstart with practical GDScript implementations for `StreamPeerTCP` and `WebSocketPeer`, `0x4D4D` framing, LEB128 varint codecs, fixed-point dequantizers, `AuthRequest`/`AuthResponse`, 20 Hz sequenced movement, and `EntityDeltaBatch`/`ClientAck` handling.

## Non-Goals
- Altering wire byte layouts, existing opcodes (`0x0001`..`0x001A`), quantization scales, or runtime gateway/node network behavior.
- Introducing any game-specific, genre-specific, or domain-opinionated types into `mmo-schema`, `mmo-core`, or `mmo-client-wasm`.
- Shipping a compiled Godot GDExtension binary crate inside this repository.
