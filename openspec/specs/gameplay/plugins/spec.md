# Gameplay Plugin Architecture Specification

## Purpose

Defines the decoupled plugin execution lifecycle, open typed event bus, dynamic component serialization codec registry, and modular packet routing interfaces for standalone gameplay modules.

## Requirements

### Requirement: Standalone GameplayPlugin Trait
The engine SHALL provide a `GameplayPlugin` trait that encapsulates the registration of dynamic components, event hooks, network packet handlers, and periodic tick systems without requiring compile-time knowledge from the core engine.

#### Scenario: Plugin initialization and registration
- **WHEN** a simulation node initializes with a set of registered plugins
- **THEN** each plugin's `build(&mut PluginContext)` SHALL be invoked, populating component codecs, event listeners, and packet handlers into the node runtime

### Requirement: Open Typed Event Bus with Veto Capability
The engine SHALL provide a type-erased event dispatcher allowing arbitrary, plugin-defined Rust structs to be emitted, received, and optionally vetoed via an `EventContext`.

#### Scenario: Plugin emits custom typed event
- **WHEN** Plugin A emits a custom event struct `DamageEvent { source: Guid, target: Guid, amount: u32 }`
- **THEN** all registered listeners for `DamageEvent` in Plugin B SHALL execute in registration priority order

#### Scenario: Cross-plugin event veto
- **WHEN** an interaction event or movement event is processed
- **THEN** a plugin handler SHALL have the ability to inspect entity state and call `ctx.cancel(reason)`, preventing the downstream core engine or other plugins from completing the action

### Requirement: Dynamic Component Serialization Codec Registry
The engine SHALL maintain a registry mapping `TypeId` and string identifiers to JSON serialization and deserialization functions, enabling arbitrary dynamic components attached to an `Entity` to be persisted into `EntitySnapshot.attributes` without core knowing their concrete types.

#### Scenario: Persisting dynamic plugin component
- **WHEN** the `WriteBehindSnapshotter` captures an entity snapshot
- **THEN** the codec registry SHALL serialize all registered attached components into an `attributes` JSON map keyed by component name

#### Scenario: Restoring dynamic plugin component
- **WHEN** an entity snapshot is loaded from storage during character login or node handover
- **THEN** the codec registry SHALL deserialize the JSON representation back into the concrete type-safe component and attach it to the `Entity`'s `ComponentStore`

### Requirement: Modular Packet Dispatching
The engine SHALL support modular packet routing where client packets with module-scoped opcodes are dispatched directly to the responsible plugin handler.

#### Scenario: Handling plugin-specific packet
- **WHEN** a client sends an action packet addressed to a registered module opcode
- **THEN** the core router SHALL forward the payload to the registered plugin handler with the invoking entity GUID and session context

### Requirement: Event-Driven Despawn and Death Pruning
Gameplay plugins tracking entity references SHALL subscribe to entity despawn and death notifications to prune stale references reactively without executing full-world entity iterations every tick.

#### Scenario: Reactive cleanup on entity despawn
- **WHEN** an entity despawns or dies in the simulation
- **THEN** registered gameplay plugins (such as spawner and threat tracking) SHALL receive notification and remove stale GUID references from internal tracking structures without querying all entities in the entity store
