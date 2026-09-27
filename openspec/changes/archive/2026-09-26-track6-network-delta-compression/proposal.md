# Proposal: Track 6 - Network Delta Compression & Bandwidth Bit-Packing

## 1. Context & Motivation

In high-concurrency multiplayer worlds, network egress is often the dominant operational cost and primary performance bottleneck. Currently, the server cluster utilizes uncompressed fixed-size entity state broadcasts:
- Each observed entity update generates a discrete `EntityStatePacket` containing 60 bytes of payload plus an 8-byte framing header (68 bytes wire total).
- For a dense Area of Interest (AoI) observing up to 200 entities at a 20 Hz tick rate, this results in:
  $$200 \times 68 \times 20 = 272,000 \text{ bytes/sec} \approx 272 \text{ KB/s per client}$$
- At 1,000 connected clients, cluster egress bandwidth for entity replication exceeds **2.17 Gbps**, which is unsustainable for commodity edge infrastructure and mobile/cloud clients.

Analysis of real-world gameplay motion reveals high temporal and spatial correlation:
1. Most entities modify only a subset of their fields on any given tick (e.g. position changes while health, max_health, and state flags remain constant).
2. Continuous kinematic values (displacement, velocity, rotation) exhibit high locality between consecutive 50ms ticks, making delta offsets small and highly compressible.
3. Transmitting individual per-entity packet headers incurs severe overhead (8 bytes framing per entity) that can be eliminated through batching.

By introducing **bit-packed delta field masks**, **fixed-point kinematic quantization**, and a **client-acknowledged baseline tracking protocol**, entity replication bandwidth can be dramatically reduced by 60%–80% while retaining sub-centimeter spatial accuracy.

---

## 2. Proposed Capabilities

### In `crates/schema`:
- **Bit-Packing Utilities (`mmo_schema::bitpack`)**:
  - `BitWriter` and `BitReader` supporting variable-bit-width serialization and byte alignment.
  - Variable-length integer codec (`put_varint_u64`, `get_varint_u64`, `put_varint_u32`, `get_varint_u32`).
- **Kinematic Quantization (`mmo_schema::quantize`)**:
  - Delta coordinate quantization (`quantize_delta_i16` / `dequantize_delta_i16` at $1/128\text{m}$ resolution, yielding $\pm 256\text{m}$ range with $\approx 7.8\text{mm}$ precision).
  - Rotation quantization: 8-bit yaw (`quantize_yaw_u8` with $\approx 1.41^\circ$ precision) and 8-bit pitch (`quantize_pitch_i8`).
  - Velocity quantization: 16-bit signed fixed point per component.
- **Protocol Packets**:
  - `EntityDeltaBatchPacket` (Opcode `0x0009`): Batched delta updates for multiple entities relative to a baseline tick, using `DeltaFieldMask` bitmasks.
  - `ClientAckPacket` (Opcode `0x000A`): Client baseline acknowledgement carrying `observed_tick` and a 32-bit historical loss recovery mask.

### In `crates/node`:
- **AoI Delta Compression Engine (`crates/node/src/aoi.rs`)**:
  - Configurable `enable_delta_compression` mode in `ObserverConfig`.
  - Baseline history buffer in `ObserverState` tracking entity states sent at recent ticks.
  - ACK processing advancing the confirmed baseline tick for each observer.
  - Keyframe generation for new entity entrants, expired baselines, or packet loss fallback.
  - Dynamic generation of `EntityDeltaBatchPacket` combining all visible entity mutations into a single framed packet per tick.

### In `crates/gateway`:
- **Gateway Packet Dispatch**:
  - Routing of `ClientAckPacket` from client sessions to authoritative simulation nodes.
  - Pass-through forwarding of `EntityDeltaBatchPacket` to client TCP/QUIC streams.

### In `crates/loadtest`:
- **Virtual Client Delta Decompression**:
  - Support in `BotSession` for parsing `EntityDeltaBatchPacket` and maintaining local entity baseline state.
  - Automatic transmission of `ClientAckPacket` acknowledgements.
  - Telemetry recording for received egress bandwidth and compression ratios.

---

## 3. Non-Goals

- Replacing TCP/QUIC edge transport with custom raw UDP fragmentation (framing remains compatible with existing TCP and QUIC streams).
- Compressing non-simulation packets such as inventory transactions, chat messages, or authentication.
- Lossy perceptual compression that degrades kinematic authority or causes anti-cheat false positives.

---

## 4. Architectural Invariants

1. **Deterministic Accuracy**: Quantization error bounds must not exceed 1 centimeter ($0.01\text{m}$) for position deltas and $2^\circ$ for rotations under standard movement speeds ($< 50\text{ m/s}$).
2. **Graceful Degradation & Keyframe Recovery**: If baseline ACK is delayed or missing for $> 2.0\text{s}$ (40 ticks), the server automatically falls back to full keyframes to prevent client state desync.
3. **Backward Compatibility**: Existing `EntityStatePacket` (opcode `0x0005`) and `EntityDespawnPacket` (opcode `0x0006`) remain fully functional and supported for legacy or uncompressed clients.
4. **Zero Heap Allocation in Hot Path**: Bit-packing and batch delta serialization must utilize pre-allocated scratch buffers.
