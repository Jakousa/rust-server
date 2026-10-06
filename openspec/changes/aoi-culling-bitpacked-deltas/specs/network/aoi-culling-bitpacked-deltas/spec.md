# Area of Interest (AoI) Culling & Bit-Packed Deltas Specification

## Purpose
Specifies aggressive spatial view frustum culling in the simulation node, macro-driven bit-packed delta serialization with sub-byte quantization, unopinionated generic scalar attributes, robust yaw wrap-around differencing, NaN-safe teleport validation, rate-limited keyframe fallback, and client baseline state acknowledgement.

---

## Requirements

### Requirement: Spatial View Frustum & Awareness Sphere Culling
The simulation node SHALL support view frustum / cone culling for AoI observers, restricting entity serialization to entities within the observer's visual field of view or 3D near-awareness sphere.

#### Scenario: Entity within view frustum is observed
- **GIVEN** an observer at position $(0, 0, 0)$ facing along the positive Z-axis ($\text{yaw} = 0.0$) with frustum culling enabled ($\text{fov} = 90^\circ$, $\text{near\_radius} = 5.0\text{m}$)
- **WHEN** an entity is located at $(5.0, 0.0, 20.0)$ (within the $90^\circ$ forward cone and within visibility radius)
- **THEN** the AoI manager SHALL include the entity in the observer's visible set and emit its state update

#### Scenario: Entity outside view frustum beyond near radius is culled
- **GIVEN** an observer at position $(0, 0, 0)$ facing along the positive Z-axis ($\text{yaw} = 0.0$) with frustum culling enabled ($\text{fov} = 90^\circ$, $\text{near\_radius} = 5.0\text{m}$)
- **WHEN** an entity is located at $(0.0, 0.0, -20.0)$ (directly behind the observer at distance $20.0\text{m} > 5.0\text{m}$)
- **THEN** the AoI manager SHALL cull the entity from the observer's visible set
- **AND** the entity SHALL NOT be serialized in the observer's tick broadcast

#### Scenario: Entity within 3D awareness sphere is never culled
- **GIVEN** an observer with frustum culling enabled ($\text{near\_radius} = 5.0\text{m}$)
- **WHEN** an entity is located at $(0.0, 0.0, -3.0)$ (behind the observer, but spherical distance $3.0\text{m} \le 5.0\text{m}$)
- **THEN** the AoI manager SHALL retain the entity in the visible set to preserve peripheral awareness

#### Scenario: Vertical separation prevents false near-clip awareness
- **GIVEN** an observer at $(0, 0, 0)$ with $\text{near\_radius} = 5.0\text{m}$
- **WHEN** an entity is located at $(0.0, 100.0, 0.0)$ (100m straight up in the sky)
- **THEN** the AoI manager SHALL evaluate true 3D spherical distance ($100.0\text{m} > 5.0\text{m}$)
- **AND** the entity SHALL NOT bypass culling via horizontal-only near-clip checks

---

### Requirement: Robust Kinematic & Angular Delta Differencing
The delta computation pipeline SHALL handle circular angular wrap-around ($[0, 2\pi)$) and protect against non-finite float inputs (`NaN`, `Infinity`).

#### Scenario: Yaw wrap-around across $0 \leftrightarrow 2\pi$ boundary
- **GIVEN** an entity rotating across the angular wrap boundary from $0.01\text{ rad}$ to $6.27\text{ rad}$
- **WHEN** calculating the angular delta against the baseline state
- **THEN** the AoI manager SHALL compute the shortest angular distance ($|6.27 - (0.01 + 2\pi)| = 0.023\text{ rad}$)
- **AND** if below threshold ($0.02\text{ rad}$), SHALL NOT emit a false positive yaw update

#### Scenario: Malicious or corrupted NaN coordinates
- **GIVEN** an entity reporting `NaN` or infinite coordinates
- **WHEN** `is_teleport_delta` or delta quantizer processes the coordinates
- **THEN** the system SHALL explicitly detect `NaN` / infinity and force a keyframe or reject corruption without silently emitting zeroed deltas

---

### Requirement: Macro-Driven Bit-Packed Delta Serialization
The schema crate SHALL provide a declarative macro pipeline (`bitpack_delta!`) for defining bitstream structures with sub-byte fixed-point quantization, LEB128 varints, and automatic presence bitmasks.

#### Scenario: Sub-byte fixed-point field packing
- **GIVEN** a delta state containing an 8-bit quantized yaw, 8-bit pitch, and 16-bit velocity components
- **WHEN** the state is serialized through `BitWriter`
- **THEN** fields SHALL be packed contiguously without 32-bit/64-bit struct padding
- **AND** deserialization via `BitReader` SHALL reconstruct the values within quantization error bounds ($\le 0.01\text{m}$ for deltas, $\le 2^\circ$ for yaw/pitch)

#### Scenario: LEB128 varint integer encoding
- **GIVEN** integer fields encoded with varints (e.g. entity GUID, sequence tick, scalar attribute values)
- **WHEN** values fit within small ranges ($< 128$)
- **THEN** each value SHALL consume exactly 1 byte on the wire

---

### Requirement: Genre-Agnostic Generic Scalar Attributes
The delta state serialization protocol SHALL remain strictly unopinionated by supporting generic scalar attribute deltas identified by numeric attribute IDs rather than hardcoding game-specific RPG variables into core primitives.

#### Scenario: Serializing generic dynamic attributes
- **GIVEN** an entity with dynamic scalar attributes (e.g., attribute ID 1 with value 85, attribute ID 2 with value 100)
- **WHEN** an `EntityDeltaState` is generated with `MASK_ATTRIBUTES` set
- **THEN** attributes SHALL be encoded as variable-length attribute tuples `(attr_id, value)`
- **AND** decoding SHALL recreate the attribute map without requiring hardcoded game-specific structs

---

### Requirement: Client Baseline Acknowledgement & Rate-Limited Fallback
The simulation node and client SHALL coordinate state baselines through acknowledgement packets (`ClientAckPacket`), transmitting relative deltas for acknowledged entities while preventing network death spirals during baseline loss.

#### Scenario: Server sends relative differential deltas against confirmed baseline
- **GIVEN** an observer whose baseline tracker has confirmed acknowledgement for tick $T$
- **WHEN** the server simulates tick $T + 1$ and an entity has moved by $(\Delta x, \Delta y, \Delta z)$ within quantization bounds
- **THEN** the server SHALL emit an `EntityDeltaBatchPacket` with `baseline_tick = T` and quantized delta offsets
- **AND** the payload size SHALL be at least $60\%$ smaller than a full keyframe

#### Scenario: Rate-limited keyframe fallback on unacknowledged baseline or timeout
- **GIVEN** an observer without confirmed baseline or whose last confirmed baseline is older than 40 ticks ($2.0\text{s}$)
- **WHEN** generating entity broadcasts under unconfirmed baseline
- **THEN** the server SHALL rate-limit full keyframe refresh broadcasts to once per `full_refresh_interval_ticks` (e.g. 1 Hz) rather than flooding 20 Hz keyframes, preventing network death spirals

#### Scenario: Client WebAssembly auto-acknowledgement
- **GIVEN** a `WasmClientWorld` receiving an `EntityDeltaBatchPacket` at server tick $S$
- **WHEN** the packet is parsed and applied
- **THEN** the client world SHALL record `latest_server_tick = S` and enqueue an outgoing `ClientAckPacket` for transmission back to the server
