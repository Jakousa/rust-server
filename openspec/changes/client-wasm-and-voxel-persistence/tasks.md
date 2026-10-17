# Implementation Tasks: Client-WASM 64-Bit IDs, Prediction Stability & Voxel Persistence (Issue #104)

- [ ] 1. Fix 64-Bit GUID & Object ID Truncation in WASM Render Buffers (`crates/client-wasm` & `web/`)
  - [ ] 1.1 Update `WasmClientWorld::get_render_buffer` (`crates/client-wasm/src/lib.rs`) to encode 64-bit `entity_id` as split `(guid_lo, guid_hi)` 32-bit halves (9-float stride) and expose `get_entity_guids(&self) -> Vec<u64>`.
  - [ ] 1.2 Update `WorldObjectStore::get_render_buffer` (`crates/client-wasm/src/world.rs`) to store `(object_id & 0xFFFF_FFFF)` at index 0 and `(object_id >> 32)` at index 15 (preserving the 16-float stride).
  - [ ] 1.3 Update `web/src/renderer.js`, `web/src/world.js`, and `web/src/main.js` to read the lossless 64-bit IDs, and add unit tests in `crates/client-wasm/src/{lib.rs, world.rs}` verifying distinct IDs for `Guid::new(EntityType::Player, 1, 1)` vs `Guid::new(EntityType::Player, 1, 2)`.

- [ ] 2. Fix Malformed Frame Stall, Keyframe Prediction Wipe, and Duplicate Getter (`crates/client-wasm/src/lib.rs`)
  - [ ] 2.1 Update `WasmClientWorld::process_packet` to clear `self.incoming_buffer` and return `Err(err.to_string())` when `ClientPacket::decode_frame` returns `Err(err)`.
  - [ ] 2.2 Update `WasmClientWorld::handle_parsed_packet` so `EntityDeltaBatch` updates for `local_player_id` do not wipe `unacknowledged_inputs` via `reconcile(0, ...)` when predicted inputs are in flight (unless exceeding `hard_snap_threshold`).
  - [ ] 2.3 Remove duplicate `pending_acks_count` method and standardize on `pending_acks_len`.
  - [ ] 2.4 Add unit tests in `crates/client-wasm/src/lib.rs` for malformed frame recovery and keyframe arrival during active prediction.

- [ ] 3. Deduplicate Voxel RLE Codec & Mesh Generator (`crates/plugins/voxel/src/{codec.rs, mesh.rs}`)
  - [ ] 3.1 Unify `encode_rle` and `encode_rle_raw` in `crates/plugins/voxel/src/codec.rs` using a shared closure-based helper `encode_rle_with`.
  - [ ] 3.2 Extract `pub fn generate_mesh_from_raw(voxels: &[u16; CHUNK_VOXELS], neighbors: &NeighborBoundarySlices) -> VoxelMesh` in `crates/plugins/voxel/src/mesh.rs`.

- [ ] 4. Wire `VoxelTickSystem` Persistence, `VoxelChangeEvent` Propagation, Rate-Limiter Pruning & Lock Poison Recovery (`crates/plugins/voxel`)
  - [ ] 4.1 Buffer `VoxelChangeEvent`s in `VoxelWorld` on modification and add `drain_events(&self)` and `prune_rate_limiter(&self, entities: &EntityStore)`.
  - [ ] 4.2 Wire `VoxelWriteBehindService` into `VoxelTerrainPlugin` and `VoxelTickSystem::update` (`crates/plugins/voxel/src/plugin.rs`) so dirty chunks are enqueued and marked clean, `VoxelChangeEvent`s are pushed to `ctx.events`, and despawned entities are pruned from `ExcavationRateLimiter`.
  - [ ] 4.3 Replace all `.unwrap()` calls on `RwLock`/`Mutex` guards in `crates/plugins/voxel/src/{plugin.rs, world.rs}` with `.unwrap_or_else(|e| e.into_inner())`.
  - [ ] 4.4 Add integration tests in `crates/plugins/voxel/tests/voxel_plugin_test.rs` verifying tick-driven `VoxelWriteBehindService` enqueueing, `VoxelChangeEvent` emission in `EventQueue`, and rate-limiter despawn pruning.

- [ ] 5. Workspace Quality Gates
  - [ ] 5.1 Run `cargo fmt --all -- --check`
  - [ ] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 5.3 Run `cargo test --workspace`
