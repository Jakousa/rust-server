# Specification: Core Primitives & Voxel Engine Audit Hardening

## Capability: Core Event Lifecycle & Memory Hygiene

### Scenario: Ephemeral Per-Tick Events Cleared at Tick Boundaries
- **GIVEN** an `EventQueue` with registered ephemeral event types (such as `ActionProgressEvent` and `VoxelChangeEvent`)
- **WHEN** multiple ephemeral events are pushed into the event queue during a simulation tick
- **AND** `clear_ephemeral()` is invoked at the end-of-tick boundary
- **THEN** all queued ephemeral events SHALL be purged from memory
- **AND** non-ephemeral domain events (such as `DamageEvent` or `EntityIncapacitatedEvent`) SHALL remain intact until explicitly drained.

### Scenario: EventQueue Memory Retention Limits
- **GIVEN** an `EventQueue` receiving non-ephemeral events without a consumer draining them
- **WHEN** `retain_recent(N)` is called
- **THEN** the number of events retained per `TypeId` SHALL NOT exceed `N`
- **AND** the oldest events SHALL be dropped to prevent unbounded heap growth.

---

## Capability: Spatial Sector Query Completeness

### Scenario: Objects Spanning Sector Boundaries
- **GIVEN** a `SectorSpatialIndex` containing a `WorldObject` whose origin `transform.position` resides in sector `(0, 0, 0)`
- **AND** the object's `bounds` extend across the sector boundary into sector `(1, 0, 0)`
- **WHEN** `query_aabb` is executed for a bounding box strictly located inside sector `(1, 0, 0)` that intersects the object's bounds
- **THEN** `SectorSpatialIndex` SHALL evaluate sector `(0, 0, 0)` via `max_extent` expansion
- **AND** the object SHALL be returned in the query results.

### Scenario: Removal of Max-Extent Object
- **GIVEN** a `SectorSpatialIndex` with indexed objects of varying spatial extents
- **WHEN** the object with the largest spatial extent is removed via `remove(guid)`
- **THEN** `SectorSpatialIndex::max_extent()` SHALL dynamically update to the maximum extent of the remaining objects.

---

## Capability: Voxel Engine Concurrency & Robustness

### Scenario: Chunk-Level Concurrency in Voxel Modification
- **GIVEN** two concurrent threads attempting to modify voxels in different chunks
- **WHEN** `VoxelWorld::modify_voxel` is called by each thread
- **THEN** the global rate limiter Mutex SHALL be released prior to acquiring the per-chunk write lock
- **AND** neither thread SHALL be blocked by the other thread's palette repacking or write lock acquisition.

### Scenario: Safe Out-of-Bounds Palette Chunk Access
- **GIVEN** a `PaletteChunk` instance
- **WHEN** `get(idx)` or `set(idx, val)` is invoked with an index `idx >= CHUNK_VOXELS` (e.g. 4096)
- **THEN** the call SHALL NOT panic
- **AND** `get` SHALL return 0 (Air) while `set` SHALL return `false`.

---

## Capability: Numeric Safety & Hierarchy Cycle Protection

### Scenario: Non-Panicking Attribute Clamping
- **GIVEN** an `AttributeSet` or `AttributeValue`
- **WHEN** an attribute's effective max or current value is queried or modified with inverted bounds (`min > max`) or `NaN`
- **THEN** `safe_clamp` SHALL NOT panic and SHALL return a safe fallback value.

### Scenario: Hierarchy Transform Cycle Safety
- **GIVEN** an `EntityStore` containing entities whose parent-child relationships form a closed cycle
- **WHEN** `propagate_hierarchy_transforms(root_guid)` is invoked
- **THEN** the traversal SHALL terminate cleanly without an infinite loop or stack/heap exhaustion
- **AND** each entity in the cycle SHALL be visited at most once.
