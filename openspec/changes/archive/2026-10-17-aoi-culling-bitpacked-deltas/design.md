# Design Document: Aggressive AoI Culling & Bit-Packed Deltas

## 1. Architectural Overview

The engine's bandwidth optimization architecture combines aggressive spatial view frustum culling, dynamic density throttling, macro-driven bitstream delta compression, and continuous client baseline synchronization.

```mermaid
flowchart TD
    subgraph SimulationTick ["Simulation Tick (20 Hz)"]
        OctreeQuery["1. Octree Radius Query\n(Spherical Scratch Buffer)"]
        FrustumCull["2. 3D Awareness & View Frustum Culling\n(Facing Yaw Angle vs FOV)"]
        DensityThrottle["3. Density Throttling\n(Select Top Max Entities)"]
        DeltaComp["4. Differential Delta Computation\n(Shortest Angle & Rate-Limited Fallback)"]
        Bitpack["5. Macro Bit-Packed Serialization\n(Sub-byte Packing, Varints, Generic Attributes)"]
    end

    OctreeQuery --> FrustumCull
    FrustumCull --> DensityThrottle
    DensityThrottle --> DeltaComp
    DeltaComp --> Bitpack

    Bitpack -->|EntityDeltaBatchPacket| Gateway["Edge Gateway"]
    Gateway -->|WebTransport / QUIC| Client["WASM Client World"]
    Client -->|ClientAckPacket (Auto-Ack Queue)| Gateway
    Gateway -->|Forward ACK| SimulationTick
```

---

## 2. Aggressive Spatial & View Frustum Culling (`crates/node`)

### Geometric Formulation
An observer at position $\mathbf{P}_0 = (x_0, y_0, z_0)$ has horizontal orientation defined by rotation yaw $\theta$.
1. **Forward Heading Vector**:
   In standard 3D coordinates (Y-up, Z-forward, X-lateral with clockwise yaw):
   $$\mathbf{f} = (-\sin\theta, 0, \cos\theta)$$
2. **True 3D Awareness Sphere (Near Clip)**:
   For candidate entity at $\mathbf{P}_e = (x_e, y_e, z_e)$, compute displacement vector:
   $$\mathbf{v} = \mathbf{P}_e - \mathbf{P}_0 = (dx, dy, dz)$$
   Total 3D spherical distance squared:
   $$d_{\text{3D}}^2 = dx^2 + dy^2 + dz^2$$
   If $d_{\text{3D}}^2 \le r_{\text{near}}^2$ (e.g. $r_{\text{near}} = 5.0\text{m}$), the entity is **immediately visible** regardless of orientation. Using true 3D distance prevents entities high above or deep underground from falsely bypassing culling.
3. **Frustum Field of View Test**:
   If $d_{\text{3D}}^2 > r_{\text{near}}^2$, compute the horizontal displacement squared:
   $$d_{\text{horiz}}^2 = dx^2 + dz^2$$
   If $d_{\text{horiz}}^2 == 0$, the entity is vertically aligned with the player outside $r_{\text{near}}$ and is culled.
   Otherwise, normalize the horizontal displacement:
   $$\hat{\mathbf{v}} = \frac{(dx, 0, dz)}{\sqrt{d_{\text{horiz}}^2}}$$
   The cosine of the horizontal angular deviation $\phi$ from the forward heading is:
   $$\cos\phi = \hat{\mathbf{v}} \cdot \mathbf{f} = \frac{-dx \sin\theta + dz \cos\theta}{\sqrt{dx^2 + dz^2}}$$
   For a configured horizontal FOV angle $\Theta_{\text{fov}}$ (e.g. $90^\circ$ or $120^\circ$):
   $$\cos\phi \ge \cos\left(\frac{\Theta_{\text{fov}}}{2}\right)$$
   If true, the entity falls within the camera's visual cone and is retained. Otherwise, it is culled.

### Zero-Allocation Scratch Buffers
Frustum culling is evaluated in-place within `AoiManager::update_observers` using the existing pre-allocated scratch buffer `scratch_query: Vec<(Guid, (f32, f32, f32), f32)>`.
Filtering is performed with `retain()` prior to density sorting, eliminating up to $70\%-80\%$ of candidate entities before $O(N)$ partial sorting runs.

---

## 3. Robust Kinematic & Angular Differencing

### Yaw Wrap-Around Shortest Angular Distance
To eliminate false-positive delta broadcasts when a rotation crosses the $[0, 2\pi)$ boundary (e.g. between $0.01$ and $6.27$ radians):
```rust
let diff = (yaw - base_yaw).rem_euclid(2.0 * std::f32::consts::PI);
let shortest_diff = diff.min(2.0 * std::f32::consts::PI - diff);
if shortest_diff > 0.02 {
    field_mask |= delta_flags::MASK_YAW;
}
```

