# Technical Design: Client-WASM 64-Bit IDs, Prediction Stability & Voxel Persistence

## 1. Lossless 64-Bit IDs in WASM Render Buffers (`crates/client-wasm` & `web/`)

### 1.1 `WasmClientWorld::get_render_buffer` (`crates/client-wasm/src/lib.rs:349-375`)
- A 64-bit `u64` GUID has a lower 32-bit counter (`lo = (guid & 0xFFFF_FFFF) as u32`) and an upper 32-bit header (`hi = (guid >> 32) as u32` packing `EntityType` in bits 16..31 and `region_id` in bits 0..15).
- In `get_render_buffer`:
  - Expand per-entity stride from 8 to 9 floats (or store `guid_lo` and `guid_hi` as `f32::from_bits(lo)` / `f32::from_bits(hi)` or `lo as f32` + `hi as f32` where counter `< 2^24` and `hi` is split, or use a 9-float stride `[f32::from_bits(guid_lo), f32::from_bits(guid_hi), x, y, z, yaw, hp_pct, cast_pct, entity_type as f32]` plus helper `reconstruct_u64(lo_f32, hi_f32) -> u64`).
  - Wait: to keep JS and Rust tests ultra-ergonomic and avoid `NaN` bit patterns when `hi` happens to have all exponent bits `1` (which JS `Float32Array` canonicalizes if read as a float!), how can we encode a 64-bit `Guid` into `f32`s with **zero `NaN` hazard** and **100% lossless integer precision** in both Rust and JS?
    - In IEEE-754 `f32`, any integer in `0..=16_777_216` ($2^{24}$) is represented **100% exactly** as a normal float Value (`val as f32 as u32 == val`) with zero `NaN` bit-pattern risk!
    - For `Guid` (`(entity_type: u16) << 48 | (region_id: u16) << 32 | (counter: u32)`):
      - `entity_type` is already in slot 7 (`replica.entity_type as f32`)!
      - If slot 0 stores the lower 24 bits of `entity_id` (`(replica.entity_id & 0x00FF_FFFF) as f32`) AND we also provide `pub fn get_render_entity_ids(&self) -> Vec<u64>` (which `wasm-bindgen` exposes as `BigUint64Array` in JS!), OR we use a 9-float stride `[guid_lo_24, guid_mid_24, guid_hi_16, x, y, z, yaw, hp_pct, cast_pct, entity_type]` / 9-float `[lo_u32_f32, hi_u32_f32, ...]`:
      - Even simpler: in `get_render_buffer` (`crates/client-wasm/src/lib.rs:353-368`), why did `Guid::new(EntityType::Player, 1, 1) as f32` lose precision? Because `(1u64 << 48) + (1u64 << 32) + 1` has bits at position 48 and position 0 (a 49-bit span > 24-bit mantissa).
      - If we store `[guid_lo, guid_hi, x, y, z, yaw, hp_pct, cast_pct, entity_type]` (stride = 9) where `guid_lo = (replica.entity_id & 0xFFFF_FFFF) as f32` and `guid_hi = (replica.entity_id >> 32) as f32` (and also expose `pub fn get_entity_guids(&self) -> Vec<u64>`), then for any counter `< 16,777,216` and any `(entity_type, region_id)`, `(guid_lo, guid_hi)` is a finite normal float in both Rust and JS, AND `(BigInt(Math.round(guid_hi)) << 32n) | BigInt(Math.round(guid_lo))` (or `((guid_hi as u64) << 32) | (guid_lo as u64)`) reconstructs the 64-bit `Guid` cleanly!
  - And in `WorldObjectStore::get_render_buffer` (`crates/client-wasm/src/world.rs:148-169`):
    - Slot 0 is `object_id_lo = (obj.object_id & 0xFFFF_FFFF) as f32` and slot 15 (which was unused `0.0` padding at line 166!) is `object_id_hi = (obj.object_id >> 32) as f32`, keeping the exact 16-float stride!
  - Update `web/src/renderer.js`, `web/src/world.js`, and `web/src/main.js` to match.

---

## 2. Packet Decode Error Handling & Keyframe Prediction Fix (`crates/client-wasm/src/lib.rs`)

### 2.1 `WasmClientWorld::process_packet` (`lines 107-116`)
```rust
pub fn process_packet(&mut self, data: &[u8]) -> Result<(), String> {
    self.incoming_buffer.extend_from_slice(data);

    loop {
        match ClientPacket::decode_frame(&mut self.incoming_buffer) {
            Ok(Some(packet)) => {
                self.received_packets += 1;
                self.handle_parsed_packet(packet);
            }
            Ok(None) => break Ok(()),
            Err(err) => {
                self.incoming_buffer.clear();
                return Err(err.to_string());
            }
        }
    }
}
```

