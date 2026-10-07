# Specification: Volumetric Voxel & Deformable Terrain Plugin

## Domain: `plugins::voxel`

### Feature: Bit-Packed Palette Chunk Storage
The system must store volumetric 3D voxel chunks (16×16×16 = 4,096 voxels) in bit-packed, palette-compressed representations to minimize memory usage.

#### Scenario: Homogeneous Chunk Compression
- **GIVEN** a newly created chunk filled entirely with a single voxel type (e.g. Air `0` or Solid Stone `1`)
- **WHEN** the chunk is initialized or compressed
- **THEN** the palette SHALL contain exactly 1 entry
- **AND** the bit-depth per voxel SHALL be 0 bits, consuming zero bit-packing array allocations.

#### Scenario: Dynamic Bit-Depth Resizing
- **GIVEN** a palette chunk containing 2 unique voxel types (1 bit per voxel)
- **WHEN** a third unique voxel type is set in the chunk
- **THEN** the palette size SHALL increase to 3
- **AND** the storage bit-depth SHALL expand to 2 bits per voxel, repacking all 4,096 voxels accurately without data corruption.

#### Scenario: Run-Length Encoding (RLE) Serialization & Decompression Bomb Defense
- **GIVEN** an active palette chunk with multiple voxel clusters or an untrusted network payload
- **WHEN** serialized or deserialized
- **THEN** the chunk SHALL be encoded into an RLE stream `[(count: u16, voxel_id: u16)]`
- **AND** the decoder SHALL enforce that the sum of all `count` values equals exactly 4,096 voxels
- **AND** any stream underflowing or exceeding 4,096 voxels SHALL be rejected with a `CodecError::MalformedRlePayload`.

---

### Feature: Authoritative Digging, Mining & Deformation
The server must validate all voxel manipulation requests authoritatively to prevent speed-mining, out-of-reach edits, and invalid block placement.

#### Scenario: Negative Coordinate Mathematical Safety
- **GIVEN** world coordinates residing in negative space (e.g. `x = -17`, `y = -5`, `z = -65`)
- **WHEN** converting to chunk coordinate and local index
- **THEN** the calculation SHALL use Euclidean division (`div_euclid(16)` and `rem_euclid(16)`)
- **AND** the local coordinates SHALL strictly lie in `[0, 15]` preventing out-of-bounds panics.

#### Scenario: Out-of-Reach Modification Rejection
- **GIVEN** a player entity at position `(0.0, 0.0, 0.0)` with a max reach of 6.0 meters (plus 0.5m network tolerance buffer)
- **WHEN** the player sends a voxel modification packet targeting world coordinate `(10.0, 0.0, 0.0)`
- **THEN** the server SHALL reject the modification request
- **AND** emit a warning/error response without modifying the chunk.

#### Scenario: Rate-Limited Excavation
- **GIVEN** a player entity excavation rate limit of 100 milliseconds
- **WHEN** the player attempts two excavation actions within 20 milliseconds
- **THEN** the second action SHALL be rate-limited and rejected.

#### Scenario: Authoritative Voxel Modification & Event Emission
- **GIVEN** a valid player within reach and within rate limits targeting block `(x, y, z)`
- **WHEN** the excavation request is processed
- **THEN** the target voxel SHALL be updated to the new voxel type (e.g. Air)
- **AND** the containing chunk's dirty flag SHALL be set
- **AND** a `VoxelChangeEvent` SHALL be emitted to the event queue.

---

### Feature: Boundary Mesh Generation
The plugin must generate clean boundary meshes (culled faces / greedy meshing) for modified chunks to support collision detection and client rendering without lock-inversion deadlocks.

#### Scenario: Internal Face Culling & Unloaded Neighbor Policy
- **GIVEN** a 2×2×2 block of solid voxels surrounded by air
- **WHEN** the mesh generator processes the chunk
- **THEN** all internal shared faces between solid blocks SHALL be culled
- **AND** unloaded adjacent neighbor chunks SHALL be treated as solid (culling faces) to prevent phantom edge geometry
- **AND** when an adjacent chunk is subsequently loaded, boundary neighbors SHALL be invalidated and remeshed.

#### Scenario: Dirty Chunk Boundary Recalculation
- **GIVEN** a voxel modification at the edge of a chunk (e.g. `x = 0` or `x = 15`)
- **WHEN** dirty chunks are processed at the tick boundary
- **THEN** both the modified chunk and the adjacent neighbor chunk sharing the boundary SHALL be flagged for mesh recalculation.

---

### Feature: Modular Network Packet Streaming
Voxel replication must use the decoupled `ModularPacketRouter` framework with designated module ID `0x00A0`.

#### Scenario: Full Chunk Delivery
- **GIVEN** an observing client entering a new sector
- **WHEN** the client sends a `VoxelChunkRequest` (sub-opcode 1)
- **THEN** the server SHALL respond with a `VoxelChunkData` packet (sub-opcode 2) containing the compressed palette chunk.

#### Scenario: Incremental Voxel Delta Broadcast
- **GIVEN** an authoritative modification modifying 3 voxels in a chunk
- **WHEN** tick deltas are collected
- **THEN** an incremental `VoxelDeltaPacket` (sub-opcode 3) containing the exact coordinate offsets and new voxel IDs SHALL be broadcast to nearby observers.

---

### Feature: Asynchronous Write-Behind Persistence
Modified voxel chunks must persist to database storage asynchronously without blocking the 20 Hz simulation loop.

#### Scenario: Dirty Chunk Coalescing
- **GIVEN** a chunk that undergoes 10 voxel modifications within a 1-second window
- **WHEN** enqueued into the write-behind storage queue
- **THEN** multiple writes SHALL coalesce into a single write-behind job
- **AND** the asynchronous persistence worker SHALL persist the latest serialized blob to the database table.

---

### Feature: Client-Side WebAssembly Decoding
The client WASM crate (`crates/client-wasm`) must be able to decode compressed voxel chunks and produce flat WebGL-compatible vertex arrays.

#### Scenario: Client WASM Mesh Buffer Extraction
- **GIVEN** a serialized chunk binary buffer received in WebAssembly
- **WHEN** decoded via `decode_voxel_chunk_mesh`
- **THEN** it SHALL output flat `Float32Array` compatible vertex data `[x, y, z, nx, ny, nz, voxel_id]` ready for GPU upload.
