//! End-to-end integration test validating the Cross-Node Social Bus:
//! Whispers, Global Chat, Cross-Node Party Management, and Guild State Synchronization.

use std::sync::Arc;

use mmo_core::guid::EntityType;
use mmo_core::{BoundingBox3D, Guid};
use mmo_gameplay::{
    CombatPlugin, GuildManager, GuildMemberComponent, GuildPlugin, GuildRank, InventoryComponent,
    InventoryPlugin, ItemStack, PartyManager, PartyMemberComponent, PartyPlugin, TradePlugin,
};
use mmo_node::sim::TickSimulation;
use mmo_storage::{
    ChatChannel, ChatMessage, GuildSyncAction, MemorySocialBus, PartyAction, SocialBus,
    SocialEnvelope,
};

#[tokio::test]
async fn test_cross_node_social_bus_and_party_sync() {
    // 1. Shared Cluster Social Bus
    let social_bus = Arc::new(MemorySocialBus::new());

    // 2. Initialize two separate simulation nodes representing different spatial octants
    let bounds1 = BoundingBox3D {
        min: (-1000.0, -1000.0, -1000.0),
        max: (0.0, 1000.0, 1000.0),
    };
    let bounds2 = BoundingBox3D {
        min: (0.0, -1000.0, -1000.0),
        max: (1000.0, 1000.0, 1000.0),
    };

    let mut node1 = TickSimulation::new(1, bounds1);
    let mut node2 = TickSimulation::new(2, bounds2);

    node1.add_plugin(Box::new(InventoryPlugin));
    node1.add_plugin(Box::new(CombatPlugin));
    node1.add_plugin(Box::new(PartyPlugin));
    node1.add_plugin(Box::new(GuildPlugin));
    node1.init_plugins();

    node2.add_plugin(Box::new(InventoryPlugin));
    node2.add_plugin(Box::new(CombatPlugin));
    node2.add_plugin(Box::new(PartyPlugin));
    node2.add_plugin(Box::new(GuildPlugin));
    node2.init_plugins();

    // Player 1 on Node 1, Player 2 on Node 2
    let p1_guid = Guid::new(EntityType::Player, 1, 101);
    let p2_guid = Guid::new(EntityType::Player, 2, 202);

    assert!(node1.spawn_entity(p1_guid, (-10.0, 0.0, -10.0)));
    assert!(node2.spawn_entity(p2_guid, (10.0, 0.0, 10.0)));

    // 3. Test Cross-Node Whisper Delivery
    let p2_whisper_channel = format!("whisper:{}", p2_guid.raw());
    let mut p2_whisper_rx = social_bus.subscribe(&p2_whisper_channel).await.unwrap();

    let whisper = ChatMessage {
        channel: ChatChannel::Whisper,
        sender_guid: p1_guid.raw(),
        sender_name: "Player1".into(),
        target_guid: Some(p2_guid.raw()),
        content: "Want to team up for the dungeon?".into(),
        timestamp: 100,
    };

    // Node 1 emits whisper to social bus
    social_bus
        .publish(&p2_whisper_channel, &SocialEnvelope::Chat(whisper.clone()))
        .await
        .unwrap();

    let received_whisper = p2_whisper_rx.recv().await.unwrap();
    assert_eq!(received_whisper, SocialEnvelope::Chat(whisper));

    // 4. Test Cross-Node Party Formation & Coordination
    let mut party_events_rx = social_bus.subscribe("party:events").await.unwrap();

    let mut party_mgr_node1 = PartyManager::new();
    let mut party_mgr_node2 = PartyManager::new();

    // P1 (Node 1) creates party
    let party_id = party_mgr_node1.create_party(p1_guid).unwrap();

    // P1 attaches PartyMemberComponent on Node 1
    {
        let p1_entity = node1.entity_store.get_mut_by_guid(p1_guid).unwrap();
        p1_entity.add_component(PartyMemberComponent::new(party_id, true));
    }

    // P1 invites P2 over the Social Bus
    let invite_action = PartyAction::Invite {
        party_id,
        inviter_guid: p1_guid.raw(),
        invitee_guid: p2_guid.raw(),
    };
    social_bus
        .publish("party:events", &SocialEnvelope::Party(invite_action))
        .await
        .unwrap();

    // Node 2 receives party invite
    let received_action = party_events_rx.recv().await.unwrap();
    match received_action {
        SocialEnvelope::Party(PartyAction::Invite { party_id: p_id, inviter_guid, invitee_guid }) => {
            assert_eq!(p_id, party_id);
            assert_eq!(inviter_guid, p1_guid.raw());
            assert_eq!(invitee_guid, p2_guid.raw());

            // P2 accepts and joins on Node 2
            let p2_entity = node2.entity_store.get_mut_by_guid(p2_guid).unwrap();
            p2_entity.add_component(PartyMemberComponent::new(party_id, false));

            // Publish acceptance back to the cluster
            let accept_action = PartyAction::Accept {
                party_id,
                member_guid: p2_guid.raw(),
            };
            social_bus
                .publish("party:events", &SocialEnvelope::Party(accept_action))
                .await
                .unwrap();
        }
        _ => panic!("Expected PartyAction::Invite"),
    }

    // Node 1 receives acceptance
    let accept_received = party_events_rx.recv().await.unwrap();
    match accept_received {
        SocialEnvelope::Party(PartyAction::Accept { party_id: p_id, member_guid }) => {
            assert_eq!(p_id, party_id);
            assert_eq!(member_guid, p2_guid.raw());
        }
        _ => panic!("Expected PartyAction::Accept"),
    }

    // Both players now belong to the same party across node boundaries
    let p1_party = node1
        .entity_store
        .get_by_guid(p1_guid)
        .unwrap()
        .get_component::<PartyMemberComponent>()
        .unwrap();
    let p2_party = node2
        .entity_store
        .get_by_guid(p2_guid)
        .unwrap()
        .get_component::<PartyMemberComponent>()
        .unwrap();

    assert_eq!(p1_party.party_id, p2_party.party_id);
    assert!(p1_party.is_leader);
    assert!(!p2_party.is_leader);

    // 5. Test Cross-Node Guild Vault Synchronization
    let guild_id = 999;
    let guild_sync_channel = format!("guild:sync:{}", guild_id);
    let mut guild_rx = social_bus.subscribe(&guild_sync_channel).await.unwrap();

    // P1 (Node 1) deposits an item into the guild vault and broadcasts sync
    let sync_event = GuildSyncAction::VaultUpdated {
        guild_id,
        actor_guid: p1_guid.raw(),
    };
    social_bus
        .publish(&guild_sync_channel, &SocialEnvelope::Guild(sync_event.clone()))
        .await
        .unwrap();

    // Node 2 receives guild vault update notification
    let received_guild_event = guild_rx.recv().await.unwrap();
    assert_eq!(received_guild_event, SocialEnvelope::Guild(sync_event));
}
