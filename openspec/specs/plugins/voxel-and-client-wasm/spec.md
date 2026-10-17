# Client-WASM Render Fidelity, Prediction Stability, and Voxel Persistence Specification

## Purpose
Eliminates 64-bit GUID precision loss in WebAssembly render buffers, prevents corrupt frames from stalling client packet processing, preserves client-side movement prediction across unsequenced server keyframes, and wires dirty voxel chunk persistence and event emission.

## Requirements

### Requirement: Lossless 64-Bit Entity and World Object Identification in Client-WASM
`WasmClientWorld::get_render_buffer` and `WorldObjectStore::get_render_buffer` SHALL encode 64-bit entity GUIDs and world object IDs without `f32` mantissa truncation so that entities sharing the same high 32 bits (`EntityType` and `region_id`) remain uniquely identifiable by the renderer.

#### Scenario: Two player entities in the same region with sequential counters
- **GIVEN** two entities with GUIDs `Guid::new(EntityType::Player, 1, 1)` and `Guid::new(EntityType::Player, 1, 2)`
- **WHEN** `get_render_buffer` serializes both entities
- **THEN** the encoded identity fields for the two entities SHALL be distinct and reconstruct their exact 64-bit GUIDs

### Requirement: Corrupt Frame Recovery and Non-Disruptive Keyframe Prediction
`WasmClientWorld::process_packet` SHALL return an `Err` and clear `incoming_buffer` when `ClientPacket::decode_frame` returns a `ClientCodecError`. Receiving an `EntityDeltaBatch` keyframe for `local_player_id` while unacknowledged predicted inputs are pending SHALL NOT wipe `unacknowledged_inputs` unless the position divergence exceeds `hard_snap_threshold`.

#### Scenario: Malformed frame followed by valid frame
- **GIVEN** a `WasmClientWorld` instance
- **WHEN** `process_packet` is called with invalid magic bytes followed by a second call with a valid `EntityDeltaBatch` frame
- **THEN** the first call SHALL return `Err(...)` and clear the buffer, and the second call SHALL succeed and increment `received_packets`

#### Scenario: Keyframe delta arrives while local player has unacknowledged inputs
- **GIVEN** a local player with 3 unacknowledged predicted movement inputs moving within `hard_snap_threshold`
- **WHEN** an `EntityDeltaBatch` with `MASK_KEYFRAME` arrives for the local player
- **THEN** `prediction_buffer.pending_input_count()` SHALL remain `3` and the local player's predicted position SHALL NOT snap back

### Requirement: Voxel Dirty Chunk Write-Behind Enqueueing and Event Propagation
`VoxelTickSystem::update` SHALL drain `VoxelChangeEvent`s into `TickContext::events`, enqueue dirty chunks into `VoxelWriteBehindService` while marking them clean, and prune despawned entities from `ExcavationRateLimiter`.

#### Scenario: Voxel modification processed during simulation tick
- **GIVEN** a `VoxelWorld` where a player modifies a voxel via `VoxelPacketHandler` or `modify_voxel`
- **WHEN** `VoxelTickSystem::update` executes on the next simulation tick
- **THEN** a `VoxelChangeEvent` SHALL be pushed to `ctx.events` and the modified chunk SHALL be enqueued in `VoxelWriteBehindService` and marked clean
