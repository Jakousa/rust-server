## 1. Core Primitives & Storage Extensions (`crates/core`)

- [x] 1.1 Extend `ComponentStore` in `crates/core` with `iter(&self)` and `insert_raw(TypeId, Box<dyn Any + Send + Sync>)` to enable dynamic iteration and deserialization
- [x] 1.2 Extend `EntityStore` in `crates/core` with `get_two_mut(g1, g2)` providing safe disjoint simultaneous mutable borrows of two entities
- [x] 1.3 Implement `ComponentCodecRegistry` mapping `TypeId` to JSON serialization/deserialization functions and test with mock components
- [x] 1.4 Implement deferred `EventQueue` for buffered domain events and `TickSystem` trait for batch entity processing
- [x] 1.5 Implement `ModularPacketRouter` dispatching module-scoped client opcodes to plugin handlers
- [x] 1.6 Define `GameplayPlugin` and `PluginContext` traits and verify plugin registration via unit tests

## 2. Reference Gameplay Plugins

- [x] 2.1 Implement `InventoryPlugin` with `InventoryComponent`, `EquipmentComponent`, encumbrance veto on `BeforeMove`, and JSON component codecs
- [x] 2.2 Implement `CombatPlugin` with `HealthComponent`, `StatsComponent`, batch `HealthRegenSystem`, and queued `DamageEvent` processing
- [x] 2.3 Implement `TradePlugin` with two-player trade session lifecycle, mutual lock/acceptance, distance abort hook, and atomic disjoint inventory swap

## 3. Node Integration & Verification

- [x] 3.1 Update `TickSimulation` in `crates/node` to initialize plugins, run batch `TickSystem`s, and drain `EventQueue` at tick boundaries
- [x] 3.2 Update `WriteBehindSnapshotter` to serialize dynamic plugin components into `EntitySnapshot.attributes` using `ComponentCodecRegistry`
- [x] 3.3 Create end-to-end integration test verifying plugin loading, encumbered movement veto, damage reduction, and state persistence recovery
