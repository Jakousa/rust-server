# Content Gameplay Plugins Specification

## Purpose

Defines requirements for the Loot Table, Quest System, and Guild Management plugins.

## Requirements

### Requirement: Weighted Probability Loot Tables
The loot system SHALL evaluate probability-weighted drop tables upon entity death or chest opening, generating randomized item stacks with configurable chance (0.0 to 1.0) and quantity ranges.

#### Scenario: Slaying a monster generates loot
- **WHEN** an entity with a linked loot table dies
- **THEN** the system SHALL roll each loot entry's probability and populate a `LootContainerComponent` with winning item stacks

### Requirement: Event-Driven Quest Progression
The quest system SHALL maintain a `QuestLogComponent` on player entities and automatically advance quest objectives in response to domain events without direct coupling to combat or inventory systems.

#### Scenario: Monster kill objective progression
- **WHEN** a player with an active kill objective slays the targeted monster type
- **THEN** the system SHALL increment the objective counter and mark the quest ready for turn-in when all objectives are complete

#### Scenario: Quest turn-in and reward distribution
- **WHEN** a player turns in a completed quest
- **THEN** the system SHALL award experience/currency and deposit quest reward item stacks into the player's inventory

### Requirement: Guild Hierarchy and Transactional Vault Bank
The guild system SHALL track member rosters, officer permissions, and provide a shared `GuildVault` bank that atomically deposits and withdraws item stacks to and from member inventories.

#### Scenario: Depositing item into guild vault
- **WHEN** an authorized guild member deposits an item stack into the guild vault
- **THEN** the system SHALL remove the item from the player's inventory and store it in the guild vault within a single transaction

---

### Requirement: Transactional Equipment Swap and Unequip
The inventory and equipment system SHALL guarantee atomic transactions for equipment swapping and unequipping without silent item deletion.

#### Scenario: Swapping equipped item into full inventory preserves unequipped item
- **GIVEN** an entity with a full inventory containing item `A` in slot 0 and item `B` equipped in `MainHand`
- **WHEN** the player requests to equip item `A` into `MainHand` via sub-opcode 1
- **THEN** item `A` is equipped in `MainHand`
- **AND** item `B` is safely deposited into the vacated slot 0 without item loss
- **AND** if storing item `B` fails for any reason, the entire transaction rolls back atomically.

#### Scenario: Unequipping into full inventory is rejected without item deletion
- **GIVEN** an entity with an equipped item in `Chest` and an inventory with 0 empty slots
- **WHEN** the player requests to unequip `Chest` via sub-opcode 2
- **THEN** the handler returns an error
- **AND** the item remains securely equipped in `Chest` without being discarded.

---

### Requirement: Non-Finite Combat Float Rejection
The combat system SHALL reject non-finite (`NaN`, `Infinity`, `-Infinity`) values in damage, healing, respawn, and mitigation formulas to prevent immortal or zombie entity states.

#### Scenario: Non-finite damage rejection
- **WHEN** damage containing `NaN` or `Infinity` is applied to `HealthComponent::take_damage`
- **THEN** the damage is rejected, `current_hp` remains finite, and `is_dead` remains unaffected.

---

### Requirement: Bilateral Trade Acceptance and Lock Reset
The trade system SHALL reset both participants' acceptance and lock flags to `false` upon any atomic swap validation or precondition failure.

#### Scenario: Swap failure invalidates bilateral consent
- **WHEN** `execute_atomic_swap` fails validation due to capacity, weight, currency, or missing components
- **THEN** `accepted_a`, `accepted_b`, `locked_a`, and `locked_b` are all reset to `false`
- **AND** unilateral finalization is prevented until both parties re-lock and re-accept.
