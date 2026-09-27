# Implementation Tasks: Track 8 - Visual Client Frontend & WebTransport Gateway Ingress

## Task 1: WebTransport Gateway Ingress (`crates/gateway`)
- [x] 1.1 Add `wtransport` dependency to `crates/gateway/Cargo.toml` and configure HTTP/3 ALPN (`h3`).
- [x] 1.2 Implement self-signed ECDSA certificate generator in `crates/gateway/src/webtransport.rs` with strict $< 14\text{ days}$ validity period (e.g., 10 days) per W3C WebTransport specification, outputting SHA-256 fingerprint for browser pinning.
- [x] 1.3 Refactor session egress in `GatewayServer`:
  - Split session sinks into `datagram_tx` (bounded, `try_send` for ephemeral movement and delta batches) and `stream_tx` (unbounded, non-dropping for reliable interaction/trade/auth packets).
  - Run independent Tokio egress tasks per session for datagrams and streams to eliminate Head-of-Line blocking and mutex contention across `await` points.
- [x] 1.4 Implement `WebTransportServer` in `crates/gateway/src/webtransport.rs` accepting browser WebTransport sessions, routing incoming datagrams and bidirectional streams into `SessionRouter`.
- [x] 1.5 Verification: Unit tests in `crates/gateway/src/webtransport.rs` and `cargo test -p mmo-gateway` passes cleanly.

## Task 2: Rust WebAssembly Client Core (`crates/client-wasm`)
- [x] 2.1 Create leaf crate `crates/client-wasm` in Cargo workspace, importing `mmo-schema` and `mmo-core` with `wasm-bindgen`.
- [x] 2.2 Implement `InterpolationBuffer` in `crates/client-wasm/src/interp.rs` (Hermite/LERP, $20\text{m}$ snapback threshold, velocity extrapolation clamp).
- [x] 2.3 Implement `ClientEntityReplica` and entity state tracking in `crates/client-wasm/src/state.rs` (health, cast progress, entity type).
- [x] 2.4 Implement `WasmClientWorld` in `crates/client-wasm/src/lib.rs`:
  - Zero-copy flat `Float32Array` buffer export (`[guid_raw, x, y, z, yaw, hp_pct, cast_pct, type] * N`).
  - Bit-unpacked Track 6 delta decoding (`EntityDeltaBatchPacket`, `MovementCorrectionPacket`).
  - Outgoing packet encoding (`encode_movement`, `encode_ack`, `encode_interaction`).
- [x] 2.5 Add native unit tests verifying packet decoding, interpolation, and flat buffer layout without requiring browser environment.
- [x] 2.6 Verification: `cargo test -p mmo-client-wasm` compiles and passes natively on Linux.

## Task 3: Interactive Web Inspector Frontend (`web/`)
- [x] 3.1 Create `web/` application scaffold (`index.html`, `style.css`, `package.json`, Vite configuration).
- [x] 3.2 Implement `transport.js` connecting via `WebTransport` with `serverCertificateHashes` support for local self-signed certificates.
- [x] 3.3 Implement `renderer.js` using Three.js rendering player capsules, mob cubes, partition boundary planes, and coordinate grid.
- [x] 3.4 Implement `controls.js` capturing WASD movement and sending 20 Hz movement updates.
- [x] 3.5 Implement `ui.js` showing real-time telemetry HUD (ping, server tick, FPS, entity count, bandwidth).
- [x] 3.6 Verification: Build check for frontend assets (`npm run build` or standalone bundle verification).

## Task 4: End-to-End Integration Verification
- [x] 4.1 Create `crates/gateway/tests/webtransport_e2e.rs` running `GatewayServer` with WebTransport enabled.
- [x] 4.2 Test Case 1: Client connects via WebTransport, receives session ID, and sends `MovementPacket` datagram.
- [x] 4.3 Test Case 2: Gateway pushes `EntityDeltaBatchPacket` over datagram to WebTransport client.
- [x] 4.4 Test Case 3: Reliable stream interaction packet round-trip with zero dropped packets under simulated backpressure.
- [x] 4.5 Verification: `cargo test -p mmo-gateway --test webtransport_e2e` passes.

## Task 5: Quality Gates & Documentation Verification
- [x] 5.1 Run `cargo fmt --all -- --check`.
- [x] 5.2 Run strict linter: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 5.3 Run full workspace test suite: `cargo test --workspace`.
- [x] 5.4 Mark completed tasks in `tasks.md` and update `ROADMAP.md`.
