# Specification: Rust WebAssembly Client Core (`mmo-client-wasm`)

## Overview
A dedicated Rust leaf crate located at `crates/client-wasm` that compiles to WebAssembly (`wasm32-unknown-unknown`) using `wasm-bindgen`. Acts as the single source of truth for all packet parsing, Track 6 bit-unpacking, kinematic dequantization, and temporal interpolation, preventing any duplication of codec logic in JavaScript or TypeScript.

---

## 1. Crate Architecture & Isolation

```
crates/client-wasm/
├── Cargo.toml (dependencies: mmo-schema, mmo-core, wasm-bindgen, serde)
└── src/
    ├── lib.rs (Wasm exports and memory management)
    ├── interp.rs (Hermite/LERP temporal interpolation buffer)
    ├── state.rs (Client entity replica tracking & health/cast bars)
    └── codec.rs (WebAssembly wrappers around mmo-schema decoding)
```

**Zero Ingress Invariant**: No server crate (`mmo-node`, `mmo-gateway`, `mmo-director`, `mmo-storage`) will list `mmo-client-wasm` in its `Cargo.toml`.

---

## 2. Core Struct: `WasmClientWorld`

Exposed directly to the browser via `#[wasm_bindgen]`:

```rust
#[wasm_bindgen]
pub struct WasmClientWorld {
    entities: HashMap<Guid, ClientEntityReplica>,
    interp_buffer: InterpolationBuffer,
    local_player_guid: Option<Guid>,
    latest_server_tick: u64,
    received_packets: u64,
}

#[wasm_bindgen]
impl WasmClientWorld {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self;

    /// Ingests a raw binary WebTransport datagram or stream frame.
    pub fn process_packet(&mut self, data: &[u8]) -> Result<(), String>;

    /// Advances interpolation time by dt_sec and returns renderable entity states.
    pub fn update(&mut self, dt_sec: f32);

    /// Serializes an outgoing movement packet into a Uint8Array ready for datagram transmission.
    pub fn encode_movement(
        &mut self,
        x: f32,
        y: f32,
        z: f32,
        yaw: f32,
        sequence: u32,
    ) -> Vec<u8>;

    /// Serializes a ClientAckPacket acknowledging the latest received baseline tick.
    pub fn encode_ack(&self) -> Option<Vec<u8>>;

    /// Returns a flat Float32Array of entity render data for fast WebGL updates:
    /// [guid_raw, x, y, z, yaw, current_hp_pct, cast_progress_pct, entity_type] * N
    pub fn get_render_buffer(&self) -> Vec<f32>;

    /// Returns telemetry metrics (entities in view, latest tick, interp delay).
    pub fn get_telemetry_json(&self) -> String;
}
```

---

## 3. Temporal Interpolation Buffer
1. **Render Delay Window**:
   - Maintains a jitter buffer of 100ms (2 server ticks at 20 Hz).
   - Smoothly linearly interpolates positions $(x, y, z)$ and spherical linear interpolates yaw between snapshot $S_{k}$ and $S_{k+1}$.
2. **Teleport & Snapback Handling**:
   - If distance delta between consecutive updates exceeds $20.0\text{m}$, immediately snap position rather than interpolating across obstacles or partition warps.
3. **Extrapolation Clamp**:
   - If network packets stall for $> 250\text{ms}$, extrapolate velocity up to $0.2\text{s}$, then clamp to stationary position to avoid runaway ghosts.

---

## 4. Testability Invariants
1. `mmo-client-wasm` compiles cleanly both on `x86_64-unknown-linux-gnu` and `wasm32-unknown-unknown`.
2. All unit tests (`tests for interpolation`, `packet parsing`, `telemetry serialization`) run directly via `cargo test -p mmo-client-wasm` in headless CI without requiring Node.js or browser drivers.
