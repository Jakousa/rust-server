# Specification: Decoupled Genre-Agnostic Core Architecture

## Domain: Architecture & Modularity

### Capability: Pluggable Combat Resolution
The simulation core in `mmo-node` SHALL delegate combat calculations (including damage mitigation, armor curves, and vitality reductions) to a pluggable `CombatResolver` trait defined in `mmo-core`, defaulting to a genre-agnostic baseline and allowing `crates/gameplay` plugins to install specialized rules via `PluginContext`.

#### Scenario: Default Combat Resolution
- **GIVEN** a `TickSimulation` instance without external plugins
- **WHEN** combat damage is applied to an entity carrying `AttributeSet` with `PRIMARY_VITALITY`
- **THEN** damage is applied directly without applying fantasy armor mitigation curves
- **AND** if vitality drops to zero or below, the entity is marked incapacitated and an `EntityIncapacitatedEvent` is emitted.

#### Scenario: Plugin Combat Resolution
- **GIVEN** a `TickSimulation` with `CombatPlugin` from `crates/gameplay` installed
- **WHEN** combat damage is applied to an entity carrying armor attributes
- **THEN** the `RpgCombatResolver` calculates mitigation according to the RPG armor curve
- **AND** deducts effective damage from the target's vitality pool.

---

### Capability: Extensible Attribute Registry
`crates/core` SHALL provide an extensible `AttributeRegistry` mapping domain string aliases (e.g. "health", "armor", "hull") to canonical `AttributeId`s, preserving backward compatibility while keeping the mathematical substrate genre-agnostic.

#### Scenario: Domain Attribute Alias Resolution
- **GIVEN** an `AttributeRegistry` with standard mappings
- **WHEN** querying canonical IDs for "vitality", "health", "armor", or custom registered aliases
- **THEN** the matching `AttributeId` is returned consistently.

---

### Capability: Generalized Snapshot Persistence
The storage substrate in `mmo-storage` SHALL persist `entity_snapshots` purely as a generalized key-value/spatial entity snapshot without requiring or issuing queries against the RPG `characters` table.

#### Scenario: Batch Persistence of Non-Character Entities
- **GIVEN** an `EntitySnapshot` batch containing GUIDs of non-character entities (e.g. world objects, vehicles, items)
- **WHEN** `CharacterRepository::save_snapshot_batch` is executed
- **THEN** only the `entity_snapshots` table is updated
- **AND** no queries or schema dependencies are executed against the `characters` table.

---

### Capability: Unified 3D Spatial Vector Substrate
The codebase SHALL maintain a single canonical `Vec3` definition in `mmo-schema::client` that implements standard vector arithmetic, byte codecs, serialization, and coordinate conversion traits, and is re-exported by `mmo-core::spatial`.

#### Scenario: Vector Representation Consistency
- **GIVEN** any component in `mmo-core`, `mmo-schema`, `mmo-node`, or `mmo-gateway`
- **WHEN** spatial coordinates or velocities are manipulated
- **THEN** `mmo_core::spatial::Vec3` and `mmo_schema::client::Vec3` refer to the identical struct definition
- **AND** conversions to/from `(f32, f32, f32)` are zero-cost.

---

### Capability: Generic Edge Packet Interception
`crates/gateway` SHALL provide an `EdgePacketInterceptor` trait allowing edge modules to intercept and handle client packets without hardcoding specific game services into the core gateway packet loop.

#### Scenario: Edge Interceptor Dispatch
- **GIVEN** an `EdgePacketInterceptor` registered on `GatewayServer`
- **WHEN** an inbound client packet matching the interceptor's criteria arrives
- **THEN** the packet is intercepted and handled by the registered interceptor
- **AND** the core gateway loop does not require static match branches for the domain packet type.
