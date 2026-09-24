# Modular Gameplay Plugin Architecture Design

## Architecture Overview

To achieve complete isolation between core simulation infrastructure and gameplay mechanics, this design introduces a plugin ecosystem modeled after **Bevy's Plugin/System pattern** and **TrinityCore's Script/Hook managers**, tailored for Rust's strict ownership model and our 20 Hz tick architecture.

Following architectural verification, the design adopts a dual event model:
1. **Synchronous Veto Pipeline** (`HookDispatcher`): Lightweight, synchronous hooks for pre-action validation (`BeforeMove`, `BeforeInteract`) returning `EventContext`.
2. **Deferred Domain Event Queue** (`EventQueue`): Buffered typed events (`DamageEvent`, `EntityDiedEvent`, `TradeCompletedEvent`) drained at tick boundaries to prevent re-entrancy, stack overflow, and Rust borrow-checker aliasing conflicts.
3. **Batch `TickSystem`s**: Instead of firing expensive per-entity dynamic tick events at 20 Hz, plugins register systems that iterate over entities in bulk.

```
+---------------------------------------------------------------------------------------+
|                                    Plugin Runtime                                     |
|                                                                                       |
|   +-----------------------+   +-----------------------+   +-----------------------+   |
|   |    InventoryPlugin    |   |     CombatPlugin      |   |      TradePlugin      |   |
|   | - InventoryComponent  |   | - HealthComponent     |   | - TradeSession        |   |
|   | - EquipSlots          |   | - RegenTickSystem     |   | - 2-Phase Commit      |   |
|   | - Encumbrance Veto    |   | - DamageEventQueue    |   | - Disjoint Mut Swap   |   |
|   +-----------------------+   +-----------------------+   +-----------------------+   |
+---------------------------------------------------------------------------------------+
                                          |
                                          | registers via PluginContext
                                          v
+---------------------------------------------------------------------------------------+
|                                 Plugin Infrastructure                                 |
|                                                                                       |
|   +-----------------------+   +-----------------------+   +-----------------------+   |
|   |     EventQueue        |   | ComponentCodecRegistry|   |  ModularPacketRouter  |   |
|   | - Type-erased FIFO    |   | - Component -> JSON   |   | - Opcode -> Handler   |   |
|   | - Drained at tick end |   | - JSON -> Raw Insert  |   | - Client Payload      |   |
|   | - Zero re-entrancy    |   | - Snapshot Pipeline   |   | - Route by Entity GUID|   |
|   +-----------------------+   +-----------------------+   +-----------------------+   |
+---------------------------------------------------------------------------------------+
                                          |
                                          | consumed by
                                          v
+---------------------------------------------------------------------------------------+
|                            Core Engine (TickSimulation)                               |
|   - 20 Hz Tick Loop           - Sparse 3D Octree Partitioning                         |
|   - SlotMap Generational Arena- Boundary Ghost Synchronization                        |
|   - Disjoint Entity Borrow    - Write-Behind Relational Snapshotter                   |
+---------------------------------------------------------------------------------------+
```

---

## 1. Core Primitives

### 1.1 The `GameplayPlugin` Trait

```rust
pub trait GameplayPlugin: Send + Sync {
    /// Unique identifier for this plugin (e.g. "inventory", "combat", "trade").
    fn id(&self) -> &'static str;

    /// Registers components, event handlers, tick systems, and packet routes.
    fn build(&self, ctx: &mut PluginContext);
}
```

### 1.2 `PluginContext`

```rust
pub struct PluginContext<'a> {
    pub hooks: &'a mut HookDispatcher,
    pub event_queue: &'a mut EventQueueRegistry,
    pub codecs: &'a mut ComponentCodecRegistry,
    pub packets: &'a mut ModularPacketRouter,
    pub tick_systems: &'a mut Vec<Box<dyn TickSystem>>,
}
```

### 1.3 `TickSystem` Trait (Batch Processing)

Avoids per-entity dynamic dispatch at 20 Hz. Systems run once per tick in bulk:
```rust
pub trait TickSystem: Send + Sync {
    fn name(&self) -> &'static str;
    fn update(&mut self, entities: &mut EntityStore, dt_sec: f32);
}
```

### 1.4 Deferred `EventQueue` (Eliminating Borrow Conflicts & Re-entrancy)

Domain events are enqueued during the tick and drained at dedicated phase boundaries:

