use std::sync::Arc;
use tokio::sync::RwLock;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::octree::BoundingBox3D;
use mmo_gameplay::village::{
    bootstrap_village_world, InspectRequest, NpcBarterRequest, PlayerTradeOfferRequest,
    PlayerTradeSessionRequest, PlayerTradeStartRequest, VillageTradeStateResponse,
};
use mmo_gameplay::{InventoryComponent, ItemStack};
use mmo_node::network::NodeService;
use mmo_node::{NodeServiceImpl, TickSimulation};
use mmo_schema::cluster::ForwardClientPacketRequest;

#[tokio::test]
async fn test_village_bootstrap_and_modular_barter_integration() {
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let mut tick_sim = TickSimulation::new(1, bounds);

    // 1. Authoritatively bootstrap village world (structures, plugins, NPCs)
    bootstrap_village_world(&mut tick_sim);

    let sim = Arc::new(RwLock::new(tick_sim));
    let service = NodeServiceImpl::new(Arc::clone(&sim));

    let p1_guid = Guid::new(EntityType::Player, 1, 555);
    let ilmari_guid = Guid::new(EntityType::Npc, 1, 1);

    // 2. Incoming inspect packet from newly arriving player P1 triggers auto-hydration & starter kit hook
    let inspect_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p1_guid.raw(),
            target_guid: ilmari_guid.raw(),
            interaction_type: mmo_schema::client::InteractionType::Custom(1),
            action_id: 0x0002_0001, // module 0x02, sub-opcode 1
            payload: bytes::Bytes::from(
                serde_json::to_vec(&InspectRequest {
                    target_guid: Some(ilmari_guid.raw()),
                })
                .unwrap(),
            ),
        });

    let resp = service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 101,
            guid: p1_guid.raw(),
            packet_data: inspect_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap()
        .into_inner();

    assert!(resp.success, "Inspect failed: {}", resp.error_message);

    // 3. Verify P1 is auto-hydrated and provisioned with starter kit
    {
        let s = sim.read().await;
        assert!(s.entity_store.contains_guid(p1_guid));
        let ent = s.entity_store.get_by_guid(p1_guid).unwrap();
        let inv = ent
            .get_component::<InventoryComponent>()
            .expect("Starter inventory exists");
        assert_eq!(inv.currency, 50);
        assert!(inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 3, 2.0)));
        assert!(inv.has_item_stack(&ItemStack::new(1002, "Wild Herbs", 5, 0.5)));
        assert!(inv.has_item_stack(&ItemStack::new(1003, "Firewood", 2, 1.5)));

        // Verify broadcast response queue
        assert!(!s.pending_broadcast_jobs.is_empty());
        let job = &s.pending_broadcast_jobs[0];
        assert_eq!(job.session_id, 101);
        if let mmo_schema::client::ClientPacket::InteractionResult(ref res) = job.packets[0] {
            assert_eq!(res.action_id, 0x0002_0001);
            assert!(res.success);
            let state: VillageTradeStateResponse = serde_json::from_slice(&res.payload).unwrap();
            assert_eq!(
                state.target_merchant.as_ref().unwrap().name,
                "Blacksmith Ilmari"
            );
        } else {
            panic!("Expected InteractionResultPacket");
        }
    }

    // 4. Move P1 near Blacksmith Ilmari (-9.0, 0.0, -6.0) for barter
    {
        let mut s = sim.write().await;
        s.entity_store.get_mut_by_guid(p1_guid).unwrap().position = (-9.0, 0.0, -5.0);
        s.pending_broadcast_jobs.clear();
    }

    // 5. P1 executes NPC barter Recipe 1: 2 Iron Ore + 1 Firewood -> 1 Iron Pickaxe
    let barter_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p1_guid.raw(),
            target_guid: ilmari_guid.raw(),
            interaction_type: mmo_schema::client::InteractionType::Custom(2),
            action_id: 0x0002_0002, // module 0x02, sub-opcode 2
            payload: bytes::Bytes::from(
                serde_json::to_vec(&NpcBarterRequest {
                    target_guid: ilmari_guid.raw(),
                    recipe_id: 1,
                })
                .unwrap(),
            ),
        });

    let resp_barter = service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 101,
            guid: p1_guid.raw(),
            packet_data: barter_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap()
        .into_inner();

    assert!(
        resp_barter.success,
        "Barter failed: {}",
        resp_barter.error_message
    );

    // Verify P1 has crafted Iron Pickaxe
    {
        let s = sim.read().await;
        let ent = s.entity_store.get_by_guid(p1_guid).unwrap();
        let inv = ent.get_component::<InventoryComponent>().unwrap();
        assert!(inv.has_item_stack(&ItemStack::new(2001, "Iron Pickaxe", 1, 3.5)));
        assert_eq!(
            inv.slots
                .iter()
                .flatten()
                .find(|s| s.item_id == 1001)
                .unwrap()
                .quantity,
            1
        );
        assert_eq!(
            inv.slots
                .iter()
                .flatten()
                .find(|s| s.item_id == 1003)
                .unwrap()
                .quantity,
            1
        );
    }

    // 6. Player 2 arrives and performs a two-player atomic trade with Player 1
    let p2_guid = Guid::new(EntityType::Player, 1, 666);
    let p2_inspect_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p2_guid.raw(),
            target_guid: 0,
            interaction_type: mmo_schema::client::InteractionType::Custom(1),
            action_id: 0x0002_0001,
            payload: bytes::Bytes::from_static(b"{}"),
        });

    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 102,
            guid: p2_guid.raw(),
            packet_data: p2_inspect_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    // Move P2 near P1
    {
        let mut s = sim.write().await;
        s.entity_store.get_mut_by_guid(p2_guid).unwrap().position = (-8.0, 0.0, -5.0);
        s.pending_broadcast_jobs.clear();
    }

    // P1 starts trade with P2 (sub-opcode 3)
    let trade_start_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p1_guid.raw(),
            target_guid: p2_guid.raw(),
            interaction_type: mmo_schema::client::InteractionType::Custom(3),
            action_id: 0x0002_0003,
            payload: bytes::Bytes::from(
                serde_json::to_vec(&PlayerTradeStartRequest {
                    target_guid: p2_guid.raw(),
                })
                .unwrap(),
            ),
        });

    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 101,
            guid: p1_guid.raw(),
            packet_data: trade_start_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    let session_id = {
        let s = sim.read().await;
        let job = s.pending_broadcast_jobs.last().unwrap();
        if let mmo_schema::client::ClientPacket::InteractionResult(ref res) = job.packets[0] {
            let state: VillageTradeStateResponse = serde_json::from_slice(&res.payload).unwrap();
            state.active_trade.unwrap().session_id
        } else {
            panic!("Expected InteractionResultPacket");
        }
    };

    // P1 offers 1x Iron Pickaxe (sub-opcode 4)
    let offer_p1_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p1_guid.raw(),
            target_guid: 0,
            interaction_type: mmo_schema::client::InteractionType::Custom(4),
            action_id: 0x0002_0004,
            payload: bytes::Bytes::from(
                serde_json::to_vec(&PlayerTradeOfferRequest {
                    session_id,
                    items: vec![ItemStack::new(2001, "Iron Pickaxe", 1, 3.5)],
                    currency: 10,
                })
                .unwrap(),
            ),
        });
    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 101,
            guid: p1_guid.raw(),
            packet_data: offer_p1_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    // P2 offers 3x Wild Herbs and 15 Gold (sub-opcode 4)
    let offer_p2_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p2_guid.raw(),
            target_guid: 0,
            interaction_type: mmo_schema::client::InteractionType::Custom(4),
            action_id: 0x0002_0004,
            payload: bytes::Bytes::from(
                serde_json::to_vec(&PlayerTradeOfferRequest {
                    session_id,
                    items: vec![ItemStack::new(1002, "Wild Herbs", 3, 0.5)],
                    currency: 15,
                })
                .unwrap(),
            ),
        });
    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 102,
            guid: p2_guid.raw(),
            packet_data: offer_p2_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    // P1 locks offer (sub-opcode 5)
    let accept_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p1_guid.raw(),
            target_guid: 0,
            interaction_type: mmo_schema::client::InteractionType::Custom(5),
            action_id: 0x0002_0005,
            payload: bytes::Bytes::from(
                serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap(),
            ),
        });
    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 101,
            guid: p1_guid.raw(),
            packet_data: accept_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    // P2 locks and accepts (sub-opcode 5)
    let accept_p2_pkt =
        mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
            source_guid: p2_guid.raw(),
            target_guid: 0,
            interaction_type: mmo_schema::client::InteractionType::Custom(5),
            action_id: 0x0002_0005,
            payload: bytes::Bytes::from(
                serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap(),
            ),
        });
    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 102,
            guid: p2_guid.raw(),
            packet_data: accept_p2_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    // P1 final accept (sub-opcode 5) -> atomic swap!
    service
        .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
            session_id: 101,
            guid: p1_guid.raw(),
            packet_data: accept_pkt.encode(),
            session_epoch: 1,
        }))
        .await
        .unwrap();

    // 7. Verify atomic swap between P1 and P2
    {
        let s = sim.read().await;
        let p1_inv = s
            .entity_store
            .get_by_guid(p1_guid)
            .unwrap()
            .get_component::<InventoryComponent>()
            .unwrap();
        let p2_inv = s
            .entity_store
            .get_by_guid(p2_guid)
            .unwrap()
            .get_component::<InventoryComponent>()
            .unwrap();

        // P1 had 50 gold, sent 10, received 15 -> 55 gold. Lost pickaxe.
        assert_eq!(p1_inv.currency, 55);
        assert!(!p1_inv.has_item_stack(&ItemStack::new(2001, "Iron Pickaxe", 1, 3.5)));

        // P2 had 50 gold, sent 15, received 10 -> 45 gold. Received pickaxe.
        assert_eq!(p2_inv.currency, 45);
        assert!(p2_inv.has_item_stack(&ItemStack::new(2001, "Iron Pickaxe", 1, 3.5)));
    }
}
