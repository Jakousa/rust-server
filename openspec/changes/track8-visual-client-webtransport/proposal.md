# Proposal: Track 8 - Playable Visual Client Frontend & WebTransport Gateway Ingress

## Executive Summary
Track 8 delivers a browser-based, zero-install 3D **Visual Verification Harness and Operator Client** powered by **WebTransport (HTTP/3 over QUIC/UDP)** and a **Rust WebAssembly core**.

Rather than introducing heavyweight game engine dependencies (like Bevy or Unreal) that bloat server compilation times and complicate CI pipelines, Track 8 adheres to a strict architectural rule: **Zero Server Codebase Pollution**. The server engine retains its pristine, authoritative role. A single leaf crate (`crates/client-wasm`) compiles the authoritative packet codec directly to WebAssembly, ensuring **0% code duplication** for Track 6 bit-packing, LEB128 varints, and fixed-point kinematic dequantization. A lightweight WebGL/Three.js frontend renders the live simulation, spatial boundaries, and combat interactions directly in any modern web browser.

---

## Motivation & Problem Statement
The distributed MMO server has reached advanced maturity:
- **Track 5**: Pluggable abilities, dynamic attributes, and cancellable timed actions.
- **Track 6**: Bit-packed delta compression achieving an **88.05% bandwidth reduction** ($1/128\text{m} \approx 7.8\text{mm}$ fixed-point resolution).
- **Track 7**: Cross-partition interaction with authoritative range re-validation and proactive cooldown compensation.

While automated integration tests and headless bot swarms verify mathematical correctness and throughput, developer-facing verification remains blind to:
1. **Kinematic Interpolation Quality**: Does 20 Hz fixed-step delta streaming feel smooth and jitter-free at 60/144 Hz display refresh rates?
2. **Boundary Handoff Visualization**: Can operators visually observe spatial partition boundaries, entity migrations, and ghost replicas in real time?
3. **Interactive Manual Playtesting**: Can developers manually control a character, cast spells, interact with NPCs, and inspect cast progress alongside 1,000 headless bots?

Traditional WebSockets rely on TCP, introducing catastrophic **Head-of-Line (HoL) blocking** upon packet loss that ruins real-time kinematic feel. WebTransport provides native, unreliable UDP datagrams and multiplexed QUIC streams inside browsers without WebRTC signaling overhead.

---

## Architectural Principles & Scope Guardrails

1. **Strict One-Way Dependency Isolation**:
   - `crates/client-wasm` is a pure leaf crate. No server crate (`mmo-node`, `mmo-director`, `mmo-gateway`, `mmo-storage`) will ever depend on `client-wasm`.
   - The server code will never contain `wasm-bindgen`, DOM bindings, or graphics logic.
2. **Zero Code Duplication**:
   - `client-wasm` directly imports `mmo-schema` and `mmo-core`.
   - All varint decoding, bit unpacking, dequantization, and interpolation logic run in Rust via WebAssembly. JavaScript/TypeScript contains zero packet math.
3. **Dual-Transport Edge Gateway**:
   - `mmo-gateway` exposes an optional WebTransport UDP listener alongside its existing TCP listener.
   - Incoming WebTransport datagrams and streams are converted into standard `ClientPacket`s and routed through the existing `SessionRouter`.
4. **Developer-First Debug Visualization**:
   - Visualizing the server's spatial reality: partition boundary planes, entity bounding volumes, ghost replicas, ping, FPS, and bandwidth telemetry.

---

## Success Criteria & Verification Gates
1. **WebTransport Ingress on Gateway**:
   - `mmo-gateway` accepts WebTransport sessions, datagrams (unreliable movement/deltas), and bidirectional streams (reliable interactions).
2. **WASM Codec Verification**:
   - `client-wasm` successfully decodes `EntityDeltaBatchPacket` (opcode `0x0009`), unpacking bit-packed kinematic deltas without precision loss.
   - Comprehensive unit tests run on native Linux target (`cargo test -p mmo-client-wasm`) as well as `wasm32-unknown-unknown`.
3. **Client Interpolation Smoothness**:
   - Interpolation buffer in `client-wasm` translates 20 Hz server tick snapshots into smooth 60+ FPS kinematic transitions.
4. **Interactive Browser Frontend**:
   - Lightweight browser client connecting to `mmo-gateway` over WebTransport, rendering player, mobs, partition planes, and cast bars.
5. **Quality Gates**:
   - Strict formatting (`cargo fmt`), strict clippy (`-D warnings`), and 100% green test suite across the entire workspace.
