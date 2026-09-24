# Advanced Gameplay Plugins Design (Loot, Quests, Guilds)

## Architecture Overview

This design implements three essential MMO systems as standalone plugins in `crates/gameplay`:
1. **`LootPlugin`**: Drop table probability rolls, loot containers, corpse looting.
2. **`QuestPlugin`**: Quest definitions, objective state tracking via `QuestLogComponent`, event-driven progression.
3. **`GuildPlugin`**: Guild state, membership ranks, and shared transactional `GuildVault` bank.

```
                                 Domain Events / Actions
                                           |
      +------------------------------------+-----------------------------------+
      |                                    |                                   |
      v                                    v                                   v
+------------------------+   +---------------------------+   +-------------------------------+
|      LootPlugin        |   |        QuestPlugin        |   |          GuildPlugin          |
| - LootTable            |   | - QuestLogComponent       |   | - GuildMemberComponent        |
| - LootEntry (Chance)   |   | - Objective: Kill / Slay  |   | - Rank: Member/Officer/Leader |
| - LootContainerComponent|  | - Objective: Gather/Fetch |   | - GuildVault: Shared Bank     |
| - Corpse Loot Action   |   | - QuestCompletedEvent     |   | - Transactional Deposit/Draw  |
+------------------------+   +---------------------------+   +-------------------------------+
      |                                    |                                   |
      +------------------------------------+-----------------------------------+
                                           |
                                           v
                       +---------------------------------------+
                       |             crates/core               |
                       | - EventQueue    - ComponentStore      |
                       | - EntityStore   - CodecRegistry       |
                       +---------------------------------------+
```

---

## 1. Loot Tables Plugin (`LootPlugin`)

### Data Structures
- `LootEntry`:
  ```rust
  pub struct LootEntry {
      pub item_id: u32,
      pub name: String,
      pub drop_chance: f32, // 0.0 to 1.0 (e.g. 0.25 = 25%)
      pub min_count: u32,
      pub max_count: u32,
      pub unit_weight: f32,
  }
  ```
- `LootTable`:
  ```rust
  pub struct LootTable {
      pub table_id: u32,
      pub entries: Vec<LootEntry>,
  }
  ```
- `LootContainerComponent`:
  Attached to monster corpses or world chests. Holds rolled item stacks waiting to be looted by players within interaction distance.

### Behavior
- When an entity dies, `roll_loot(&loot_table, rng_seed)` rolls probabilities and attaches a `LootContainerComponent` containing the generated items.
- Players interact via `loot_all(player_entity, container)`: items are transferred directly into `player.get_component_mut::<InventoryComponent>()`.

---

## 2. Quest System Plugin (`QuestPlugin`)

### Data Structures
- `QuestObjective`:
  ```rust
  pub enum QuestObjective {
      KillMonster { monster_type_id: u32, required: u32, current: u32 },
      FetchItem { item_id: u32, required: u32, current: u32 },
  }
  ```
- `QuestStatus`: `NotStarted`, `InProgress`, `ReadyForTurnIn`, `Completed`.
- `QuestRecord`: Tracks individual quest ID, objective progress, and status.
- `QuestLogComponent`:
  ```rust
  pub struct QuestLogComponent {
      pub quests: HashMap<u32, QuestRecord>,
  }
  ```

### Behavior
- Listens to `EntityDiedEvent`: If killer has `QuestLogComponent`, increments matching `KillMonster` objective counters.
- When all objectives meet requirement, status transitions to `ReadyForTurnIn`.
- Turning in quest distributes XP/currency and inserts reward `ItemStack`s into `InventoryComponent`.

---

## 3. Guilds System Plugin (`GuildPlugin`)

### Data Structures
- `GuildRank`: `Member = 1`, `Officer = 2`, `Leader = 3`.
- `GuildMemberComponent`:
  ```rust
  pub struct GuildMemberComponent {
      pub guild_id: u64,
      pub rank: GuildRank,
  }
  ```
- `GuildVault`:
  Shared item bank container with slot limits and transaction safeguards.
- `GuildState`:
  ```rust
  pub struct GuildState {
      pub guild_id: u64,
      pub name: String,
      pub leader_guid: Guid,
      pub members: HashMap<Guid, GuildRank>,
      pub vault: GuildVault,
  }
  ```

### Behavior
- `deposit_to_vault(player_entity, guild, item_slot)`: Atomically extracts item from player's `InventoryComponent` and deposits it into `guild.vault`.
- `withdraw_from_vault(player_entity, guild, vault_slot)`: Checks rank permissions (e.g. Officer or Leader), extracts item from vault, and inserts into player's inventory.
