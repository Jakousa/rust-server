# Tasks: Volumetric Voxel & Deformable Terrain Plugin

## Stage 1: Plugin Crate Creation & Spatial Coordinates (`crates/plugins/voxel`)
- [x] 1. Create crate `crates/plugins/voxel` (package name `mmo-plugin-voxel`) and register in root `Cargo.toml`.
  - Dependencies: `mmo-core`, `serde`, `thiserror`, `bytes`, `tracing`, `dashmap`, `tokio`.
  - Verification: `cargo check -p mmo-plugin-voxel`
- [x] 2. Implement voxel spatial coordinates in `crates/plugins/voxel/src/coords.rs`:
  - `VoxelCoord { x: i64, y: i64, z: i64 }`
  - `VoxelChunkCoord { x: i32, y: i32, z: i32 }`
  - Euclidean division & modulo math (`div_euclid(16)` and `rem_euclid(16)`) ensuring positive chunk indices across negative world coordinates.
  - Conversions between world positions, `VoxelCoord`, `VoxelChunkCoord`, and `mmo_core::world::SectorCoord3D`.
  - Verification: Unit tests in `coords.rs` including negative world coordinates.

## Stage 2: Bit-Packed Palette Chunk Storage & RLE Codecs
- [x] 3. Implement bit-packed palette chunk storage in `crates/plugins/voxel/src/chunk.rs`:
  - `PaletteChunk` supporting dynamic bit-depths (0, 1, 2, 4, 8, 16 bits per voxel).
  - Fast bit operations for 4,096 voxels (16×16×16).
  - Homogeneous chunk optimization (0 bits allocated when single palette entry).
  - Dynamic palette expansion and array repacking.
  - Verification: Unit tests for get/set, palette expansion, and homogeneity in `chunk.rs`.
- [x] 4. Implement Run-Length Encoding (RLE) codecs in `crates/plugins/voxel/src/codec.rs`:
  - Compact serialization into `[count: u16, voxel_id: u16]` stream.
  - Strict decompression bomb defense: verify sum of RLE `count` values equals exactly `4,096`. Reject underflow/overflow.
  - Roundtrip decode verification, malformed payload boundary handling.
  - Verification: Unit tests and proptests in `codec.rs`.

## Stage 3: Boundary Mesh Generation & Deadlock Freedom
- [x] 5. Implement boundary face culling and mesh extraction in `crates/plugins/voxel/src/mesh.rs`:
  - Culled face meshing for outward-facing quad boundaries bordering air or transparent voxels.
  - Flat vertex layout `[x, y, z, nx, ny, nz, voxel_id]`.
  - Deadlock-free neighbor sampling: snapshot isolated boundary slices; never hold simultaneous locks on multiple chunks.
  - Unloaded neighbor policy: treat unloaded adjacent chunks as solid to prevent phantom external geometry.
  - Verification: Unit tests in `mesh.rs` verifying face culling on isolated and clustered blocks.

## Stage 4: Authoritative Digging, Mining & Deformation
- [x] 6. Implement authoritative modification handler in `crates/plugins/voxel/src/modify.rs`:
  - Reach validation against player position (`max_reach` threshold + tolerance buffer).
  - Rate limiting per entity (`ExcavationRateLimiter`).
  - Chunk modification, dirty flagging, and event dispatch (`VoxelChangeEvent`).
  - Verification: Unit tests in `modify.rs` testing reach limits, rate limiting, and chunk mutation.

## Stage 5: Modular Packet Routing & Wire Protocol
- [x] 7. Implement wire protocol and packet router in `crates/plugins/voxel/src/net.rs`:
  - Module ID `0x00A0` registration on `ModularPacketRouter`.
  - Sub-opcodes for `VoxelChunkRequest` (0x01), `VoxelChunkData` (0x02), `VoxelDeltaBatch` (0x03), `VoxelModifyRequest` (0x04), `VoxelModifyResponse` (0x05).
  - Safe bounds-checked binary decoding.
  - Verification: Unit tests in `net.rs` for packet encoding/decoding.

## Stage 6: GameplayPlugin & VoxelWorld Subsystem
- [x] 8. Implement `VoxelWorld` in `crates/plugins/voxel/src/world.rs`:
  - Concurrent chunk registry (`DashMap<VoxelChunkCoord, Arc<RwLock<PaletteChunk>>>`).
  - Chunk loading, unloading, modification, boundary neighbor invalidation, and dirty tracking.
  - Verification: Unit tests in `world.rs`.
- [x] 9. Implement `VoxelTerrainPlugin` in `crates/plugins/voxel/src/plugin.rs`:
  - Implements `mmo_core::plugin::GameplayPlugin`.
  - Registers packet handler, tick system for dirty chunk processing, and event listeners.
  - Export public API in `crates/plugins/voxel/src/lib.rs`.
  - Verification: Integration with `mmo_core::plugin::PluginManager`.

## Stage 7: Asynchronous Write-Behind Chunk Persistence
- [x] 10. Implement write-behind chunk persistence in `crates/plugins/voxel/src/persistence.rs`:
  - Bounded in-memory coalescing map (`DashMap<VoxelChunkCoord, SerializedChunkSnapshot>`).
  - Background async channel / batch flusher to prevent OOM under DB latency spikes.
  - SQLite/PostgreSQL table schema and batch upsert logic.
  - Flush and graceful shutdown coordination.
  - Verification: Unit tests with in-memory SQLite storage.

## Stage 8: Client WebAssembly Decoder (`crates/client-wasm`)
- [x] 11. Implement client-side WASM voxel chunk decoder in `crates/client-wasm/src/voxel.rs`:
  - Fast client-side RLE decompression and WebGL-ready flat float vertex buffer generation.
  - Expose WASM-bound method `decode_voxel_chunk_mesh(data: &[u8]) -> Vec<f32>`.
  - Verification: Unit tests in `client-wasm`.

## Stage 9: Comprehensive Integration & Stress Tests
- [x] 12. Implement end-to-end integration tests in `crates/plugins/voxel/tests/voxel_plugin_test.rs`:
  - Authoritative mining cycle (player reach, cooldown, chunk mutation, delta broadcast).
  - Multi-chunk deformation and boundary mesh recalculation.
  - High-throughput excavation stress test.
  - Verification: `cargo test -p mmo-plugin-voxel`

## Stage 10: Quality Gates & PR
- [x] 13. Run full workspace quality gates:
  - Formatting: `cargo fmt --all -- --check`
  - Strict Clippy: `cargo clippy --workspace --all-targets -- -D warnings`
  - Workspace tests: `cargo test --workspace`
- [x] 14. Git commit, push branch, create PR with auto-merge enabled.