### 2.2 `WasmClientWorld::handle_parsed_packet` (`lines 140-165`)
- When `Some(entity_id) == self.local_player_id`:
  - Save `let server_pos = replica.position;` after `replica.apply_delta(&delta);`.
  - If `self.prediction_buffer.pending_input_count() == 0`:
    - If `delta.field_mask & delta_flags::MASK_KEYFRAME != 0` or `delta.field_mask & delta_flags::MASK_POS_X != 0`, synchronize `let reconciled = self.prediction_buffer.reconcile(0, server_pos); replica.position = self.prediction_buffer.visual_position(reconciled);`.
  - Else (`self.prediction_buffer.pending_input_count() > 0`):
    - Do **not** call `reconcile(0, server_pos)` unless the Euclidean distance between `self.prediction_buffer.current_sim_pos` and `server_pos` exceeds `self.prediction_buffer.hard_snap_threshold` ($20.0\text{m}$).
    - Keep `replica.position = self.prediction_buffer.visual_position(self.prediction_buffer.current_sim_pos);` so unsequenced delta broadcasts never overwrite in-flight client prediction!

### 2.3 Remove Duplicate `pending_acks_count` (`line 456`)
- Delete `pub fn pending_acks_count(&self) -> usize` at line 456 and update test call sites in `crates/client-wasm/src/lib.rs` to use `pending_acks_len()`.

---

## 3. Voxel Codec Deduplication, Event Emission, Persistence & Lock Poison Recovery (`crates/plugins/voxel`)

### 3.1 RLE Codec & Mesh Deduplication (`crates/plugins/voxel/src/{codec.rs, mesh.rs}`)
- In `crates/plugins/voxel/src/codec.rs`:
  - Extract `fn encode_rle_with<F: Fn(usize) -> u16>(revision: u64, get_voxel: F) -> Vec<u8>`.
  - Implement `encode_rle(chunk: &PaletteChunk)` as `encode_rle_with(chunk.revision, |i| chunk.get(i))` and `encode_rle_raw(voxels: &[u16; CHUNK_VOXELS], revision: u64)` as `encode_rle_with(revision, |i| voxels[i])`.
- In `crates/plugins/voxel/src/mesh.rs`:
  - Extract `pub fn generate_mesh_from_raw(voxels: &[u16; CHUNK_VOXELS], neighbors: &NeighborBoundarySlices) -> VoxelMesh` so `generate_chunk_mesh` simply calls `generate_mesh_from_raw(&chunk.unpack_all(), neighbors)`.

### 3.2 `VoxelWorld` Event Queue, `VoxelTickSystem` Persistence, and `ExcavationRateLimiter` Pruning
- In `crates/plugins/voxel/src/world.rs`:
  - Add `pending_events: Arc<Mutex<Vec<VoxelChangeEvent>>>` to `VoxelWorld`.
  - In `VoxelWorld::modify_voxel`, when `let Some( ref ev ) = event`, push `ev.clone()` into `self.pending_events`.
  - Add `pub fn drain_events(&self) -> Vec<VoxelChangeEvent>` and `pub fn prune_rate_limiter(&self, entities: &mmo_core::EntityStore)` (which retains only `guid`s in `rate_limiter.last_excavation` where `entities.get_by_guid(guid).is_some()`).
  - Replace all `.unwrap()` calls on `RwLock`/`Mutex` guards (`lines 81, 120, 178, 184, 190, 196, 202, 208, 219`) with `.unwrap_or_else(|e| e.into_inner())`.
- In `crates/plugins/voxel/src/plugin.rs`:
  - Replace `chunk_arc.read().unwrap()` at line 51 with `chunk_arc.read().unwrap_or_else(|e| e.into_inner())`.
  - Add `write_behind: VoxelWriteBehindService` to `VoxelTerrainPlugin` and `VoxelTickSystem` (with `pub fn with_write_behind(world: VoxelWorld, write_behind: VoxelWriteBehindService) -> Self` and `pub fn write_behind(&self) -> &VoxelWriteBehindService`).
  - In `VoxelTickSystem::update(&mut self, ctx: &mut TickContext)`:
    1. For each `ev` in `self.world.drain_events()`, call `ctx.events.push(ev);`.
    2. For each `coord` in `self.world.drain_dirty_chunks()`, if `let Some(chunk_arc) = self.world.get_chunk(coord)`:
       ```rust
       let mut chunk = chunk_arc.write().unwrap_or_else(|e| e.into_inner());
       if chunk.is_dirty() {
           self.write_behind.enqueue(coord, &chunk);
           chunk.mark_clean();
       }
       ```
    3. Call `self.world.prune_rate_limiter(ctx.entities);`.
