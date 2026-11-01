# Design Document: Gameplay Systems Resilience, State Consistency, and WASM Limits

## 1. Architectural Overview
This change hardens Partition 5 (`crates/gameplay`) against data loss, entity invulnerability, Denial of Service via WASM OOM memory exhaustion, unilateral trade exploits, and AI drop-aggro anomalies.

```mermaid
flowchart TD
    subgraph Inventory ["Inventory & Equipment"]
        Swap["Equip Swap"] --> Check["Vacated Slot Check & Pre-validation"]
        Check -->|Success| Apply["Apply Equip & Store Unequipped"]
        Check -->|Failure| Rollback["Atomic Rollback (Zero Item Loss)"]
        Unequip["Unequip Item"] --> CapCheck["Check Free Slots > 0"]
        CapCheck -->|No Space| UnequipErr["Reject without Removing Item"]
        CapCheck -->|Has Space| UnequipOk["Remove from Equip & Add to Bag"]
    end

    subgraph Combat ["Combat & Health"]
        Dmg["take_damage(amount)"] --> FinCheck["amount.is_finite() && amount > 0"]
        FinCheck -->|Valid| HPUpdate["Deduct HP & Check Death"]
        FinCheck -->|Invalid (NaN/Inf)| Reject["Reject (Keep Finite HP & Life State)"]
        Respawn["respawn(initial_hp)"] --> HPCheck["hp.is_finite() ? hp : max_hp"]
        HPCheck --> Clamp["clamp(1.0, max_hp)"]
    end

    subgraph Scripting ["WASM Sandbox"]
        Guest["Guest Script (memory.grow)"] --> Limiter["wasmtime::StoreLimits (32MB Cap)"]
        Limiter -->|<= 32MB| AllowGrowth["Allow Page Allocation"]
        Limiter -->|> 32MB| DenyGrowth["Deny (-1 / Trap) - No Host OOM"]
    end

    subgraph Trade ["Bilateral Trade"]
        AtomicSwap["execute_atomic_swap"] --> PreCheck["Entity & Inventory Existence Check"]
        PreCheck -->|Missing Component| ResetPreLocks["Reset bilateral accepted & locked flags"]
        PreCheck -->|Exists| Val["Staging Inventory Validation"]
        Val -->|Validation Failure| ResetLocks["Reset bilateral accepted & locked flags"]
        Val -->|Success| Commit["Commit Disjoint Inventories"]
    end

    subgraph AI ["AI Brain FSM"]
        ThreatEval["Threat Table Evaluation"] --> TargetCheck{"current_threat == active_target?"}
        TargetCheck -->|Same| Maintain["Continue Chase / Attack"]
        TargetCheck -->|Different| SwitchChase["Direct Transition to Chase(new_target)"]
        TargetCheck -->|None| Evade["Evade to Spawn Origin"]
    end
```

## 2. Component Design & Changes

### 2.1. Transactional Inventory & Equipment Operations (`inventory.rs`)
- In `InventoryPacketHandler::handle`:
  - **Sub-opcode 1 (Equip)**:
    - Vacate `inv_slot` via `inv.remove_item(inv_slot)`.
    - If swapping out an `old_item`:
      - First attempt placing into `inv.slots[inv_slot]`.
      - If that slot is unavailable or an error occurs when adding to inventory, immediately rollback:
        - Re-equip `old_item` into `equip`.
        - Re-insert the new `item` into `inv.slots[inv_slot]`.
        - Return error without dropping any items.
    - Return `Ok(Some(vec![1]))` on success.
  - **Sub-opcode 2 (Unequip)**:
    - Payload: `[equip_slot_u8]`.
    - Decode `equip_slot_u8`: 0 = Head, 1 = Chest, 2 = MainHand, 3 = OffHand, 4 = Boots; any other index returns `MalformedPayload`.
    - Validate that `equip.get_equipped(&equip_slot).is_some()`; if empty, return `Err(PacketHandlerError::Custom("No item equipped in specified slot"))`.
    - Pre-validate that `inv` contains at least one empty slot (`inv.slots.iter().any(|s| s.is_none())`).
    - If full, return `Err(PacketHandlerError::Custom("Inventory is full"))` *before* removing item from equipment.
    - If space exists, unequip from `EquipmentComponent` and add to `InventoryComponent`.
    - Return `Ok(Some(vec![2]))` on success.

