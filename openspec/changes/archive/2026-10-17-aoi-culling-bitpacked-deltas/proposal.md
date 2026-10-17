# Proposal: Aggressive AoI (Area of Interest) Culling & Bit-Packed Deltas

## 1. Problem Statement

In massive multiplayer simulations, high player density poses a fundamental bandwidth scaling problem. Naive state replication broadcasts all entity updates to all nearby participants, yielding $O(N^2)$ network egress scaling that rapidly overwhelms edge gateways, client connections, and mobile/cloud bandwidth budgets.

The engine's design target requires an **88.05% bandwidth egress reduction** under dense player clusters. Achieving this requires a ruthless, multi-tiered optimization pipeline:
1. **Inefficient Spatial Culling**: Traditional pure-radius sphere queries replicate entities located entirely behind the player or outside the camera's visual cone, wasting significant egress and client-side interpolation CPU.
2. **Padding & Boilerplate Overhead**: Standard struct replication transmits aligned 32-bit/64-bit primitive types with byte padding, multiplying byte sizes unnecessarily when fields change by minor deltas or occupy restricted numeric domains.
3. **Opinionated Core State Coupling**: Hardcoded gameplay attributes (such as RPG health or mana) in core serialization pipelines violate the engine's unopinionated prime directive and prevent generic genre-agnostic MMO games (space simulations, voxel sandboxes, survival games) from leveraging delta compression.
4. **Redundant Absolute State Broadcasts**: Clients without continuous baseline acknowledgement receive redundant absolute state updates every tick instead of compact relative differentials.

---

## 2. Proposed Capabilities

### In `crates/schema`:
- **Macro-Driven Bit-Packed Delta Serialization Pipeline**:
  - Declarative macro suite (`bitpack_delta!`) generating zero-copy serialization and deserialization codecs.
  - Sub-byte bitstream packing leveraging `BitWriter` and `BitReader`: 1-bit boolean flags, arbitrary $N$-bit quantized fields (e.g. 8-bit yaw/pitch, 16-bit velocity components), and unsigned LEB128 varints.
  - Field presence bitmasks generated automatically without manual bitshift boilerplate.
- **Unopinionated Generic Scalar Attributes**:
  - Introduce `ScalarAttributeDelta` and generic scalar attribute support (`MASK_ATTRIBUTES`), enabling arbitrary custom gameplay stats (health, shields, fuel, stamina) to be packed with varint IDs and values without hardcoding game-specific types into core codecs.
  - Retain full backward compatibility with existing `EntityDeltaBatchPacket` and `EntityDeltaState`.

### In `crates/node`:
- **Aggressive Spatial & View Frustum Culling (`crates/node/src/aoi.rs`)**:
  - Add configurable view frustum / cone culling to `ObserverConfig` (`enable_frustum_culling`, `fov_degrees`, `near_clip_radius`).
  - Implement mathematical 2D/3D horizontal view frustum culling evaluated in $O(1)$ per entity using vector dot products against the observer's facing yaw direction.
  - Entities within an immediate "awareness bubble" (`near_clip_radius`, e.g. 5.0m) remain visible regardless of facing direction, preventing sudden pop-in from behind, while entities outside the cone at distance are aggressively culled.
  - Dynamic radius throttling combined with frustum culling ensures network egress scales logarithmically, not exponentially, with player density.
- **Differential Delta Computation & Baseline Tracking**:
  - Compute relative deltas against the client's confirmed baseline tick snapshot.
  - Automatic keyframe fallback if client ACKs time out ($> 2.0\text{s}$ or 40 ticks).

### In `crates/client-wasm`:
- **Client-Side Delta Decoding & Baseline Acknowledgment**:
  - Update `ClientEntityReplica` to decode and store generic scalar attributes.
  - Add automatic outgoing ACK queuing in `WasmClientWorld` upon ingesting `EntityDeltaBatchPacket`, maintaining continuous baseline synchronization.

---

## 3. Scope & Non-Goals

### In Scope
- Macro-driven bitstream delta serialization in `crates/schema`.
- View frustum spatial culling in `crates/node`.
- Generic scalar attribute delta packing complying with the prime directive.
- Client-side delta decoding and auto-acknowledgment queue in `crates/client-wasm`.
- Comprehensive property tests, benchmarks, and regression tests demonstrating bandwidth reduction.

### Non-Goals
- Changing transport protocols (WebTransport, QUIC, and TCP framing remains standard).
- Compressing static world terrain mesh geometry (handled via sector streaming).
- Implementing visual client graphics rendering shaders.

---

## 4. Architectural Invariants

1. **Unopinionated MMO Core**: Serialization primitives must remain genre-agnostic, supporting generic scalar attributes alongside kinematics.
2. **Bandwidth Scaling**: Network egress must scale logarithmically under player density throttling, reducing byte volume by $> 75\%$ relative to uncompressed state broadcasts.
3. **Zero Heap Allocation in Hot Path**: Frustum evaluation and bitstream packing must utilize pre-allocated scratch buffers.
4. **Deterministic Accuracy**: Quantization error bounds must not exceed 1 centimeter ($0.01\text{m}$) for position deltas and $2^\circ$ for rotations.
