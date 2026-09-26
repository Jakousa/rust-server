//! End-to-end integration test validating the complete MMO content loop:
//! Quests, Combat, Loot Drops, Player Bag Management, and Guild Vault Storage.

use mmo_core::guid::EntityType;
use mmo_core::{BoundingBox3D, Entity, Guid};
use mmo_gameplay::{
    process_damage_events, CombatPlugin, DamageEvent, GuildManager, GuildMemberComponent,
    GuildPlugin, GuildRank, HealthComponent, InventoryComponent, InventoryPlugin,
    LootContainerComponent, LootEntry, LootPlugin, LootTable, QuestLogComponent, QuestObjective,
    QuestPlugin, QuestRecord, QuestStatus, StatsComponent, TradePlugin,
};
use mmo_node::sim::TickSimulation;
use mmo_node::snapshotter::WriteBehindSnapshotter;
use mmo_storage::repository::CharacterRepository;
use mmo_storage::Database;

#[tokio::test]
async fn test_full_mmo_content_loop_integration() {
    let bounds = BoundingBox3D {
        min: (-1000.0, -1000.0, -1000.0),
        max: (1000.0, 1000.0, 1000.0),
    };
    let mut sim = TickSimulation::new(1, bounds);

    // 1. Register all 6 decoupled gameplay plugins into simulation node
    sim.add_plugin(Box::new(InventoryPlugin));
    sim.add_plugin(Box::new(CombatPlugin));
    sim.add_plugin(Box::new(TradePlugin));
    sim.add_plugin(Box::new(LootPlugin));
    sim.add_plugin(Box::new(QuestPlugin));
    sim.add_plugin(Box::new(GuildPlugin));
    sim.init_plugins();

    // 2. Set up Guild
    let leader_guid = Guid::new(EntityType::Player, 1, 10);
    let player_guid = Guid::new(EntityType::Player, 1, 20);

    let mut guild_mgr = GuildManager::new();
    let guild_id = guild_mgr.create_guild("Dragon Slayers", leader_guid, 10);
    guild_mgr
        .add_member(guild_id, player_guid, GuildRank::Officer)
        .unwrap();

    // 3. Spawn Player entity with components
    assert!(sim.spawn_entity(player_guid, (0.0, 0.0, 0.0)));
    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        player.add_component(InventoryComponent::new(6, 100.0));
        player.add_component(HealthComponent::new(100.0, 5.0));
        player.add_component(StatsComponent::new(30, 50));
        player.add_component(GuildMemberComponent::new(guild_id, GuildRank::Officer));

        let mut quest_log = QuestLogComponent::new();
        let wolf_quest = QuestRecord::new(
            1,
            "Wolf Menace",
            vec![QuestObjective::KillMonster {
                monster_type_id: 2, // Npc
                required: 1,
                current: 0,
            }],
            vec![mmo_gameplay::ItemStack::new(99, "Wolf Fang Trophy", 1, 0.5)],
            500, // 500 XP
        );
        quest_log.accept_quest(wolf_quest).unwrap();
        player.add_component(quest_log);
    }

    // 4. Spawn Monster NPC (Wolf) with Loot Table
    let monster_guid = Guid::new(EntityType::Npc, 1, 500);
    assert!(sim.spawn_entity(monster_guid, (2.0, 0.0, 2.0)));
    {
        let monster = sim.entity_store.get_mut_by_guid(monster_guid).unwrap();
        monster.add_component(HealthComponent::new(40.0, 0.0));
        monster.add_component(StatsComponent::new(5, 0));
    }

    let mut wolf_loot_table = LootTable::new(1);
    wolf_loot_table.add_entry(LootEntry::new(101, "Wolf Pelt", 1.0, 1, 2, 1.0));
    wolf_loot_table.add_entry(LootEntry::new(102, "Wolf Meat", 1.0, 2, 3, 0.5));

    // 5. Combat: Player attacks and kills Wolf
    sim.event_queue.push(DamageEvent {
        source: player_guid,
        target: monster_guid,
        raw_damage: 50.0, // 50 > 40 HP = fatal
    });

    let deaths = process_damage_events(&mut sim.event_queue, &mut sim.entity_store);
    assert_eq!(deaths.len(), 1);
    assert_eq!(deaths[0].entity, monster_guid);
    assert_eq!(deaths[0].killer, Some(player_guid));

    // 6. Loot Generation: Monster drops items into a LootContainerComponent
    let drops = wolf_loot_table.roll_loot(12345);
    assert_eq!(drops.len(), 2);
    {
        let monster = sim.entity_store.get_mut_by_guid(monster_guid).unwrap();
        monster.add_component(LootContainerComponent::new(drops));
    }

    // 7. Quest Progression: Player quest log advances kill count
    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        let quest_log = player.get_component_mut::<QuestLogComponent>().unwrap();
        let ready_quests = quest_log.on_monster_killed(2);
        assert_eq!(ready_quests, vec![1]);
        assert_eq!(
            quest_log.active_quests.get(&1).unwrap().status,
            QuestStatus::ReadyForTurnIn
        );
    }

    // 8. Player loots the monster corpse
    {
        let mut monster_loot = {
            let monster = sim.entity_store.get_mut_by_guid(monster_guid).unwrap();
            monster
                .remove_component::<LootContainerComponent>()
                .unwrap()
        };

        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        let player_inv = player.get_component_mut::<InventoryComponent>().unwrap();
        let looted_count = monster_loot.loot_all(player_inv);
        assert_eq!(looted_count, 2);
        assert_eq!(player_inv.slots[0].as_ref().unwrap().item_id, 101);
        assert_eq!(player_inv.slots[1].as_ref().unwrap().item_id, 102);
    }

    // 9. Player turns in quest: receives "Wolf Fang Trophy" in inventory slot 2
    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        let mut quest_log = player.remove_component::<QuestLogComponent>().unwrap();
        let player_inv = player.get_component_mut::<InventoryComponent>().unwrap();

        let xp_reward = quest_log.turn_in_quest(1, player_inv).unwrap();
        assert_eq!(xp_reward, 500);
        assert_eq!(player_inv.slots[2].as_ref().unwrap().item_id, 99); // Wolf Fang Trophy!
        player.add_component(*quest_log);
    }

    // 10. Guild Vault Deposit: Officer deposits the "Wolf Fang Trophy" into guild bank
    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        assert!(guild_mgr.deposit_item(player_guid, 2, player).is_ok());
    }

    let guild = guild_mgr.get_guild(guild_id).unwrap();
    assert_eq!(
        guild.vault.slots[0].as_ref().unwrap().name,
        "Wolf Fang Trophy"
    );

    // 11. Persistence: Verify full component snapshot serialization and recovery
    let db = Database::connect_memory().await.unwrap();
    let repo = CharacterRepository::new(db.clone());

    let player_entity = sim.entity_store.get_by_guid(player_guid).unwrap();
    let snapshot = WriteBehindSnapshotter::capture_entity_snapshot(player_entity, &sim.codecs, 1);

    // Verify snapshot includes all dynamically attached components
    assert!(snapshot.attributes.get("inventory").is_some());
    assert!(snapshot.attributes.get("health").is_some());
    assert!(snapshot.attributes.get("stats").is_some());
    assert!(snapshot.attributes.get("quest_log").is_some());
    assert!(snapshot.attributes.get("guild_member").is_some());

    repo.save_snapshot(&snapshot).await.unwrap();

    // Restore into a fresh entity
    let mut restored_player = Entity::new(Guid(snapshot.guid), snapshot.position);
    let restored = WriteBehindSnapshotter::restore_entity_components(
        &mut restored_player,
        &snapshot,
        &sim.codecs,
    )
    .unwrap();
    assert_eq!(restored, 5); // inventory, health, stats, quest_log, guild_member

    let restored_quest_log = restored_player
        .get_component::<QuestLogComponent>()
        .unwrap();
    assert!(restored_quest_log.completed_quests.contains(&1));
    let restored_guild = restored_player
        .get_component::<GuildMemberComponent>()
        .unwrap();
    assert_eq!(restored_guild.guild_id, guild_id);
    assert_eq!(restored_guild.rank, GuildRank::Officer);
}
