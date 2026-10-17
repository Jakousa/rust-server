# Technical Design: Trade Swap, Ability Pre-Checks, Guild Roster, WASM Host Sync & Plugin Wiring

## 1. Atomic Trade Swap & Currency Settlement (`crates/gameplay/src/inventory.rs` & `trade.rs`)

### 1.1 `InventoryComponent` Currency & Item Removal (`crates/gameplay/src/inventory.rs:39-105`)
- Add `#[serde(default)] pub currency: u64` to `InventoryComponent` (initialized to `0` in `InventoryComponent::new`).
- Add helper methods on `InventoryComponent`:
  - `pub fn has_item_stack(&self, item: &ItemStack) -> bool`: verifies that slots with `slot.item_id == item.item_id && slot.name == item.name` have total quantity `>= item.quantity`.
  - `pub fn remove_item_stack(&mut self, item: &ItemStack) -> bool`: deducts `item.quantity` from matching slot(s), setting slot to `None` when quantity reaches `0`.

### 1.2 `TradeManager::execute_atomic_swap` (`crates/gameplay/src/trade.rs:209-268`)
1. Verify currency balances:
   - `if inv1.currency < session.offer_a.currency { return Err("Player A has insufficient currency"); }`
   - `if inv2.currency < session.offer_b.currency { return Err("Player B has insufficient currency"); }`
2. Verify item ownership on cloned test inventories (or pre-check):
   - Clone `let mut test_inv1 = inv1.clone();` and `let mut test_inv2 = inv2.clone();`.
   - For each `item` in `&session.offer_a.items`:
     `if !test_inv1.remove_item_stack(item) { return Err("Player A no longer possesses offered item"); }`
   - For each `item` in `&session.offer_b.items`:
     `if !test_inv2.remove_item_stack(item) { return Err("Player B no longer possesses offered item"); }`
   - Check post-removal weight limits:
     - `let weight_a_to_b: f32 = session.offer_a.items.iter().map(|i| i.total_weight()).sum();`
     - `let weight_b_to_a: f32 = session.offer_b.items.iter().map(|i| i.total_weight()).sum();`
     - `if test_inv1.current_weight() + weight_b_to_a > test_inv1.max_weight { return Err("Player A would exceed max weight"); }`
     - `if test_inv2.current_weight() + weight_a_to_b > test_inv2.max_weight { return Err("Player B would exceed max weight"); }`
   - For each `item` in `&session.offer_b.items`:
     `if !test_inv1.add_item(item.clone()) { return Err("Player A has insufficient inventory slots"); }`
   - For each `item` in `&session.offer_a.items`:
     `if !test_inv2.add_item(item.clone()) { return Err("Player B has insufficient inventory slots"); }`
   - Settle currency on `test_inv1` and `test_inv2`:
     `test_inv1.currency = test_inv1.currency.checked_sub(session.offer_a.currency).and_then(|c| c.checked_add(session.offer_b.currency)).ok_or("Currency overflow")?;`
     `test_inv2.currency = test_inv2.currency.checked_sub(session.offer_b.currency).and_then(|c| c.checked_add(session.offer_a.currency)).ok_or("Currency overflow")?;`
   - Commit `*inv1 = test_inv1;` and `*inv2 = test_inv2;` back into `p1` and `p2`!
   - Update `test_atomic_trade_lifecycle_and_swap` in `crates/gameplay/src/trade.rs` to initialize `inv1.currency = 100` and assert that `p1_inv.slots[0] == Some(shield)` and `p2_inv.slots[0] == Some(sword)` with `p1_inv.currency == 50` and `p2_inv.currency == 50`.

---

## 2. Ability Range & `ActionQueue` Pre-Validation (`crates/gameplay/src/ability.rs:100-159`)

In `execute_or_start_ability`:
1. **Target & Range Check (Before Mutable Caster Borrow)**:
   - Look up caster position: `let caster_pos = entities.get_by_guid(entity_guid).ok_or("Entity not found in store")?.position;`
   - If `let Some(target) = target_guid`:
     - If `ability.range > 0.0`:
       - Look up `target_ent = entities.get_by_guid(target).ok_or("Target entity not found in store")?;`
       - Compute squared distance `dx*dx + dy*dy + dz*dz`; if `dist_sq > ability.range * ability.range`, return `Err("Target is out of range")`.
2. **Pre-Validate Cooldowns & Resource Balance (Read-Only)**:
   - Borrow `entity = entities.get_mut_by_guid(entity_guid).ok_or("Entity not found in store")?;`
   - Check `CooldownTracker`: `if cd.is_on_cooldown(ability.id) { return Err("Ability is currently on cooldown"); }`
   - If `let Some((ref resource_name, cost)) = ability.resource_cost`:
     - Verify `AttributeSet` exists and `attrs.get_current(resource_name).ok_or("Required resource attribute not found")? >= cost`.