```rust
pub trait DomainEvent: 'static + Send + Sync {}

pub struct EventQueue {
    queue: Vec<Box<dyn Any + Send + Sync>>,
}

impl EventQueue {
    pub fn push<E: DomainEvent>(&mut self, event: E) {
        self.queue.push(Box::new(event));
    }

    pub fn drain<E: DomainEvent>(&mut self) -> Vec<E> {
        // Extracts all queued events matching type E
    }
}
```

### 1.5 `ComponentStore` & `EntityStore` Primitives

To support dynamic persistence and multi-entity interactions (e.g., Trade):
- `ComponentStore::iter(&self) -> impl Iterator<Item = (&TypeId, &Box<dyn Any + Send + Sync>)>`
- `ComponentStore::insert_raw(&mut self, type_id: TypeId, component: Box<dyn Any + Send + Sync>)`
- `EntityStore::get_two_mut(&mut self, g1: Guid, g2: Guid) -> Option<(&mut Entity, &mut Entity)>` (using disjoint checks to safely borrow both trading players mutably).

### 1.6 `ComponentCodecRegistry` (Dynamic Persistence)

```rust
pub trait ComponentCodec: Send + Sync {
    fn type_name(&self) -> &'static str;
    fn type_id(&self) -> TypeId;
    fn serialize(&self, boxed: &(dyn Any + Send + Sync)) -> Result<serde_json::Value, CodecError>;
    fn deserialize(&self, value: serde_json::Value) -> Result<Box<dyn Any + Send + Sync>, CodecError>;
}
```

During snapshotting:
1. `WriteBehindSnapshotter` queries `codec_registry.serialize_all(&entity.components)`.
2. Registered components produce an `attributes` JSON map keyed by type name.
3. On load, `codec_registry.deserialize_into(attributes_json, &mut entity.components)` restores concrete structs into `ComponentStore` via `insert_raw`.

### 1.7 `ModularPacketRouter`

```rust
pub trait ModularPacketHandler: Send + Sync {
    fn handle(
        &self,
        entity_guid: Guid,
        sub_opcode: u16,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketError>;
}
```

---

## 2. Reference Gameplay Plugins

### 2.1 Inventory & Equipment Plugin (`InventoryPlugin`)
- **Components:**
  - `InventoryComponent`: Fixed capacity grid (slots: `Vec<Option<ItemStack>>`, current_weight, max_weight).
  - `EquipmentComponent`: Slot map (e.g. `Head`, `Chest`, `MainHand`, `OffHand`, `Boots`).
- **Behaviors:**
  - Synchronous Veto: Subscribes to `BeforeMove`. If `inventory.current_weight > inventory.max_weight`, calls `ctx.cancel("Player is encumbered")`.
  - Packet Handler: Handles `EquipItemRequest`. Moves item from inventory slot to equipment slot, recalculates weight, and enqueues `OnEquipmentChangedEvent`.
  - Codecs: Registers JSON codecs for `InventoryComponent` and `EquipmentComponent`.

### 2.2 Combat & Attributes Plugin (`CombatPlugin`)
- **Components:**
  - `HealthComponent`: `current_hp: f32`, `max_hp: f32`, `is_dead: bool`, `regen_per_sec: f32`.
  - `StatsComponent`: `attack_power: u32`, `armor: u32`.
- **Behaviors:**
  - Synchronous Veto: In `BeforeMove`, if `health.is_dead`, cancels movement.
  - Batch Tick System: `HealthRegenSystem` iterates all entities with `HealthComponent` once per tick, adding `regen_per_sec * dt_sec` without event overhead.
  - Event Handling: Processes queued `DamageEvent`s: applies armor mitigation (`damage * 100 / (100 + armor)`), decrements HP. If `<= 0.0`, marks `is_dead = true` and emits `EntityDiedEvent`.

### 2.3 Atomic Trade Plugin (`TradePlugin`)
- **Components / State:**
  - `TradeSessionManager`: Tracks active trade sessions between two entities (`player_a`, `player_b`), offered items, currency, and mutual lock/accept flags.
- **Behaviors:**
  - Proximity Check: If either player moves (`BeforeMove`/`OnMove`) beyond 5.0m, the trade session aborts and items return to owners.
  - Atomic Swap: When both players accept, uses `EntityStore::get_two_mut(p1, p2)` to atomically mutate both inventories in a single operation. If either inventory lacks capacity, the transaction rolls back cleanly with zero item duplication.

