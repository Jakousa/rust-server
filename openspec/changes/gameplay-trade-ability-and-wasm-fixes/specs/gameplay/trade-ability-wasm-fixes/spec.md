# Gameplay Trade, Ability, Guild, WASM Host & Plugin Wiring Specification

## Purpose
Ensures strict conservation of items and currency during player trades, pre-validated ability resource/cooldown deduction, single-guild membership invariants, synchronized spatial/combat state in WASM host calls, and active encumbrance and damage processing in gameplay plugins.

## Requirements

### Requirement: Conservation of Items and Currency in Player-to-Player Trades
`TradeManager::execute_atomic_swap` SHALL verify that both participants own all offered items and currency, verify post-swap slot and weight limits, remove offered items from giver inventories, add them to recipient inventories, and transfer currency balances atomically.

#### Scenario: Successful atomic item and currency swap
- **GIVEN** Player A has a `"Sword"` and `100` currency, and Player B has a `"Shield"` and `0` currency
- **WHEN** Player A offers `"Sword"` + `50` currency for Player B's `"Shield"` and both accept and confirm
- **THEN** Player A's inventory SHALL contain only `"Shield"` and `50` currency, and Player B's inventory SHALL contain only `"Sword"` and `50` currency

#### Scenario: Trade rejected when offered item was removed before confirmation
- **GIVEN** an active trade session where Player A offered `"Sword"`
- **WHEN** `"Sword"` is removed from Player A's inventory prior to `confirm_trade`
- **THEN** `execute_atomic_swap` SHALL fail and leave both inventories unchanged

### Requirement: Pre-Validated Ability Range and ActionQueue Availability
`execute_or_start_ability` and `execute_or_start_ability_with_collision` SHALL validate target range and `ActionQueue` availability before deducting resources or starting cooldowns.

#### Scenario: Caster is already busy casting when activating a second ability
- **GIVEN** a caster with `100.0` mana currently executing a non-interruptible `TimedAction` in `ActionQueue`
- **WHEN** `execute_or_start_ability` is invoked for a casted ability costing `25.0` mana
- **THEN** the call SHALL return an error, the caster's mana SHALL remain `100.0`, and the ability cooldown SHALL NOT be started

#### Scenario: Target is outside ability range
- **GIVEN** a caster at `(0.0, 0.0, 0.0)` and a target at `(50.0, 0.0, 0.0)`
- **WHEN** `execute_or_start_ability` is invoked with an ability whose `range` is `20.0`
- **THEN** the call SHALL return an out-of-range error without deducting mana or starting cooldowns

### Requirement: Single-Guild Membership Enforcement
`GuildManager` SHALL prevent a player who is already a member of a guild from joining or creating another guild until they leave their current guild.

#### Scenario: Adding a player who is already in a guild
- **GIVEN** a player who is already a member of Guild 1
- **WHEN** `add_member` is called to add that player to Guild 2
- **THEN** `add_member` SHALL return `Err("Player already in a guild")` and preserve the player's membership in Guild 1

### Requirement: Synchronized WASM Host Mutations and Active Gameplay Plugin Systems
WASM host function `entity_set_pos` SHALL reject non-finite coordinates (`NaN` or `Infinity`) and synchronize the `Octree`. `entity_modify_health` SHALL synchronize `AttributeSet` `"health"` and emit `EntityDiedEvent` upon lethal damage. `InventoryPlugin` SHALL cancel `Event::BeforeMove` when encumbered, and `CombatPlugin` SHALL process queued `DamageEvent`s on every tick.

#### Scenario: WASM script moves entity and inflicts lethal damage
- **GIVEN** a WASM tick script invoking `entity_set_pos` with finite coordinates and `entity_modify_health` with lethal negative delta
- **WHEN** `ScriptTickSystem::update` executes the script
- **THEN** the entity's new position SHALL be indexed in `Octree`, its `AttributeSet` `"health"` SHALL be `0.0`, and an `EntityDiedEvent` SHALL be queued in `EventQueue`
