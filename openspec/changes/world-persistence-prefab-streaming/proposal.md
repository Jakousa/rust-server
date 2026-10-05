# Proposal: Persistent World Objects, Dynamic Prefabs & Spatial Sector Streaming

## Motivation & Background
The server platform provides authoritative 20 Hz entity simulation, dynamic 3D Octree spatial domain balancing, and high-performance delta compression for mobile player and NPC entities. However, world environments currently lack durable, interactive world state.

In accordance with the project's **genre-agnostic virtual world philosophy**, the engine substrate must support dynamic world persistence without locking the platform into specific gameplay tropes or niche mechanics (such as destructive volumetric voxel blocks):
* **MMORPGs** need interactive chests, resource nodes, levers, gates, and guild monuments.
* **Survival & Sandbox Worlds** need player-constructed fortifications, barricades, campfires, and workbenches placed with 3D transforms.
* **Sci-Fi & Space Sims** need deployable sensor buoys, orbital defenses, docking bays, and spatial cargo containers.

To serve all genres with zero code pollution, the server platform requires generalized **Persistent World Objects, Dynamic Prefabs & Spatial Sector Streaming**.

---

## Architectural Objectives

1. **Generalized World Object & Prefab Abstraction (`mmo-core`)**:
   - Model all stationary and constructible world state as `WorldObject`:
     - 64-bit generational `object_id: Guid`.
     - `prefab_id: u32`: Decoupled visual/archetype identifier.
     - `transform: Transform3D`: World position $(x, y, z)$, orientation quaternion, and scale.
     - `bounds: BoundingBox3D`: Axis-aligned bounding box for kinematic collision, targeting, and raycasting.
     - `owner: ContainerOwner`: Ownership credentials (`Character`, `Guild`, `WorldEntity`, `System`).
     - `state_flags: u32`: Generalized state bitmask (e.g., Active, Open, Locked, Damaged, Destroyed).
     - `properties: HashMap<String, String>`: Flexible key-value metadata for arbitrary gameplay plugins.

2. **Spatial Sector Indexing & AoI Paging (`mmo-core`, `mmo-node`)**:
   - Partition continuous 3D space into uniform cubic sectors (e.g. $64\text{m} \times 64\text{m} \times 64\text{m}$) indexed by `SectorCoord3D`.
   - Extend `Observer` in `crates/node/src/aoi.rs` to track sector visibility within player observation spheres, streaming entering world objects with distance-prioritized bandwidth pacing.

3. **Authoritative Placement & Interaction Anti-Cheat (`mmo-node`)**:
   - Validate placement requests authoritatively on the simulation node holding the spatial partition.
   - Enforce server-authoritative player position bounds ($D \le 6.0\text{m}$), line-of-sight raycasting, non-overlap bounding box checks, and token-bucket rate limiting (10 ops/sec).

4. **Dual-Engine Write-Behind Persistence (`mmo-storage`)**:
   - Introduce migration `V4__world_objects.sql` supporting PostgreSQL and SQLite.
   - Implement `WorldObjectWriteBehindSnapshotter` with dirty coalescing queues and asynchronous multi-row batch flushes, isolating heavy serialization from the 20 Hz tick.

5. **Decoupled Plugin Extension & Genre-Agnostic Guarantee**:
   - Specialized mechanics (e.g. volumetric digging, specific building tiers, or auction boards) plug into world objects via [`GameplayPlugin`](crates/core/src/plugin.rs) hooks and events, maintaining clean separation from engine core.

6. **Local Subagent Delegation**:
   - Repetitive data definitions, SQL repositories, binary packet codecs, and test fixtures are structured for clean delegation to local subagents where available.

---

## Non-Goals
- Hardcoding specific RPG crafting trees, building recipes, or fantasy materials into engine core crates.
- Baking volumetric voxel grids into core spatial math (voxel mechanics can be implemented as an optional plugin).
- Client-authoritative structure placement without server verification.
