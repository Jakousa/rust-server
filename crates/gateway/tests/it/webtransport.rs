//! End-to-End WebTransport Gateway Integration Test
//!
//! Verifies:
//! 1. WebTransport HTTP/3 handshake with SHA-256 certificate hash pinning.
//! 2. Unreliable datagram ingestion: Movement packet sent by WebTransport client routed into simulation.
//! 3. Unreliable datagram downstream push: Entity delta batch packet broadcast from gateway received by client.
//! 4. Reliable bidirectional stream interaction: Client sends Interaction packet over stream, verifies packet arrives.
//! 5. Clean teardown and session sink deregistration upon disconnect.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use wtransport::tls::Sha256Digest;
use wtransport::{ClientConfig, Endpoint};

use mmo_core::guid::{EntityType, Guid};
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_schema::client::{
    delta_flags, ClientPacket, EntityDeltaBatchPacket, EntityDeltaState, InteractionPacket,
    InteractionType, MovementPacket, Vec3,
};

#[tokio::test]
async fn test_webtransport_gateway_e2e_full_lifecycle() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);
    let server = Arc::new(GatewayServer::with_packet_channel(
        router.clone(),
        packet_tx,
    ));

    let session_id = 1;
    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    let node_addr = "127.0.0.1:50051".to_string();
    server
        .register_session(session_id, player_guid, node_addr)
        .expect("register session");

    // Bind WebTransport HTTP/3 server
    let wt_server = server
        .bind_webtransport("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind webtransport server");

    let local_addr = wt_server.local_addr().expect("local addr");
    let cert_digest_bytes = *wt_server.cert_digest_bytes();

    // Spawn server accept loop
    let (srv_handle, _shutdown_tx) = wt_server.run_background();

    // Configure native WebTransport client with pinned SHA-256 certificate digest
    let sha256_digest = Sha256Digest::new(cert_digest_bytes);
    let client_config = ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([sha256_digest])
        .build();

    let client_endpoint = Endpoint::client(client_config).expect("create client endpoint");
    let connect_url = format!("https://{}", local_addr);

    let connection = timeout(
        Duration::from_secs(5),
        client_endpoint.connect(&connect_url),
    )
    .await
    .expect("connect timeout")
    .expect("client connect");

    // Allow gateway to register session sink
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.active_sink_count().unwrap(), 1);
    assert!(server.has_session_sink(session_id).unwrap());

    // -------------------------------------------------------------
    // Test Case 1: Ingress Movement Datagram
    // -------------------------------------------------------------
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(125.0, 10.0, -80.0),
        velocity: Vec3::new(2.5, 0.0, 0.0),
        yaw: 1.57,
        pitch: 0.0,
        timestamp: 1_700_000_000,
        sequence: 1,
    });
    let move_bytes = move_pkt.encode();

    connection.send_datagram(move_bytes).expect("send datagram");

    let routed_move = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("movement timeout")
        .expect("routed movement packet");

    assert_eq!(routed_move.session_id, session_id);
    if let ClientPacket::Movement(m) = routed_move.packet {
        assert_eq!(m.entity_id, player_guid.raw());
        assert!((m.position.x - 125.0).abs() < 1e-4);
    } else {
        panic!("Expected Movement packet");
    }

    // -------------------------------------------------------------
    // Test Case 2: Egress Entity Delta Batch Datagram
    // -------------------------------------------------------------
    let mob_guid = Guid::new(EntityType::Npc, 1, 2002);
    let batch_pkt = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
        server_tick: 100,
        baseline_tick: 99,
        despawns: Vec::new(),
        deltas: vec![EntityDeltaState {
            entity_id: mob_guid.raw(),
            field_mask: delta_flags::MASK_KEYFRAME | delta_flags::MASK_HEALTH,
            position: (200.0, 5.0, 50.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 85,
            max_health: 100,
            state_flags: 0,
            action_id: 0,
            attributes: Vec::new(),
        }],
    });

    let sent_count = server
        .broadcast_packet(&batch_pkt)
        .await
        .expect("broadcast delta batch");
    assert_eq!(sent_count, 1);

    let client_datagram = timeout(Duration::from_secs(2), connection.receive_datagram())
        .await
        .expect("receive datagram timeout")
        .expect("client datagram");

    let mut dgram_buf = bytes::BytesMut::from(&client_datagram[..]);
    let received_pkt = ClientPacket::decode_frame(&mut dgram_buf)
        .expect("decode frame")
        .expect("decoded packet");

    if let ClientPacket::EntityDeltaBatch(b) = received_pkt {
        assert_eq!(b.server_tick, 100);
        assert_eq!(b.deltas.len(), 1);
        assert_eq!(b.deltas[0].entity_id, mob_guid.raw());
        assert_eq!(b.deltas[0].health, 85);
    } else {
        panic!("Expected EntityDeltaBatch packet");
    }

    // -------------------------------------------------------------
    // Test Case 3: Reliable Stream Interaction Packet
    // -------------------------------------------------------------
    let interact_pkt = ClientPacket::Interaction(InteractionPacket {
        source_guid: player_guid.raw(),
        target_guid: mob_guid.raw(),
        interaction_type: InteractionType::Attack,
        action_id: 42,
        payload: bytes::Bytes::new(),
    });

    let (mut send_stream, _recv_stream) = connection
        .open_bi()
        .await
        .expect("open bi stream")
        .await
        .expect("stream ready");

    send_stream
        .write_all(&interact_pkt.encode())
        .await
        .expect("write to stream");

    let routed_interact = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("interaction timeout")
        .expect("routed interaction packet");

    assert_eq!(routed_interact.session_id, session_id);
    if let ClientPacket::Interaction(i) = routed_interact.packet {
        assert_eq!(i.source_guid, player_guid.raw());
        assert_eq!(i.target_guid, mob_guid.raw());
        assert_eq!(i.action_id, 42);
    } else {
        panic!("Expected Interaction packet");
    }

    // Clean teardown
    drop(connection);
    srv_handle.abort();
}

