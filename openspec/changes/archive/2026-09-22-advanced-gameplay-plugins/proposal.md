## Why

With the foundational `GameplayPlugin` framework, `ComponentCodecRegistry`, `EventQueue`, and `ModularPacketRouter` in place, we need to implement the core progression and social content loops of an MMO:
1. **Loot Tables & Drops (`LootPlugin`)**: Monster and container drop tables with probability weighting, roll guarantees, and item instantiation.
2. **Quest System (`QuestPlugin`)**: Structured quest logs, objective tracking (monster slaying, item fetching, area exploration), and reward distribution.
3. **Guilds & Social Organizations (`GuildPlugin`)**: Guild memberships, rank hierarchies, officer permissions, and transactional shared guild bank vaults.

Implementing these three systems as standalone plugins in `crates/gameplay` proves that complex, interconnected MMO game loops can be built cleanly using our decoupled architecture.

## What Changes

- Implement `LootPlugin` in `crates/gameplay/src/loot.rs`:
  - `LootTable`, `LootEntry`, and probability roll algorithms.
  - Integration with `EntityDiedEvent` to generate loot rolls on monster death.
  - `LootContainerComponent` allowing players to loot dropped corpses or chests within distance.
- Implement `QuestPlugin` in `crates/gameplay/src/quest.rs`:
  - `QuestDefinition`, `QuestObjective` (Kill, Fetch, Interact), and `QuestReward`.
  - `QuestLogComponent` with active/completed quest states and progress counters.
  - Event listeners incrementing kill counts on `EntityDiedEvent` and fetch counts on inventory changes.
- Implement `GuildPlugin` in `crates/gameplay/src/guild.rs`:
  - `GuildMemberComponent` holding guild ID and rank (`Member`, `Officer`, `Leader`).
  - `GuildState` with roster, member ranks, motd, and shared `GuildVault` bank.
  - Transaction-safe deposit and withdraw operations between player inventory and guild vault.
- Register all three plugins in `crates/gameplay/src/lib.rs`.
- Add unit tests for each plugin and an end-to-end integration test validating the complete quest -> monster kill -> loot drop -> guild vault deposit gameplay loop.

## Capabilities

### New Capabilities
- `gameplay/loot`: Provides weighted probability loot tables and lootable container components.
- `gameplay/quest`: Provides quest definitions, quest log components, event-driven objective tracking, and completion rewards.
- `gameplay/guild`: Provides guild rosters, permission hierarchies, and shared transactional guild bank vaults.

## Impact

- Extends `crates/gameplay` without requiring any changes to `crates/core`, `crates/node`, or `crates/storage`.
- Provides full end-to-end MMO gameplay loops: combat yields loot, triggers quest progress, and rewards items that can be stored in player bags or shared in guild vaults.
