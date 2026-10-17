# Node Packet Wiring, AoI Replication, and Observer Lifecycle Specification

## Purpose
Specifies authoritative simulation node packet routing, spatial world object AoI replication, observer lifecycle eviction, complete entity teardown, and unified damage mitigation pipeline behavior.

---

## Requirements

### Requirement: Authoritative Client Packet Forwarding & Routing
The simulation node SHALL validate upstream session epochs, verify entity existence and lack of migration freeze, return placement error feedback, dispatch interaction events to gameplay plugins with veto enforcement, enqueue world object resync spawn jobs, and dispatch custom plugin packets through `ModularPacketRouter`.

#### Scenario: Fencing stale packets by session epoch
- **GIVEN** an entity with authoritative `session_epoch = 2` on the simulation node
- **WHEN** `ForwardClientPacket` is invoked with `session_epoch = 1` or `session_epoch = 0`
- **THEN** the request SHALL be rejected with `success: false` and error message indicating stale session epoch
- **AND** the packet data SHALL NOT be processed.

#### Scenario: PlaceObject error feedback
- **GIVEN** an invalid placement request (e.g. non-finite transform or line-of-sight obstruction)
- **WHEN** `ForwardClientPacket` is invoked with `PlaceObject`
- **THEN** the simulation node SHALL return `success: false` with the specific placement error reason.

#### Scenario: InteractObject plugin hook dispatch and veto
- **GIVEN** an interactive world object within range of an entity
- **WHEN** `ForwardClientPacket` is invoked with `InteractObject`
- **THEN** the node SHALL dispatch `Event::BeforeInteract` to registered hook handlers
- **AND** IF any handler cancels the event, the interaction SHALL be aborted and return `success: false`
- **AND** IF no handler cancels, the node SHALL dispatch `Event::OnInteract` and return `success: true`.

#### Scenario: WorldObjectResync enqueues spawn packet to observer
- **GIVEN** a player observing a sector with an active world object
- **WHEN** the player sends `WorldObjectResync` for that object ID
- **THEN** an `AoiBroadcastJob` containing `WorldObjectSpawnPacket` SHALL be queued into `pending_broadcast_jobs` for that player's session.

#### Scenario: Modular packet dispatch with anti-spoofing
- **GIVEN** a registered plugin module in `ModularPacketRouter`
- **WHEN** an interaction packet matching the module ID is forwarded
- **THEN** IF `interact.source_guid != 0 && interact.source_guid != req.guid`, the packet SHALL be rejected as spoofed
- **AND** IF valid, `ModularPacketRouter::dispatch` SHALL execute the handler with mutable entity access and return the handler's execution result.

---

### Requirement: World Object Spatial AoI Streaming in Tick Simulation
The simulation node tick step SHALL execute spatial sector visibility updates for world objects every tick.

#### Scenario: World object visibility streaming
- **GIVEN** an active player observer and world objects within observed sectors
- **WHEN** `TickSimulation::step` executes
- **THEN** `AoiManager::update_world_object_observers` SHALL be invoked
- **AND** any resulting `AoiBroadcastJob`s SHALL be queued into `pending_broadcast_jobs`.

---

### Requirement: Observer Eviction on Session Takeover
`AoiManager::register_observer` SHALL evict prior session IDs belonging to the same player GUID.

#### Scenario: Player session takeover evicts old observer
- **GIVEN** player GUID 100 registered with session ID 10
- **WHEN** `register_observer` is called for player GUID 100 with session ID 20
- **THEN** session ID 10 SHALL be removed from `self.observers`
- **AND** session ID 20 SHALL become the sole registered observer for player GUID 100.

---

### Requirement: Complete Entity Lifecycle Cleanup
Entity despawn and outbound migration (both single and batch) SHALL clean up all associated state across AoI, migration tracking, session epochs, and sector rate limits.

#### Scenario: Entity despawn cleans up all simulation tracking
- **GIVEN** an entity registered in `entity_store`, `octree`, `aoi`, `session_epochs`, `last_migration_ticks`, and `sectors.rate_limits`
- **WHEN** `despawn_entity` is called
- **THEN** the entity SHALL be removed from `aoi` observers and GUID maps
- **AND** `last_migration_ticks` SHALL NOT contain the entity GUID
- **AND** `session_epochs` SHALL NOT contain the entity GUID
- **AND** `sectors.rate_limits` SHALL NOT contain the entity GUID.

---

### Requirement: Unified Local & Remote Damage Pipeline
Applying damage or healing to an entity—whether via local ability dispatch, batch AoE dispatch, or remote RPC execution—SHALL follow identical rules for dead target checks, damage mitigation, death event emission, action interruption, and attribute synchronization.

#### Scenario: Single-target ability dead check
- **GIVEN** a dead target entity
- **WHEN** a single-target ability is cast on the entity
- **THEN** the cast SHALL fail with target dead error
- **AND** NO caster resources or cooldowns SHALL be deducted.

#### Scenario: Damage mitigation and death events
- **GIVEN** a live target entity with `StatsComponent` and `HealthComponent`
- **WHEN** damage is applied to the target
- **THEN** `StatsComponent::mitigate_damage` SHALL scale the applied damage
- **AND** IF the entity dies, `EntityDiedEvent` SHALL be emitted to `event_queue`
- **AND** IF the entity has an active action with `cancel_on_damage`, the action SHALL be interrupted
- **AND** IF the entity has an `AttributeSet`, its `"health"` attribute SHALL be synchronized.