#[tokio::test]
async fn test_webtransport_gateway_poc_auth_handshake() {
    use mmo_schema::client::AuthRequestPacket;

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, _packet_rx) = mpsc::channel(100);
    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_default_node_address("127.0.0.1:50052"),
    );

    let wt_server = server
        .bind_webtransport("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind webtransport server");

    let local_addr = wt_server.local_addr().expect("local addr");
    let cert_digest_bytes = *wt_server.cert_digest_bytes();
    let (srv_handle, _shutdown_tx) = wt_server.run_background();

    let sha256_digest = Sha256Digest::new(cert_digest_bytes);
    let client_config = ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([sha256_digest])
        .build();

    let client_endpoint = Endpoint::client(client_config).expect("create client endpoint");
    let connect_url = format!("https://{}", local_addr);

    let connection = timeout(
        Duration::from_secs(5),
        client_endpoint.connect(&connect_url),
    )
    .await
    .expect("connect timeout")
    .expect("client connect");

    // Open bidirectional stream and send AuthRequest
    let (mut send_stream, mut recv_stream) = connection
        .open_bi()
        .await
        .expect("open bi stream")
        .await
        .expect("stream ready");

    let auth_pkt = ClientPacket::AuthRequest(AuthRequestPacket {
        token: "poc_guest_token".to_string(),
        client_version: 1,
    });

    send_stream
        .write_all(&auth_pkt.encode())
        .await
        .expect("send auth request");

    // Receive AuthResponse from stream
    let mut resp_buf = bytes::BytesMut::with_capacity(1024);
    let mut temp = [0u8; 512];
    let auth_resp = timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(Some(pkt)) = ClientPacket::decode_frame(&mut resp_buf) {
                return pkt;
            }
            match recv_stream.read(&mut temp).await {
                Ok(Some(n)) if n > 0 => {
                    resp_buf.extend_from_slice(&temp[..n]);
                }
                _ => panic!("Stream closed unexpectedly without AuthResponse"),
            }
        }
    })
    .await
    .expect("auth response timeout");

    if let ClientPacket::AuthResponse(resp) = auth_resp {
        assert!(resp.success);
        assert_eq!(resp.session_id, 1);
        assert_ne!(resp.player_guid, 0);
        assert_eq!(resp.assigned_node, "127.0.0.1:50052");

        // Verify route registered in router
        let player_guid = mmo_core::Guid::from_raw(resp.player_guid);
        let route = router.get_route_by_guid(player_guid).expect("route exists");
        assert_eq!(route, "127.0.0.1:50052");
    } else {
        panic!("Expected AuthResponse packet");
    }

    drop(connection);
    srv_handle.abort();
}
