# Implementation Tasks: Aggressive AoI Culling & Bit-Packed Deltas

- [x] 1. Macro-Driven Bit-Packed Delta Serialization (`crates/schema`)
  - [x] 1.1 Implement declarative `bitpack_delta!` macro in `crates/schema/src/bitpack.rs` supporting arbitrary bit widths, sub-byte packing, varints, and field masks.
  - [x] 1.2 Add unit tests for macro-generated codecs validating sub-byte boundaries, bit alignment, and zero byte padding.
  - [x] 1.3 Patch `is_teleport_delta` in `crates/schema/src/quantize.rs` to detect non-finite / `NaN` coordinates and force fallback.
  - [x] 1.4 Verify `cargo test -p mmo-schema --lib bitpack --lib quantize`.

- [x] 2. Unopinionated Generic Scalar Attributes (`crates/schema`)
  - [x] 2.1 Define `ScalarAttributeDelta` struct with `attribute_id: u16` and `value: u32`.
  - [x] 2.2 Add `delta_flags::MASK_ATTRIBUTES = 1 << 11` to `crates/schema/src/client.rs`.
  - [x] 2.3 Extend `EntityDeltaState` with `attributes: Vec<ScalarAttributeDelta>` and implement binary encode/decode with varints.
  - [x] 2.4 Add proptest and roundtrip serialization unit tests in `crates/schema/src/client.rs`.
  - [x] 2.5 Verify `cargo test -p mmo-schema`.

- [x] 3. View Frustum Culling & Robust Differencing (`crates/node`)
  - [x] 3.1 Extend `ObserverConfig` with `enable_frustum_culling: bool`, `fov_degrees: f32`, and `near_clip_radius: f32`.
  - [x] 3.2 Implement pure geometric `is_in_view_frustum(center, yaw, target_pos, fov_deg, near_clip_radius)` in `crates/node/src/aoi.rs` using 3D spherical awareness bubble and forward dot products.
  - [x] 3.3 Fix yaw wrap-around math in `aoi.rs` to evaluate shortest circular distance across the $[0, 2\pi)$ boundary.
  - [x] 3.4 Rate-limit full keyframe refresh broadcasts under expired baseline states to prevent network death spirals.
  - [x] 3.5 Decouple `aoi.rs` from direct `mmo_gameplay::combat::HealthComponent` hardcoded imports, migrating entity attribute replication to generic scalar attributes.
  - [x] 3.6 Integrate frustum filtering into `AoiManager::update_observers` using in-place scratch buffer retain.
  - [x] 3.7 Add comprehensive unit tests covering frustum culling, 3D awareness sphere, yaw boundary crossing, and keyframe throttling.
  - [x] 3.8 Verify `cargo test -p mmo-node --lib aoi`.

- [x] 4. Baseline Acknowledgement & Client WASM Auto-ACK (`crates/client-wasm`)
  - [x] 4.1 Update `ClientEntityReplica` to decode and store dynamic scalar attributes.
  - [x] 4.2 Add `pending_acks: VecDeque<u64>` and `drain_pending_acks(&mut self) -> Vec<Vec<u8>>` to `WasmClientWorld`.
  - [x] 4.3 Automatically queue client acks upon receiving `EntityDeltaBatchPacket` in `handle_parsed_packet`.
  - [x] 4.4 Add unit tests for client-side delta application and auto-ACK generation in `crates/client-wasm/src/lib.rs`.
  - [x] 4.5 Verify `cargo test -p mmo-client-wasm`.

- [x] 5. Bandwidth Compression & Density Scaling Verification
  - [x] 5.1 Implement comprehensive test / benchmark verifying bandwidth scaling under dense player clusters.
  - [x] 5.2 Validate $> 80\%$ payload size reduction comparing bit-packed deltas to raw uncompressed struct broadcasts.
  - [x] 5.3 Verify that network egress scales logarithmically / linearly throttled, not quadratically, with local density.
  - [x] 5.4 Verify `cargo test --workspace`.

- [x] 6. Quality Gates & OpenSpec Sync
  - [x] 6.1 Run `cargo fmt --all -- --check`.
  - [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 6.3 Run full workspace test suite `cargo test --workspace`.
  - [x] 6.4 Sync delta specs to `openspec/specs/network/aoi-culling-bitpacked-deltas/spec.md`.