### NaN & Infinite Coordinate Sanitization
In `crates/schema/src/quantize.rs`:
```rust
#[inline]
pub fn is_teleport_delta(dx: f32, dy: f32, dz: f32) -> bool {
    if !dx.is_finite() || !dy.is_finite() || !dz.is_finite() {
        return true; // NaN or inf forces keyframe recovery / validation failure
    }
    dx.abs() > MAX_DELTA_POS || dy.abs() > MAX_DELTA_POS || dz.abs() > MAX_DELTA_POS
}
```

---

## 4. Macro-Driven Bit-Packed Delta Serialization (`crates/schema`)

### Macro Definition (`bitpack_delta!`)
To eliminate boilerplate and human error in bitmask bitshifting and field serialization, `crates/schema/src/bitpack.rs` provides declarative macros for generating packed structures:

```rust
macro_rules! bitpack_delta {
    (
        struct $name:ident {
            $(
                $(#[$meta:meta])*
                $field:ident : $type:ident $( ( $($opt:tt)* ) )? = $flag:expr,
            )*
        }
    ) => { ... };
}
```

Supported field encoding directives:
- `varint`: Encoded using unsigned LEB128 varints (`put_varint_u64` / `put_varint_u32`).
- `bits(N)`: Packed directly into `BitWriter` / `BitReader` at bit width $N$ (1 to 64 bits), crossing byte boundaries cleanly without padding.
- `quant_delta_i16`: Quantized displacement relative to baseline at $1/128\text{m}$ resolution (16 bits).
- `quant_yaw_u8`: 8-bit quantized yaw angle ($[0, 2\pi) \to [0, 255]$).
- `quant_pitch_i8`: 8-bit quantized pitch angle ($[-\pi/2, \pi/2] \to [-128, 127]$).
- `quant_vel_i16`: 16-bit signed quantized velocity.
- `generic_attributes`: Dynamically sized vector of `ScalarAttributeDelta` tuples `(attr_id, value)`.

### Genre-Agnostic Generic Scalar Attributes (Prime Directive)
To preserve the prime directive of an unopinionated core engine:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalarAttributeDelta {
    pub attribute_id: u16,
    pub value: u32,
}
```
In `crates/schema/src/client.rs`, `delta_flags::MASK_ATTRIBUTES = 1 << 11` is introduced. When present in an `EntityDeltaState`, it serializes a varint-prefixed count followed by `(put_varint_u32(attr_id), put_varint_u32(value))` pairs. This allows games to replicate health, shield, stamina, mana, or arbitrary telemetry scalars with zero game-specific core code.

In `crates/node/src/aoi.rs`, hardcoded imports of `mmo_gameplay::combat::HealthComponent` are eliminated from core AoI replication: `aoi.rs` collects dynamic scalar attributes generically without hardcoding RPG combat logic into the spatial simulation substrate.

---

## 5. Client Baseline Acknowledgement & Rate-Limited Fallback

### Baseline Ring Buffer (`mmo-node`)
`BaselineTracker` maintains a bounded ring buffer (`VecDeque<ObserverTickSnapshot>`) retaining up to 60 historical ticks (3 seconds).
- When a client sends `ClientAckPacket { session_id, observed_tick, ack_mask }`, the server validates `observed_tick <= current_tick` (anti-cheat future tick rejection) and advances `confirmed_baseline_tick`.
- All snapshot entries older than `confirmed_baseline_tick` are popped from the ring buffer.
- Subsequent entity state evaluations compute diffs against `confirmed_baseline_tick`.

### Rate-Limited Fallback (Anti-Death Spiral)
If a client experiences packet loss and `is_baseline_expired` is true:
- The server does **not** broadcast full keyframes on every 20 Hz tick (which would produce a 240 KB/s flood that completely collapses the client connection).
- Instead, full keyframes are rate-limited to once per `full_refresh_interval_ticks` (e.g. 1 Hz) for stationary entities, and only transmitted when entities actually move.

### Client-WASM Auto-ACK Queue
In `crates/client-wasm/src/lib.rs`:
- `WasmClientWorld` maintains a `pending_acks: VecDeque<u64>` queue.
- When `process_packet` handles an `EntityDeltaBatchPacket`, it records `latest_server_tick = batch.server_tick` and queues an acknowledgement.
- A new method `drain_pending_acks(&mut self) -> Vec<Vec<u8>>` allows the client render loop to flush pending `ClientAckPacket`s back to the transport socket without manual tracking.
