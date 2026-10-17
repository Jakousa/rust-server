# Client-WASM 64-Bit ID Fidelity, Prediction Reconciliation, Voxel Persistence Wiring & Codec Deduplication Proposal

## Context & Problem Statement
Auditing `crates/client-wasm` and `crates/plugins/voxel` (Issue #104) identified four critical client-side and voxel plugin issues:

1. **Precision Loss Casting 64-Bit GUIDs and Revisions to `f32` (`crates/client-wasm/src/lib.rs:357` & `world.rs:151, 165`)**:
   - `WasmClientWorld::get_render_buffer` casts `replica.entity_id as f32` into a `Float32Array`. Because `Guid` packs `EntityType` in bits 48..63 and `region_id` in bits 32..47, every valid entity GUID exceeds $2^{24}$ (the 24-bit mantissa limit of `f32`), causing all entities of the same type/region to lose their lower 40 sequence bits and collide to the exact same `f32` ID in WebGL!
   - Similarly, `WorldObjectStore::get_render_buffer` casts 64-bit `obj.object_id as f32` and `obj.revision as f32`.

2. **Permanent Buffer Stall on Malformed Frame & Keyframe Prediction Rubberbanding (`crates/client-wasm/src/lib.rs:110, 149-156`)**:
   - `process_packet` loops `while let Ok(Some(packet)) = ClientPacket::decode_frame(&mut self.incoming_buffer)`, silently ignoring `Err(ClientCodecError)` and leaving corrupt frame bytes at the head of `incoming_buffer` forever.
   - `handle_parsed_packet` calls `self.prediction_buffer.reconcile(0, server_pos)` whenever a keyframe delta arrives for `local_player_id`. Since client input sequences start at `1`, `ack_sequence = 0` never matches `unacknowledged_inputs`, falling into the unsequenced fallback that clears `unacknowledged_inputs` whenever the player has moved $> 0.05\text{m}$ during RTT, causing severe movement rubberbanding.
   - `pending_acks_len` (`line 323`) and `pending_acks_count` (`line 456`) duplicate the exact same getter across two `impl WasmClientWorld` blocks.

3. **Duplicated Voxel RLE Codec & Culled Mesh Generator (`crates/client-wasm/src/voxel.rs` vs `crates/plugins/voxel/src/{codec.rs, mesh.rs}`)**:
   - `crates/plugins/voxel/src/codec.rs` duplicates the RLE span-encoding loop across `encode_rle` (`lines 31-59`) and `encode_rle_raw` (`lines 70-97`).
   - `crates/client-wasm/src/voxel.rs` duplicates `decode_rle_voxels` and `generate_culled_mesh`.

4. **Swallowed `VoxelChangeEvent`, No-Op `VoxelTickSystem`, Unpruned `ExcavationRateLimiter`, and Lock `.unwrap()`s (`crates/plugins/voxel`)**:
   - `VoxelPacketHandler::handle` (`plugin.rs:82`) discards `_event` (`VoxelChangeEvent`), and `VoxelTickSystem::update` (`plugin.rs:132`) calls `let _dirty_coords = self.world.drain_dirty_chunks();` and drops them without enqueuing to `VoxelWriteBehindService` or emitting `VoxelChangeEvent`s.
   - `ExcavationRateLimiter::last_excavation` (`modify.rs:63`) is never pruned when entities despawn, and `plugin.rs:51` / `world.rs:81, 120, 178-219` call `.unwrap()` on `RwLock`/`Mutex` guards.

## Proposed Solution

1. **Lossless 64-Bit ID Encoding in WASM Render Buffers**:
   - Encode 64-bit `entity_id` and `object_id` as lossless `(lo_u32, hi_u32)` pairs (or bit-preserved `f32::from_bits(lo_u32)` / `f32::from_bits(hi_u32)` or split low/high integer words) alongside a companion `get_entity_ids() -> Vec<u64>` / `lo, hi` representation, and update `web/src/renderer.js`, `web/src/world.js`, and `web/src/main.js` so two entities with identical `(EntityType, region_id)` and sequential counters (`1` vs `2`) have distinct IDs.

2. **Explicit Decode Error Handling & Non-Destructive Keyframe Prediction**:
   - In `WasmClientWorld::process_packet`, match explicitly on `ClientPacket::decode_frame(&mut self.incoming_buffer)`: on `Err(e)`, clear `self.incoming_buffer` and return `Err(e.to_string())`.
   - In `handle_parsed_packet` for `Some(entity_id) == self.local_player_id`, do not call `reconcile(0, server_pos)` when `unacknowledged_inputs` is non-empty (unless `distance(current_sim_pos, server_pos) > hard_snap_threshold`); keep `replica.position` aligned with `self.prediction_buffer.visual_position(self.prediction_buffer.current_sim_pos)` and rely on `MovementCorrection` for sequence-acknowledged reconciliation.
   - Remove the duplicate `pending_acks_count` method in favor of `pending_acks_len`.

3. **Deduplicate RLE Span Encoding & Mesh Generation**:
   - Unify `encode_rle` and `encode_rle_raw` in `crates/plugins/voxel/src/codec.rs` via a shared `encode_rle_from_fn(revision, |i| ...)` helper, and extract `generate_mesh_from_raw` in `crates/plugins/voxel/src/mesh.rs`.

4. **Wire `VoxelTickSystem` Persistence, `VoxelChangeEvent` Emission, Rate-Limiter Pruning, and Poison Recovery**:
   - Buffer `VoxelChangeEvent`s in `VoxelWorld` on successful modification; in `VoxelTickSystem::update`, drain buffered events into `ctx.events`, drain dirty chunks into `VoxelWriteBehindService::enqueue` (marking chunks clean), and prune despawned entities from `ExcavationRateLimiter`.
   - Replace all `.unwrap()` calls on `RwLock`/`Mutex` guards in `crates/plugins/voxel/src/{plugin.rs, world.rs}` with `.unwrap_or_else(|e| e.into_inner())`.

## Scope & Non-Goals
- **In Scope**: `crates/client-wasm/src/{lib.rs, world.rs, voxel.rs}`, `crates/plugins/voxel/src/{codec.rs, mesh.rs, modify.rs, world.rs, plugin.rs}`, and `web/src/{main.js, renderer.js, world.js}`.
- **Non-Goals**: Adding voxel mechanics to core crates (`core`, `node`, `schema`).
