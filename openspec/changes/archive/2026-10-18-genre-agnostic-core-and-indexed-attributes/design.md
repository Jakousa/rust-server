# Technical Design: Genre-Agnostic Core Decoupling & Indexed Attributes

## 1. Generalized Core Primitives (`crates/core`)

### 1.1 `Entity::state_flags` & `EntityDepletedEvent` (`crates/core/src/entity.rs` & `plugin.rs`)
- In `crates/core/src/entity.rs`, add `pub state_flags: u32` (default `0`) to `Entity`, along with module `pub mod state_flags`:
  ```rust
  pub mod state_flags {
      pub const INCAPACITATED: u32 = 1 << 0;
      pub const MOVEMENT_LOCKED: u32 = 1 << 1;
      pub const COMBAT_ACTIVE: u32 = 1 << 2;
      pub const INVULNERABLE: u32 = 1 << 3;
  }
  ```
- Add helper methods on `Entity`:
  - `pub fn is_incapacitated(&self) -> bool { (self.state_flags & state_flags::INCAPACITATED) != 0 }`
  - `pub fn is_movement_locked(&self) -> bool { (self.state_flags & state_flags::MOVEMENT_LOCKED) != 0 }`
- In `crates/core/src/plugin.rs`, add generic domain event:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct EntityIncapacitatedEvent {
      pub entity: Guid,
      pub instigator: Option<Guid>,
  }
  impl DomainEvent for EntityIncapacitatedEvent {}
  ```

### 1.2 `AttributeId(u8)` in `crates/core/src/attribute.rs`
- Define `AttributeId(pub u8)`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
  pub struct AttributeId(pub u8);

  impl AttributeId {
      pub const PRIMARY_VITALITY: Self = Self(1);
      pub const MAX_PRIMARY_VITALITY: Self = Self(2);
      pub const SECONDARY_RESOURCE: Self = Self(3);
      pub const MAX_SECONDARY_RESOURCE: Self = Self(4);
      pub const MOVE_SPEED_SCALE: Self = Self(5);
      pub const MITIGATION: Self = Self(6);
  }
  ```
- Add `pub fn canonical_id_for_name(name: &str) -> Option<AttributeId>` mapping:
  - `"health"` | `"vitality"` | `"hull"` -> `Some(AttributeId::PRIMARY_VITALITY)`
  - `"max_health"` | `"max_vitality"` | `"max_hull"` -> `Some(AttributeId::MAX_PRIMARY_VITALITY)`
  - `"mana"` | `"energy"` | `"capacitor"` | `"stamina"` -> `Some(AttributeId::SECONDARY_RESOURCE)`
  - `"max_mana"` | `"max_energy"` | `"max_capacitor"` | `"max_stamina"` -> `Some(AttributeId::MAX_SECONDARY_RESOURCE)`
  - `"move_speed_scale"` -> `Some(AttributeId::MOVE_SPEED_SCALE)`
  - `"armor"` | `"mitigation"` | `"resistance"` -> `Some(AttributeId::MITIGATION)`
- Provide `get_by_id(&self, id: AttributeId) -> Option<&AttributeValue>` and `iter_indexed(&self)` on `AttributeSet`.

### 1.3 Move `CooldownTracker`, `ActionCooldownSystem`, and `ActionDefinition` to `crates/core/src/action.rs`
- Move `CooldownTracker`, `AbilityDefinition` (aliased as `ActionDefinition` in `mmo_core::action` and re-exported as `AbilityDefinition` in `mmo_gameplay::ability`), `ActionCooldownSystem` (re-exported as `AbilityTickSystem` in `mmo_gameplay::ability`), and `resolve_boundary_sphere_aoe` into `crates/core/src/action.rs`.
- In `crates/core/src/script.rs`, define:
  ```rust
  pub trait ScriptReloadable: Send + Sync {
      fn reload_script_bytes(&self, wasm_bytes: &[u8]) -> Result<u64, String>;
      fn active_version(&self) -> u64;
  }
  ```
  and implement `ScriptReloadable` for `mmo_gameplay::ScriptHotReloader`.

---

## 2. Decouple `crates/node` from `mmo-gameplay`

1. **`crates/node/src/aoi.rs` (`extract_entity_attributes`)**:
   - Remove `use mmo_gameplay::combat::HealthComponent;`.
   - Read vitality and attributes from `ent.components.get_component::<mmo_core::AttributeSet>()` and `ent.properties` / `ent.state_flags`.
   - In `crates/gameplay/src/combat.rs`, ensure `HealthComponent` changes synchronize `"health"` and `"max_health"` into `AttributeSet` (and set `entity.state_flags |= state_flags::INCAPACITATED` when `is_dead`).
2. **`crates/node/src/combat_pipeline.rs` & `ability_dispatcher.rs`**:
   - Operate on `mmo_core::AttributeSet` (`"health"`, `"armor"`/`AttributeId::MITIGATION`), `entity.state_flags`, `mmo_core::action::{CooldownTracker, ActionDefinition, resolve_boundary_sphere_aoe}`, and emit `mmo_core::plugin::EntityIncapacitatedEvent`.
3. **`crates/node/src/sim.rs`**:
   - Replace `Option<Arc<mmo_gameplay::ScriptHotReloader>>` with `Option<Arc<dyn mmo_core::script::ScriptReloadable>>`.
   - Register `mmo_core::action::CooldownTracker` and `mmo_core::action::ActionCooldownSystem` in `TickSimulation::with_kinematics`.
   - In `process_client_move_with_seq` and `process_client_move_deterministic`, check `if entity.is_incapacitated() || entity.is_movement_locked()` and dispatch `Event::BeforeMove` (where `InventoryPlugin`'s `EncumbranceHookHandler` and `CombatPlugin`'s death veto hook cancel movement if needed).
4. **`crates/node/Cargo.toml`**:
   - Move `mmo-gameplay = { path = "../gameplay" }` from `[dependencies]` to `[dev-dependencies]`.

---

## 3. Extensible `InteractionType` in `crates/schema/src/client.rs`

In `crates/schema/src/client.rs` (`lines 224-257`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionType {
    Talk,
    Attack,
    UseItem,
    Loot,
    Trade,
    Custom(u8),
}
```
- `from_u8(val: u8) -> Result<Self, ClientCodecError>` maps `0..=4` to the named variants and `5..=255` to `Ok(Self::Custom(val))`.
- `as_u8(&self) -> u8` returns `0..=4` for named variants and `code` for `Self::Custom(code)`.
