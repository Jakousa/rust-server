# Tasks: Persistent World Objects, Dynamic Prefabs & Spatial Sector Streaming

## Phase 1: Core Primitives & Spatial Sector Indexing (`mmo-core`)
- [ ] 1.1 In `crates/core/src/world.rs`, implement `SectorCoord3D`, `Transform3D`, and `WorldObject`. *(Eligible for local subagent delegation)*
- [ ] 1.2 In `crates/core/src/world.rs`, implement spatial index mapping `SectorCoord3D` to active `WorldObject` instances with fast AABB spatial queries.
- [ ] 1.3 Export world primitives in `crates/core/src/lib.rs`.
- [ ] 1.4 Write unit tests in `crates/core/src/world.rs` verifying sector conversion, transform operations, and AABB intersection lookups. *(Eligible for local subagent delegation)*

## Phase 2: Schema Packets & Binary Wire Codecs (`mmo-schema`)
- [ ] 2.1 In `crates/schema/src/client.rs`, define `WorldObjectSpawnPacket`, `WorldObjectDeltaPacket`, `WorldObjectDespawnPacket`, and `WorldObjectResyncRequest`. *(Eligible for local subagent delegation)*
- [ ] 2.2 In `crates/schema/src/client.rs`, implement encode and decode codecs for world object packets with buffer bounds checking and error propagation. *(Eligible for local subagent delegation)*
- [ ] 2.3 Add opcode mappings and packet decoding arms to `ClientPacket`.
- [ ] 2.4 Write unit tests in `crates/schema` testing packet serialization roundtrips and truncated buffer error handling. *(Eligible for local subagent delegation)*

## Phase 3: Dual-Engine Relational Schema & Write-Behind Persistence (`mmo-storage`)
- [ ] 3.1 Create migration `crates/storage/migrations/V4__world_objects.sql` supporting PostgreSQL and SQLite. *(Eligible for local subagent delegation)*
- [ ] 3.2 In `crates/storage/src/models.rs`, define `WorldObjectRecord`. *(Eligible for local subagent delegation)*
- [ ] 3.3 In `crates/storage/src/world_repo.rs`, implement `WorldObjectRepository` with batch sector queries and multi-row upserts. *(Eligible for local subagent delegation)*
- [ ] 3.4 In `crates/node/src/world_snapshotter.rs`, implement `WorldObjectWriteBehindSnapshotter` with dirty coalescing map, `std::mem::take` buffer swapping to guarantee nanosecond lock holding, and background flusher.
- [ ] 3.5 Write integration tests in `crates/storage` validating migration execution, object persistence, and coalescing behavior across SQLite and PostgreSQL.

## Phase 4A: Authoritative Placement, Interaction & Anti-Cheat (`mmo-node`)
- [ ] 4A.1 In `crates/node/src/sector.rs`, implement `SectorManager` managing active sectors, object lookups, and partition boundary checks.
- [ ] 4A.2 Implement token-bucket rate limiter per client session (10 operations/s) in `crates/node/src/sector.rs`.
- [ ] 4A.3 In `crates/node/src/sim.rs`, handle `PlaceObjectRequest` with server-authoritative player position check ($D \le 6.0\text{m}$), line-of-sight raycast from eye position to target bounds, and bounding box non-penetration verification.
- [ ] 4A.4 In `crates/node/src/sim.rs`, handle `InteractObjectRequest` with line-of-sight raycasting and state transition validation.
- [ ] 4A.5 Emit domain events to registered `GameplayPlugin` hooks upon object placement and state mutation.

## Phase 4B: AoI Sector Discovery & Paced Streaming (`mmo-node`)
- [ ] 4B.1 In `crates/node/src/aoi.rs`, extend `Observer` to track visible `SectorCoord3D` coordinates intersecting observation spheres.
- [ ] 4B.2 Implement paced world object delivery in `aoi.rs` (max 10 objects per tick per client, prioritized by distance) and queue pruning of pending spawn packets when sectors exit AoI.
- [ ] 4B.3 Implement broadcast of `WorldObjectDeltaPacket` over reliable stream channels with monotonic `dirty_revision` numbers.
- [ ] 4B.4 Implement `WorldObjectResyncRequest` handler to re-transmit full object state upon detected revision gaps.
- [ ] 4B.5 Write integration tests in `crates/node` verifying sector entry/exit discovery, paced delivery, queue pruning, and delta replication.

## Phase 5: Client WebAssembly Core & Visual Rendering (`mmo-client-wasm`, `web/`)
- [ ] 5.1 In `crates/client-wasm/src/world.rs`, implement WebAssembly decoding of world object spawn and delta packets into flat render arrays. *(Eligible for local subagent delegation)*
- [ ] 5.2 In `web/src/world.js` (or inspector component), bind WASM world object buffers to Three.js instances for dynamic 3D rendering in the Web Inspector.
- [ ] 5.3 Write tests in `crates/client-wasm` validating packet decoding and buffer alignment. *(Eligible for local subagent delegation)*

## Phase 6: End-to-End Verification & Quality Gates
- [ ] 6.1 Execute full test suite `cargo test --workspace`.
- [ ] 6.2 Execute linter checks `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] 6.3 Execute code formatting checks `cargo fmt --all -- --check`.
- [ ] 6.4 Verify dynamic object placement, interaction, and multi-observer replication in the local cluster environment.
