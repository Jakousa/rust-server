## Why

The core MMO engine currently provides fundamental infrastructure: spatial octree partitioning, 20 Hz simulation ticks, client movement kinematics, networking/gateway routing, and write-behind relational persistence. However, gameplay mechanics (inventory, equipment, combat, trading) must NOT be hardcoded into the simulation core. Hardcoding gameplay features creates monolithic spaghetti, violates single-responsibility principles, makes feature experimentation painful, and prevents isolated testing.

To scale the game logic cleanly—drawing inspiration from proven architectures like Bevy's Plugin system, TrinityCore's script/hook managers, and Veloren's decoupled ECS systems—we need an open, standalone, and completely extensible **Gameplay Plugin Architecture**. This enables developers to create gameplay features as standalone crates or plugins that dynamically hook into entity components, simulation ticks, typed event buses, and network packets without modifying core engine crates.

## What Changes

- Create a plugin framework in `crates/core` (or a dedicated `crates/plugin` workspace crate):
  - `GameplayPlugin` trait defining registration lifecycle (`register_components`, `register_hooks`, `register_packets`, `on_tick`).
  - Open `TypedEventBus` supporting arbitrary module-defined events with synchronous veto pipelines, lifting the restriction of the closed `Event` enum.
  - `ComponentCodecRegistry` enabling dynamic components (`Box<dyn Any>`) to register JSON serialization/deserialization routines for automatic inclusion in `EntitySnapshot.attributes`.
  - `ModularPacketRouter` providing modular opcode dispatching so plugins handle client messages without central schema changes.
- Implement reference classic MMO gameplay plugins:
  - **Inventory & Equipment Plugin**: Item containers, slot constraints, weight limits, and equip/unequip logic with encumbrance hook vetoes.
  - **Combat & Attributes Plugin**: Base stats (health, mana, defense, attack), damage calculation pipeline, tick-based regeneration, and death/respawn lifecycle.
  - **Atomic Trade Plugin**: Safe, transactional two-player trade sessions with lock-step confirmation and rollback protection against item duplication.
- Integrate the plugin registry into `TickSimulation` in `crates/node`, allowing simulation nodes to load plugins dynamically on startup.

## Capabilities

### New Capabilities
- `gameplay/plugins`: Provides the decoupled `GameplayPlugin` lifecycle, type-erased event bus, component serialization codec registry, and modular packet router.
- `gameplay/inventory`: Manages inventory containers, item definitions, equipment slots, and encumbrance vetoes.
- `gameplay/combat`: Manages character stats, health/mana pools, damage calculation, and death state transitions.
- `gameplay/trade`: Manages atomic two-phase item and currency exchange between player entities.

### Modified Capabilities
- `entity/storage`: Extended with component serialization hooks for dynamic persistence.
- `interaction/hooks`: Augmented with a generic typed event bus for cross-module communication.

## Impact

- Architecture remains strictly decoupled: core crates (`core`, `node`, `schema`, `storage`) know nothing about specific items, stats, or trade rules.
- Fast compile times: gameplay systems can be compiled as separate crates or unit-tested in complete isolation.
- Extensibility: new features (e.g. Quests, Guilds, Crafting) can be added as standalone plugins without modifying core files.