### 2.2. Robust Non-Finite Combat Validation (`combat.rs`)
- In `HealthComponent`:
  - `take_damage(amount)`:
    - Reject if `self.is_dead || !amount.is_finite() || amount <= 0.0`.
  - `heal(amount)`:
    - Reject if `self.is_dead || !amount.is_finite() || amount <= 0.0`.
  - `respawn(initial_hp)`:
    - Protect against `f32::NAN.clamp(...)` returning `NaN`.
    - Implement:
      ```rust
      pub fn respawn(&mut self, initial_hp: f32) {
          self.is_dead = false;
          let safe_max = if self.max_hp.is_finite() && self.max_hp > 0.0 { self.max_hp } else { 100.0 };
          let hp = if initial_hp.is_finite() { initial_hp } else { safe_max };
          self.current_hp = hp.clamp(1.0, safe_max);
      }
      ```
  - `HealthComponent::new(max_hp, regen_per_sec)`:
    - Sanitize `max_hp` (default 100.0 if not finite or <= 0.0) and `regen_per_sec` (default 0.0 if not finite).
- In `process_damage_events` and `RpgCombatResolver`:
  - Ensure all calculated damages, bonus damages, and mitigation multipliers verify `.is_finite()` before being passed into entity health, attribute sets, or properties.

### 2.3. WASM Resource Limiter (`scripting/host_api.rs`, `scripting/runtime.rs`, `scripting/engine.rs`)
- Add `limiter: wasmtime::StoreLimits` to `HostContext`.
- Default limit: 32MB linear memory (`32 * 1024 * 1024` bytes, 512 wasm pages) and reasonable table/instance limits (`StoreLimitsBuilder::new().memory_size(32 * 1024 * 1024).build()`).
- In `WasmRuntime::execute_function_internal` (`crates/gameplay/src/scripting/runtime.rs`):
  - Invoke `store.limiter(|ctx| &mut ctx.limiter);` immediately after creating `Store::new(self.engine.engine(), host_ctx)`.
- This ensures any guest module calling `memory.grow` beyond 512 pages (32MB) receives a growth failure (`-1`) or trap instead of exhausting host RAM.

### 2.4. Trade Bilateral State Invalidation (`trade.rs`, `village.rs`)
- In `TradeManager::execute_atomic_swap`:
  - If validation fails for ANY reason (insufficient currency, item missing, weight exceeded, slot capacity exceeded, OR missing inventory components on either entity prior to validation), unconditionally clear:
    - `session.accepted_a = false;`
    - `session.accepted_b = false;`
    - `session.locked_a = false;`
    - `session.locked_b = false;`
- In `village.rs`:
  - Maintain session consistency and handle error propagation without leaving stale locks.

### 2.5. AI Threat Retargeting (`ai_brain.rs`)
- In `FsmBrainSystem::update`:
  - Under `FsmState::Chase { target }`:
    - When `current_threat_target` is `Some(new_target)` where `new_target != *target`:
      - Directly set `brain.current_state = FsmState::Chase { target: new_target };`
      - Update `set_nav_target = Some(Some(target_pos));`
  - Under `FsmState::Attack { target, cooldown_timer }`:
    - When `current_threat_target` is `Some(new_target)` where `new_target != *target`:
      - Directly transition to `FsmState::Chase { target: new_target };`
      - Update navigation target to the new threat leader.

## 3. Concurrency & Failure Modes
- **Disjoint Borrows in Trade**: Trade manager accesses entities disjointly via `EntityStore::get_two_mut`. State reset operates on `TradeSession` under exclusive mutable access, preventing data races.
- **Rollback in Inventory**: All inventory operations are locally atomic; failure branches never consume or drop items.
- **WASM Host Isolation**: The `ResourceLimiter` executes synchronously within the Wasmtime memory manager, bounded strictly per-instance and per-store.
