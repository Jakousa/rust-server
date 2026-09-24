## 1. Loot Tables Plugin (`crates/gameplay/src/loot.rs`)

- [x] 1.1 Implement `LootEntry`, `LootTable`, and probability roll engine generating `ItemStack`s
- [x] 1.2 Implement `LootContainerComponent` holding dropped items and player looting transfer logic into `InventoryComponent`
- [x] 1.3 Implement `LootPlugin` registering component codecs and test looting with unit tests

## 2. Quest System Plugin (`crates/gameplay/src/quest.rs`)

- [x] 2.1 Implement `QuestDefinition`, `QuestObjective`, and `QuestLogComponent` data models
- [x] 2.2 Implement event-driven quest advancement reacting to `EntityDiedEvent` and inventory turn-in rewards
- [x] 2.3 Implement `QuestPlugin` registering component codecs and test quest lifecycle with unit tests

## 3. Guilds Management Plugin (`crates/gameplay/src/guild.rs`)

- [x] 3.1 Implement `GuildMemberComponent`, `GuildRank`, and `GuildState` with roster management
- [x] 3.2 Implement `GuildVault` bank container with atomic deposit and permission-checked withdrawal logic
- [x] 3.3 Implement `GuildPlugin` registering component codecs and test guild operations with unit tests

## 4. Integration & System Review

- [x] 4.1 Export all three plugins in `crates/gameplay/src/lib.rs` and verify workspace compiles
- [x] 4.2 Create comprehensive integration test connecting quests, monster kills, loot drops, and guild vault deposit
