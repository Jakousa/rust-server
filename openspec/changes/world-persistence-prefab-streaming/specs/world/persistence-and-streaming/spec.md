# Specification: Persistent World Objects, Dynamic Prefabs & Spatial Sector Streaming

## 1. Generalized World Object Data Model

### Requirement: Uniform World Object Abstraction
The system SHALL represent stationary, interactive, and constructible world state using a generic `WorldObject` structure identified by a 64-bit generational `Guid`.

- **GIVEN** an interactive or persistent entity in the world (such as a door, chest, mining node, or player barricade)
- **WHEN** instantiated in memory or stored in persistence
- **THEN** it SHALL be represented by `WorldObject` containing:
  - `object_id: Guid`
  - `prefab_id: u32`
  - `transform: Transform3D` (position, rotation quaternion, scale)
  - `bounds: BoundingBox3D`
  - `state_flags: u32`
  - `owner: ContainerOwner` (`Character(u64)`, `Guild(u64)`, `WorldEntity(u64)`, `System(String)`)
  - `health: Option<HealthComponent>`
  - `properties: HashMap<String, String>`
- **AND** the data structure SHALL contain zero hardcoded game genre or design assumptions.

### Requirement: Discrete Spatial Sector Coordinate Indexing
World objects SHALL be spatially grouped into uniform cubic sectors of $64\text{m} \times 64\text{m} \times 64\text{m}$ indexed by integer coordinates $(S_x, S_y, S_z)$.

- **GIVEN** world coordinates $(x, y, z)$ in floating-point meters
- **WHEN** calculating the containing sector
- **THEN** sector coordinates SHALL be $\lfloor x / 64.0 \rfloor, \lfloor y / 64.0 \rfloor, \lfloor z / 64.0 \rfloor$
- **AND** spatial lookups by sector SHALL execute in $O(1)$ expected time via hashed spatial maps.

---

## 2. Authoritative Object Placement & Anti-Cheat Validation

### Requirement: Server-Authoritative Placement Verification
All object placements (construction, deployment, dropped interactables) SHALL be validated strictly on the authoritative simulation node.

- **GIVEN** a client sending a `PlaceObjectRequest`
- **WHEN** processed by the simulation node
- **THEN** the node SHALL verify:
  1. Target placement coordinates reside within the node's authoritative spatial partition.
  2. Interaction distance $|P_{\text{player}} - P_{\text{target}}| \le 6.0\text{m}$ evaluated strictly against the server's authoritative player entity position from `EntityStore`.
  3. Line-of-sight raycast from the player's authoritative position to target placement coordinates does not intersect occluding solid geometry (preventing placement through walls or inside closed rooms).
  4. Client session does not exceed a token-bucket rate limit (maximum 10 placement/interaction operations per second).
  5. The object's calculated bounding box does not penetrate existing immovable world obstacles or player kinematic capsules.
- **AND** upon successful verification, the object SHALL be inserted into the active sector and staged for replication and persistence.

### Requirement: Authoritative Interaction & State Transition
State transitions (opening chests, unlocking doors, harvesting nodes, dealing damage to structures) SHALL be authoritatively validated.

- **GIVEN** an interaction request directed at `object_id`
- **WHEN** evaluating interaction eligibility
- **THEN** the node SHALL verify distance ($D \le 6.0\text{m}$), line-of-sight raycast, ownership/access credentials, and state flags (e.g. not destroyed)
- **AND** if valid, emit domain events to registered `GameplayPlugin` handlers and increment the object's `dirty_revision`.

---

## 3. Spatial Sector Partitioning & AoI Streaming

### Requirement: Area of Interest Sector Observation & Queue Pruning
The AoI observation system SHALL track visible spatial sectors for each observer and manage sector object lifecycles (`entered`, `updated`, `exited`).

- **GIVEN** a player observer with position $P$ and observation radius $R$
- **WHEN** calculating visible sectors in the tick loop
- **THEN** all sectors intersecting the observation sphere $(P, R)$ SHALL be marked visible
- **AND** newly visible world objects SHALL be scheduled for paced transmission to the client
- **AND** world objects in exiting sectors SHALL be scheduled for unload packets
- **AND** any pending unstreamed spawn packets for objects in an exited sector SHALL be immediately pruned from the observer's transmission queue to prevent wasted egress bandwidth.

### Requirement: Bandwidth-Paced Object Delivery
World object initial state packets SHALL be prioritized and bandwidth-throttled.

- **GIVEN** multiple world objects entering an observer's AoI simultaneously
- **WHEN** dispatching object state packets
- **THEN** transmissions SHALL be prioritized by Euclidean proximity to the observer
- **AND** capped at a configurable rate (default: maximum 10 objects per tick per client) to prevent egress saturation.

### Requirement: Reliable Delta State Synchronization
All incremental state modifications (health changes, state flag flips, destruction) SHALL be broadcast over reliable stream channels.

- **GIVEN** an in-world state change on a `WorldObject`
- **WHEN** broadcasting to nearby observers
- **THEN** updates SHALL be sent via `WorldObjectDeltaPacket` carrying monotonic `dirty_revision` numbers over reliable stream sinks
- **AND** if a client detects a revision sequence gap, it SHALL transmit `WorldObjectResyncRequest` to fetch the authoritative current state.

---

## 4. Asynchronous Write-Behind Persistence

### Requirement: Non-Blocking Coalesced Object Persistence
Modified world objects SHALL be persisted asynchronously without stalling the 20 Hz simulation tick loop.

- **GIVEN** dirty world object modifications during tick execution
- **WHEN** staging objects into `WorldObjectWriteBehindSnapshotter`
- **THEN** staging SHALL only acquire an in-memory lock ($< 1\,\mu\text{s}$) to clone the object record
- **AND** multiple modifications to the same object within the flush interval SHALL coalesce into the same queued record
- **AND** a dedicated background worker SHALL execute SQL batch upserts (`INSERT ... ON CONFLICT DO UPDATE`).

### Requirement: Dual-Engine Schema Migration
The storage tier SHALL provide migration `V4__world_objects.sql` supporting PostgreSQL and SQLite.

- **GIVEN** a database managed by `mmo-storage`
- **WHEN** migrations run
- **THEN** table `world_objects` SHALL be created with primary key `(world_id, object_id)` and spatial index on `(world_id, sector_x, sector_y, sector_z)`.