3. **Validate & Start `ActionQueue` Before Deducting Resources/Cooldowns**:
   - If `ability.cast_time_sec > 0.0`:
     - Construct `TimedAction` and call `queue.start_action(entity_guid, timed_action, QueuePolicy::RejectIfBusy, events)?` **first**.
   - Else if `let Some(queue) = entity.get_component::<ActionQueue>()`:
     - If `queue.is_busy()`, return `Err("Entity is currently busy with an active action")`.
4. **Deduct Resource & Start Cooldown**:
   - Now that `ActionQueue` accepted the action (or confirmed non-busy), deduct `attrs.modify_current(resource_name, -cost)?` and call `cd.start_cooldown(ability.id, ability.cooldown_sec, ability.gcd_sec)`.

---

## 3. Guild Roster Validation (`crates/gameplay/src/guild.rs:129-165`)

- In `GuildManager::add_member` (`lines 150-160`):
  ```rust
  if self.player_to_guild.contains_key(&player) {
      return Err("Player already in a guild");
  }
  ```
- In `GuildManager::create_guild` (`lines 130-139`):
  - Add `pub fn try_create_guild(&mut self, name: &str, leader: Guid, vault_capacity: usize) -> Result<u64, &'static str>` which rejects if `self.player_to_guild.contains_key(&leader)`.
  - In `create_guild`, also check if `leader` is already in a previous guild (`if let Some(old_gid) = self.player_to_guild.get(&leader).copied() { if let Some(old_g) = self.guilds.get_mut(&old_gid) { old_g.members.remove(&leader); } }`) so `player_to_guild` and `guild.members` can never become inconsistent.
  - Add `pub fn remove_member(&mut self, player: Guid) -> Result<GuildRank, &'static str>` that removes `player` from `self.player_to_guild` and the corresponding `GuildState::members`.

---

## 4. WASM Host API Safety & Synchronization (`crates/gameplay/src/scripting/`)

### 4.1 `HostContext` (`crates/gameplay/src/scripting/host_api.rs:9-44`)
- Change pointer fields to private:
  ```rust
  pub struct HostContext {
      entities_ptr: *mut EntityStore,
      octree_ptr: *mut mmo_core::Octree,
      events_ptr: *mut mmo_core::EventQueue,
      pub execution_logs: Vec<String>,
  }
  ```
- Provide `pub(crate) fn with_tick_context(entities: &mut EntityStore, octree: &mut mmo_core::Octree, events: &mut mmo_core::EventQueue) -> Self`.
- In `entity_set_pos` (`lines 123-143`):
  - Check `if !x.is_finite() || !y.is_finite() || !z.is_finite() { return -1; }`.
  - When updating `entity.position`: save `let old_pos = entity.position; entity.position = (x, y, z);`, and if `!octree_ptr.is_null()`, update `octree`:
    ```rust
    let octree = unsafe { &mut *octree_ptr };
    if !octree.remove(target_guid, old_pos) {
        octree.remove_by_guid(target_guid);
    }
    octree.insert(target_guid, (x, y, z));
    ```
- In `entity_modify_health` (`lines 170-198`):
  - Compute `let died = if delta < 0 { health.take_damage(-(delta as i64) as f32) } else { health.heal(delta as f32); false };` and `let new_hp = health.current_hp;`.
  - Sync `AttributeSet`: if `let Some(attrs) = entity.components.get_component_mut::<mmo_core::AttributeSet>()`, call `let _ = attrs.set_current("health", new_hp);`.
  - If `died && !events_ptr.is_null()`:
    ```rust
    let events = unsafe { &mut *events_ptr };
    events.push( crate::combat::EntityDiedEvent { entity: target_guid, killer: None } );
    ```
- In `ScriptTickSystem::update` (`crates/gameplay/src/scripting/plugin.rs:158-175`) and `WasmRuntime` (`runtime.rs`):
  - Add `execute_function_with_tick_context` passing `ctx.entities`, `ctx.octree`, and `ctx.events`.

---

## 5. Wire `EncumbranceHookHandler` & `DamageProcessingSystem` (`inventory.rs` & `combat.rs`)

- In `crates/gameplay/src/inventory.rs`:
  - Implement `handle_with_entities` on `EncumbranceHookHandler` to check `Event::BeforeMove { entity, .. }` -> `if check_encumbrance(&ent.components) { ctx.cancel("encumbered"); }`.
  - In `InventoryPlugin::build` (`line 212`), register `ctx.hooks.register_handler(Box::new(EncumbranceHookHandler));`.
- In `crates/gameplay/src/combat.rs`:
  - Add `pub struct DamageProcessingSystem;` implementing `TickSystem` (`update` calls `let _ = process_damage_events(ctx.events, ctx.entities);`).
  - Register `ctx.tick_systems.push(Box::new(DamageProcessingSystem));` in `CombatPlugin::build`.
