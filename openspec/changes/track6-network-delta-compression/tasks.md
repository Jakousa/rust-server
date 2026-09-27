# Implementation Tasks: Track 6 - Network Delta Compression & Bandwidth Bit-Packing

## Task 1: Bit-Packing & Varint Codec Utilities
- [x] 1.1 Implement `BitWriter` and `BitReader` in `crates/schema/src/bitpack.rs` with bit-level append, peek, read, and byte alignment.
- [x] 1.2 Implement unsigned LEB128 varint functions (`put_varint_u64`, `get_varint_u64`, `put_varint_u32`, `get_varint_u32`).
- [x] 1.3 Add comprehensive unit tests in `crates/schema/src/bitpack.rs` verifying bit sequences, overflow boundaries, and malformed inputs with bounds checks.
- [x] 1.4 Verification: `cargo test -p mmo_schema -- bitpack` passes with 100% coverage.

## Task 2: Kinematic Quantization Utilities
- [x] 2.1 Implement fixed-point quantization for 3D delta offsets (`quantize_delta_i16`, `dequantize_delta_i16` at $1/128\text{m}$ resolution) with saturation clamping in `crates/schema/src/quantize.rs`.
- [x] 2.2 Implement 8-bit angular quantization for yaw (`quantize_yaw_u8`, `dequantize_yaw_u8`) and pitch (`quantize_pitch_i8`, `dequantize_pitch_i8`).
- [x] 2.3 Implement 16-bit signed velocity quantization (`quantize_velocity_i16`, `dequantize_velocity_i16`).
- [x] 2.4 Add property tests (`proptest`) verifying round-trip error bounds ($< 8\text{mm}$ position, $< 1.5^\circ$ rotation, no panics on extreme/NaN/Inf floats).
- [x] 2.5 Verification: `cargo test -p mmo_schema -- quantize` passes cleanly.

## Task 3: Delta Protocol Packets & Codecs
- [x] 3.1 Define `Opcode::EntityDeltaBatch = 0x0009` and `Opcode::ClientAck = 0x000A` in `crates/schema/src/client.rs`.
- [x] 3.2 Define `DeltaFieldMask` bitflags and `EntityDeltaState` struct.
- [x] 3.3 Implement `EntityDeltaBatchPacket` binary serialization and streaming frame deserialization.
- [x] 3.4 Implement `ClientAckPacket` binary serialization and streaming frame deserialization.
- [x] 3.5 Add unit and proptest suites verifying round-trip encoding/decoding and malformed payload rejection.
- [x] 3.6 Verification: `cargo test -p mmo_schema` passes.

## Task 4: AoI Baseline Tracker & Delta Compression Engine
- [x] 4.1 Extend `ObserverConfig` with `enable_delta_compression: bool` and `history_depth_ticks: usize` (default 60).
- [x] 4.2 Add `BaselineTracker` with ring-buffered tick snapshots to `ObserverState` in `crates/node/src/aoi.rs`.
- [x] 4.3 Implement `AoiManager::process_ack(session_id, observed_tick, ack_mask, current_tick)` with anti-cheat check `if observed_tick > current_tick { return; }` to prevent history purge DoS.
- [x] 4.4 Update `AoiManager::update_observers()` to compute deltas against client baseline and emit `EntityDeltaBatchPacket` when delta compression is enabled.
- [x] 4.5 Implement teleportation detection: displacement $> 255.0\text{m}$ along any axis automatically forces `MASK_KEYFRAME` coordinates.
- [x] 4.6 Implement keyframe elevation when baseline is unacknowledged, missing, or expired (> 40 ticks).
- [x] 4.7 Verification: Unit tests in `crates/node/src/aoi.rs` verifying baseline advancement, anti-cheat drop, teleportation keyframes, and despawn encoding.

## Task 5: Simulation Node Integration & Gateway ACK Routing
- [x] 5.1 Implement non-blocking ACK ingress via MPSC channel on `TickSimulation`, draining and applying ACKs at the start of `step()`.
- [x] 5.2 Expose `queue_client_ack` in `TickSimulation` and `NodeServiceImpl`.
- [x] 5.3 Update Edge Gateway `SessionRouter` and `PacketDispatcher` to forward `ClientAckPacket` to destination simulation nodes.
- [x] 5.4 Ensure Edge Gateway session drop guard dispatches observer unregistration to the simulation node to prevent memory leaks.
- [x] 5.5 Verification: Gateway e2e tests in `crates/gateway/tests/` verify bidirectional routing of deltas and ACKs.

## Task 6: Virtual Bot Delta Client & Swarm Verification
- [x] 6.1 Update `BotSession` in `crates/loadtest/src/session.rs` to decode `EntityDeltaBatchPacket`, maintain client-side entity shadow state, and periodically send `ClientAckPacket`.
- [x] 6.2 Update `crates/loadtest/src/swarm.rs` to aggregate bandwidth egress/ingress metrics and compare uncompressed vs. compressed modes.
- [x] 6.3 Verification: Headless bot tests pass with delta decoding enabled.

## Task 7: Bandwidth Profiling Benchmark & Verification
- [x] 7.1 Implement automated benchmark comparing byte footprint over 1,000 entities between `EntityStatePacket` and `EntityDeltaBatchPacket`.
- [x] 7.2 Verify that total egress bandwidth savings achieve $\ge 60\%$ (targeting $70\%-80\%$).
- [x] 7.3 Verify memory bounds of `BaselineTracker`: $< 20\text{ MB}$ under typical ACK latency and $\le 400\text{ MB}$ under worst-case 3-second lag for 1,000 bots.

## Task 8: Quality Gates & Cluster Verification
- [x] 8.1 Run `cargo fmt --all -- --check`.
- [x] 8.2 Run strict linter: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 8.3 Run full workspace test suite: `cargo test --workspace`.
- [x] 8.4 Mark completed tasks in `tasks.md`.
