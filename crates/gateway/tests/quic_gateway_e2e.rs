//! End-to-End QUIC Gateway Transport Integration Test
//!
//! Verifies:
//! 1. QUIC TLS 1.3 handshake negotiation with self-signed certificate.
//! 2. Unreliable datagram transmission for high-frequency player movement without head-of-line blocking.
//! 3. Reliable stream transmission for player interactions and transactional actions.
//! 4. Latency measurement ping/pong request-response over streams.
//! 5. Downstream AoI entity state broadcasts pushed to clients via QUIC datagrams.
//! 6. Downstream transactional results pushed to clients via reliable streams.
//! 7. Multi-client concurrent connections and broadcast fan-out.
//! 8. Clean session sink cleanup upon client disconnection.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

use mmo_core::guid::{EntityType, Guid};
use mmo_gateway::quic::QuicClientSession;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_schema::client::{
    ClientPacket, EntityDespawnPacket, EntityStatePacket, InteractionPacket, InteractionResultPacket,
    InteractionType, MovementPacket, PingPacket, Vec3,
};

#[tokio::test]
async fn test_quic_gateway_e2e_full_lifecycle() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);
    let server = Arc::new(GatewayServer::with_packet_channel(router.clone(), packet_tx));

    let session1 = 1001;
    let player1_guid = Guid::new(EntityType::Player, 1, 1001);
    let node_addr = "127.0.0.1:50051".to_string();
    server
        .register_session(session1, player1_guid, node_addr.clone())
        .expect("register session 1");

    let session2 = 1002;
    let player2_guid = Guid::new(EntityType::Player, 1, 1002);
    server
        .register_session(session2, player2_guid, node_addr.clone())
        .expect("register session 2");

    // Bind QUIC server endpoint
    let quic_endpoint = server
        .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
        .expect("bind quic server");
    let local_addr = quic_endpoint.local_addr().expect("local addr");

    // Spawn server listener loop
    let srv = server.clone();
    let srv_endpoint = quic_endpoint.clone();
    let listener_handle = tokio::spawn(async move {
        let mut session_ids = vec![session1, session2].into_iter();
        while let Some(incoming) = srv_endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                if let Some(sid) = session_ids.next() {
                    let s = srv.clone();
                    tokio::spawn(async move {
                        let _ = s.handle_quic_connection(conn, sid).await;
                    });
                }
            }
        }
    });

    // 1. Connect Client 1
    let mut client1 = QuicClientSession::connect(local_addr, "localhost")
        .await
        .expect("client 1 connect");

    // Connect Client 2
    let client2 = QuicClientSession::connect(local_addr, "localhost")
        .await
        .expect("client 2 connect");

    // Wait for both sessions to be registered in gateway session sinks
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(server.has_session_sink(session1).unwrap());
    assert!(server.has_session_sink(session2).unwrap());
    assert_eq!(server.active_sink_count().unwrap(), 2);

    // 2. High-Frequency Movement via QUIC Datagram
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player1_guid.raw(),
        position: Vec3::new(100.5, 20.0, -45.2),
        velocity: Vec3::new(5.0, 0.0, 0.0),
        yaw: std::f32::consts::PI,
        pitch: -0.1,
        timestamp: 1_700_000_001,
        sequence: 42,
    });
    client1.send_datagram(&move_pkt).expect("send datagram");

    let routed_movement = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("timeout waiting for routed movement")
        .expect("routed movement pkt");
    assert_eq!(routed_movement.session_id, session1);
    assert_eq!(routed_movement.packet, move_pkt);

    // 3. Transactional Action via Reliable QUIC Stream
    let interact_pkt = ClientPacket::Interaction(InteractionPacket {
        source_guid: player1_guid.raw(),
        target_guid: 9999,
        interaction_type: InteractionType::Attack,
        action_id: 101,
        payload: bytes::Bytes::from_static(&[10, 20, 30]),
    });
    client1
        .send_packet_stream(&interact_pkt)
        .await
        .expect("send interaction stream");

    let routed_interact = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("timeout waiting for routed interaction")
        .expect("routed interact pkt");
    assert_eq!(routed_interact.session_id, session1);
    assert_eq!(routed_interact.packet, interact_pkt);

    // 4. Ping/Pong RTT Measurement on Stream
    let ping_pkt = ClientPacket::Ping(PingPacket { timestamp: 888_999 });
    client1
        .send_packet_stream(&ping_pkt)
        .await
        .expect("send ping");

    let pong_response = timeout(Duration::from_secs(2), client1.recv_packet_stream())
        .await
        .expect("timeout waiting for pong")
        .expect("recv pong stream");
    match pong_response {
        ClientPacket::Pong(pong) => {
            assert_eq!(pong.client_timestamp, 888_999);
            assert!(pong.server_timestamp > 0);
        }
        other => panic!("Expected Pong, got {other:?}"),
    }

    // 5. Downstream AoI Entity State Broadcast (Ephemeral Datagram)
    let entity_state_pkt = ClientPacket::EntityState(EntityStatePacket {
        entity_id: 777,
        position: Vec3::new(105.0, 20.0, -40.0),
        velocity: Vec3::ZERO,
        yaw: 1.57,
        pitch: 0.0,
        health: 95,
        max_health: 100,
        state_flags: 2,
        timestamp: 1_700_000_050,
    });
    server
        .send_packet_to_session(session1, &entity_state_pkt)
        .await
        .expect("send entity state to session 1");

    let recv_state = timeout(Duration::from_secs(2), client1.read_datagram())
        .await
        .expect("timeout waiting for downstream entity state")
        .expect("read datagram");
    assert_eq!(recv_state, entity_state_pkt);

    // 6. Downstream Transactional Result (Reliable Stream)
    let interaction_result_pkt = ClientPacket::InteractionResult(InteractionResultPacket {
        action_id: 101,
        success: true,
        error_code: 0,
        payload: vec![255, 128],
    });
    server
        .send_packet_to_session(session1, &interaction_result_pkt)
        .await
        .expect("send interaction result to session 1");

    let recv_result = timeout(Duration::from_secs(2), client1.recv_packet_stream())
        .await
        .expect("timeout waiting for downstream interaction result")
        .expect("recv stream");
    assert_eq!(recv_result, interaction_result_pkt);

    // 7. Multi-Client Broadcast (EntityDespawn via Datagram)
    let despawn_pkt = ClientPacket::EntityDespawn(EntityDespawnPacket { entity_id: 777 });
    let broadcast_count = server
        .broadcast_packet(&despawn_pkt)
        .await
        .expect("broadcast packet");
    assert_eq!(broadcast_count, 2);

    let recv_despawn_c1 = timeout(Duration::from_secs(2), client1.read_datagram())
        .await
        .expect("c1 timeout waiting for broadcast despawn")
        .expect("c1 read datagram");
    assert_eq!(recv_despawn_c1, despawn_pkt);

    let recv_despawn_c2 = timeout(Duration::from_secs(2), client2.read_datagram())
        .await
        .expect("c2 timeout waiting for broadcast despawn")
        .expect("c2 read datagram");
    assert_eq!(recv_despawn_c2, despawn_pkt);

    // 8. Graceful Disconnect and Sink Cleanup
    client1.close(0, "client 1 shutdown");
    client2.close(0, "client 2 shutdown");

    // Wait for disconnect cleanup
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.active_sink_count().unwrap(), 0);

    quic_endpoint.close(0u32.into(), b"server shutdown");
    listener_handle.abort();
}
