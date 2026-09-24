## Purpose

Defines requirements for the Loot Table, Quest System, and Guild Management plugins.

## ADDED Requirements

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
