# Specification: Gameplay Systems Resilience, State Consistency, and WASM Memory Limits

## Feature: Equipment Swap & Unequip Transactional Safety

### Scenario: Swapping equipped item into full inventory preserves unequipped item
- **GIVEN** an entity with a full inventory containing item `A` in slot 0 and item `B` equipped in `MainHand`
- **WHEN** the player requests to equip item `A` into `MainHand` via sub-opcode 1
- **THEN** item `A` is equipped in `MainHand`
- **AND** item `B` is safely deposited into the vacated slot 0 without item destruction or loss
- **AND** the handler returns `Ok(Some(vec![1]))`
- **AND** if storing item `B` fails for any reason, the entire transaction rolls back such that item `A` remains in slot 0 and item `B` remains equipped.

### Scenario: Unequipping into full inventory is rejected without item deletion
- **GIVEN** an entity with an equipped item in `Chest` and an inventory with 0 empty slots
- **WHEN** the player requests to unequip `Chest` via sub-opcode 2
- **THEN** the handler returns `Err(PacketHandlerError::Custom("Inventory is full"))`
- **AND** the item remains securely equipped in `Chest` without being discarded.

### Scenario: Unequipping from empty slot returns error
- **GIVEN** an entity with no item equipped in `Head`
- **WHEN** the player requests to unequip `Head` via sub-opcode 2
- **THEN** the handler returns `Err(PacketHandlerError::Custom("No item equipped in specified slot"))`.

### Scenario: Successful unequip into inventory
- **GIVEN** an entity with an item equipped in `Boots` and at least one free inventory slot
- **WHEN** the player requests to unequip `Boots` via sub-opcode 2
- **THEN** the item is removed from `Boots` and added to inventory
- **AND** the handler returns `Ok(Some(vec![2]))`.

---

## Feature: Non-Finite Combat Damage and Healing Rejection

### Scenario: NaN damage does not alter entity health or make entity immortal
- **GIVEN** an entity with `HealthComponent` having 100.0 / 100.0 HP and `is_dead = false`
- **WHEN** `take_damage` is called with `f32::NAN`, `f32::INFINITY`, or negative damage
- **THEN** `take_damage` returns `false`
- **AND** `current_hp` remains 100.0
- **AND** `is_dead` remains `false`.

### Scenario: Subsequent valid lethal damage kills entity normally after NaN attempt
- **GIVEN** an entity that received a NaN damage attempt
- **WHEN** `take_damage(100.0)` is executed
- **THEN** `take_damage` returns `true`
- **AND** `current_hp` is 0.0
- **AND** `is_dead` is `true`.

### Scenario: NaN heal and respawn values are sanitized
- **GIVEN** an entity with 50.0 / 100.0 HP
- **WHEN** `heal(f32::NAN)` or `respawn(f32::NAN)` is executed
- **THEN** the entity's health remains valid and finite without transitioning to NaN.

---

## Feature: WASM Guest Memory Resource Limiter

### Scenario: Guest script memory growth is strictly bounded to 32MB
- **GIVEN** a WASM script executing via `WasmRuntime::execute_function_internal` with fuel metering
- **WHEN** the guest script executes `memory.grow` requesting pages within 32MB (512 pages * 64KiB)
- **THEN** the allocation succeeds
- **WHEN** the guest script executes `memory.grow` exceeding 32MB (e.g. 513 pages or 4GB)
- **THEN** the growth fails (`memory.grow` returns -1 or execution errors)
- **AND** the host node process remains healthy without OOM killing.

---

## Feature: Trade Bilateral Acceptance and Lock Reset on Validation Failure

### Scenario: Failed trade atomic swap resets acceptance and locks for both players
- **GIVEN** an active trade session where Player A and Player B have locked and accepted
- **WHEN** `execute_atomic_swap` fails validation due to insufficient inventory slots, currency, weight, or missing inventory components
- **THEN** `execute_atomic_swap` returns an error
- **AND** `accepted_a`, `accepted_b`, `locked_a`, and `locked_b` are all reset to `false`
- **AND** neither party can complete the trade until both re-lock and re-accept.

---

## Feature: AI FSM Threat Retargeting

### Scenario: Mob FSM switches target directly upon threat table leadership change
- **GIVEN** an NPC entity in `FsmState::Chase { target: player_1 }` or `FsmState::Attack { target: player_1 }`
- **WHEN** player_2 surpasses player_1 as the highest threat in `ThreatTableComponent`
- **THEN** the FSM transitions directly to `FsmState::Chase { target: player_2 }`
- **AND** the navigation target is updated to player_2's current position
- **AND** the mob does not enter `FsmState::Evade` unless all threat is eliminated.
