//! End-to-End WebSocket (WS & WSS) Fallback Transport Integration Tests
//!
//! Verifies:
//! 1. Plain WebSocket (`ws://`) binary framing parity (`0x4D4D` header + varints).
//! 2. Ingress Movement, Interaction, Ping/Pong, and coalesced multi-frame binary messages.
//! 3. Egress EntityDeltaBatch broadcast multiplexing.
//! 4. TLS WebSocket (`wss://`) with `EdgeAuthenticator` JWT handshake and invalid frame rejection.
//! 5. Clean teardown and session sink deregistration upon disconnect.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use mmo_core::guid::{EntityType, Guid};
use mmo_gateway::auth::EdgeAuthenticator;
use mmo_gateway::quic::{generate_self_signed_cert, SkipServerVerification};
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_schema::client::{
    delta_flags, AuthRequestPacket, ClientPacket, EntityDeltaBatchPacket, EntityDeltaState,
    InteractionPacket, InteractionType, MovementPacket, PingPacket, Vec3,
};
use mmo_storage::cache::{MemoryCache, SessionCacheBackend};

fn build_test_server_tls_config() -> Arc<rustls::ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (certs, key) =
        generate_self_signed_cert(&["localhost", "127.0.0.1"]).expect("self-signed cert");
    Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("server tls config"),
    )
}

async fn connect_wss(addr: SocketAddr) -> WebSocketStream<TlsStream<TcpStream>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client_tls = Arc::new(
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification::new()))
            .with_no_client_auth(),
    );
    let tls_connector = tokio_rustls::TlsConnector::from(client_tls);
    let tcp_stream = timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .expect("tcp connect timeout")
        .expect("tcp connect");
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tls_stream = timeout(
        Duration::from_secs(5),
        tls_connector.connect(server_name, tcp_stream),
    )
    .await
    .expect("tls connect timeout")
    .expect("tls connect");

    let url = format!("wss://{addr}");
    let (ws_stream, _) = timeout(
        Duration::from_secs(5),
        tokio_tungstenite::client_async(&url, tls_stream),
    )
    .await
    .expect("wss connect timeout")
    .expect("wss connect");
    ws_stream
}

#[tokio::test]
async fn test_websocket_gateway_e2e_full_lifecycle() {
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

    let ws_server = server
        .bind_websocket("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind websocket server");
    let local_addr = ws_server.local_addr().expect("local addr");
    assert!(!ws_server.is_tls());

    let (_srv_handle, shutdown_tx) = ws_server.run_background();

    let url = format!("ws://{local_addr}");
    let (mut ws_stream, _) = timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .expect("connect timeout")
    .expect("ws connect");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.active_sink_count().unwrap(), 1);
    assert!(server.has_session_sink(session_id).unwrap());

    // 1. Ingress Movement packet
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(125.0, 10.0, -80.0),
        velocity: Vec3::new(2.5, 0.0, 0.0),
        yaw: 1.57,
        pitch: 0.0,
        timestamp: 1_700_000_000,
        sequence: 1,
    });
    ws_stream
        .send(Message::Binary(move_pkt.encode().into()))
        .await
        .expect("send movement");

    let routed_move = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("movement timeout")
        .expect("routed movement");
    assert_eq!(routed_move.session_id, session_id);
    if let ClientPacket::Movement(m) = routed_move.packet {
        assert_eq!(m.entity_id, player_guid.raw());
        assert!((m.position.x - 125.0).abs() < 1e-4);
    } else {
        panic!("Expected Movement packet");
    }

    // 2. Egress EntityDeltaBatch (varint-encoded)
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

    let msg = timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("receive timeout")
        .expect("ws stream item")
        .expect("ws message");
    let data = match msg {
        Message::Binary(b) => b,
        other => panic!("Expected Binary message, got {other:?}"),
    };
    let mut buf = BytesMut::from(&data[..]);
    let decoded = ClientPacket::decode_frame(&mut buf)
        .expect("decode frame")
        .expect("decoded packet");
    if let ClientPacket::EntityDeltaBatch(b) = decoded {
        assert_eq!(b.server_tick, 100);
        assert_eq!(b.deltas.len(), 1);
        assert_eq!(b.deltas[0].entity_id, mob_guid.raw());
        assert_eq!(b.deltas[0].health, 85);
    } else {
        panic!("Expected EntityDeltaBatch packet");
    }

    // 3. Ping / Pong RTT
    let ping_pkt = ClientPacket::Ping(PingPacket { timestamp: 987654 });
    ws_stream
        .send(Message::Binary(ping_pkt.encode().into()))
        .await
        .expect("send ping");

    let pong_msg = timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("pong timeout")
        .expect("ws stream item")
        .expect("ws pong message");
    if let Message::Binary(b) = pong_msg {
        let mut pbuf = BytesMut::from(&b[..]);
        match ClientPacket::decode_frame(&mut pbuf).unwrap().unwrap() {
            ClientPacket::Pong(p) => assert_eq!(p.client_timestamp, 987654),
            other => panic!("Expected Pong, got {other:?}"),
        }
    } else {
        panic!("Expected Binary Pong");
    }

    // 4. Coalesced multi-frame binary message (two packets in one WS Binary message)
    let move2 = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(130.0, 10.0, -80.0),
        velocity: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1_700_000_010,
        sequence: 2,
    });
    let interact = ClientPacket::Interaction(InteractionPacket {
        source_guid: player_guid.raw(),
        target_guid: mob_guid.raw(),
        interaction_type: InteractionType::Talk,
        action_id: 42,
        payload: vec![1, 2, 3].into(),
    });
    let mut coalesced = BytesMut::new();
    coalesced.put_slice(&move2.encode());
    coalesced.put_slice(&interact.encode());
    ws_stream
        .send(Message::Binary(coalesced.freeze()))
        .await
        .expect("send coalesced");

    let r1 = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let r2 = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(r1.packet, ClientPacket::Movement(_)));
    assert!(matches!(r2.packet, ClientPacket::Interaction(_)));

    // 5. Clean teardown
    let _ = ws_stream.close(None).await;
    drop(ws_stream);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.active_sink_count().unwrap(), 0);
    assert!(!server.has_session_sink(session_id).unwrap());

    let _ = shutdown_tx.send(());
}

#[tokio::test]
async fn test_websocket_tls_and_auth_handshake() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let secret = b"ws-test-secret-key-32-bytes-long!";
    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(secret, None, None).with_auth_timeout(Duration::from_secs(2)),
    );

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);
    let server = Arc::new(
        GatewayServer::with_packet_channel(router, packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50052"),
    );

    let server_tls = build_test_server_tls_config();

    let ws_server = server
        .bind_websocket_tls("127.0.0.1:0".parse().unwrap(), server_tls)
        .await
        .expect("bind wss server");
    let local_addr = ws_server.local_addr().expect("local addr");
    assert!(ws_server.is_tls());

    let (_srv_handle, shutdown_tx) = ws_server.run_background();

    let mut ws_stream = connect_wss(local_addr).await;

    let player_guid = Guid::new(EntityType::Player, 1, 777);
    let token = EdgeAuthenticator::create_test_jwt(player_guid, secret, 3600, None, None);
    let auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token,
        client_version: 1,
    });
    ws_stream
        .send(Message::Binary(auth_req.encode().into()))
        .await
        .expect("send auth req");

    let auth_msg = timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("auth resp timeout")
        .expect("ws item")
        .expect("ws msg");
    let mut abuf = match auth_msg {
        Message::Binary(b) => BytesMut::from(&b[..]),
        other => panic!("Expected Binary AuthResponse, got {other:?}"),
    };
    match ClientPacket::decode_frame(&mut abuf).unwrap().unwrap() {
        ClientPacket::AuthResponse(resp) => {
            assert!(resp.success);
            assert_eq!(resp.player_guid, player_guid.raw());
            assert_eq!(resp.assigned_node, "127.0.0.1:50052");
        }
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // Authenticated Movement packet succeeds
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(10.0, 20.0, 30.0),
        velocity: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1000,
        sequence: 1,
    });
    ws_stream
        .send(Message::Binary(move_pkt.encode().into()))
        .await
        .expect("send movement");
    let routed = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("movement timeout")
        .expect("routed movement");
    assert_eq!(routed.guid, Some(player_guid));

    // Sending a Text frame violates binary framing and closes the session
    let _ = ws_stream.send(Message::Text("invalid".into())).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.active_sink_count().unwrap(), 0);

    let _ = shutdown_tx.send(());
}

#[tokio::test]
async fn test_websocket_rejects_invalid_jwt_and_unauthenticated_packet() {
    let secret = b"ws-invalid-jwt-secret-32-bytes!!";
    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(secret, None, None).with_auth_timeout(Duration::from_secs(2)),
    );
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(16);
    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50052"),
    );

    let ws_server = server
        .bind_websocket_tls(
            "127.0.0.1:0".parse().unwrap(),
            build_test_server_tls_config(),
        )
        .await
        .expect("bind wss server");
    let local_addr = ws_server.local_addr().unwrap();
    let (_srv_handle, shutdown_tx) = ws_server.run_background();

    // 1. Malformed / invalid JWT must return AuthResponse(success=false) and terminate connection
    let mut ws_invalid = connect_wss(local_addr).await;
    let bad_auth = ClientPacket::AuthRequest(AuthRequestPacket {
        token: "not.a.valid.jwt".to_string(),
        client_version: 1,
    });
    ws_invalid
        .send(Message::Binary(bad_auth.encode().into()))
        .await
        .expect("send invalid auth");

    let msg = timeout(Duration::from_secs(2), ws_invalid.next())
        .await
        .expect("auth resp timeout")
        .expect("ws item")
        .expect("ws msg");
    let mut abuf = match msg {
        Message::Binary(b) => BytesMut::from(&b[..]),
        other => panic!("Expected binary AuthResponse, got {other:?}"),
    };
    match ClientPacket::decode_frame(&mut abuf).unwrap().unwrap() {
        ClientPacket::AuthResponse(resp) => {
            assert!(!resp.success);
            assert_ne!(resp.error_code, 0);
        }
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // Server closes the connection shortly after sending the rejection frame
    let closed_msg = timeout(Duration::from_secs(2), ws_invalid.next())
        .await
        .expect("close timeout");
    assert!(matches!(
        closed_msg,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
    assert_eq!(server.active_sink_count().unwrap(), 0);
    assert!(router.is_empty());

    // 2. Sending non-AuthRequest packet first when auth is enabled closes immediately
    let mut ws_unauth = connect_wss(local_addr).await;
    let fake_guid = Guid::new(EntityType::Player, 1, 999);
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: fake_guid.raw(),
        position: Vec3::new(1.0, 2.0, 3.0),
        velocity: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1,
        sequence: 1,
    });
    ws_unauth
        .send(Message::Binary(move_pkt.encode().into()))
        .await
        .expect("send unauthenticated movement");

    let unauth_closed = timeout(Duration::from_secs(2), ws_unauth.next())
        .await
        .expect("unauth close timeout");
    assert!(matches!(
        unauth_closed,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
    assert!(packet_rx.try_recv().is_err());
    assert_eq!(server.active_sink_count().unwrap(), 0);
    assert!(router.is_empty());

    let _ = shutdown_tx.send(());
}

#[tokio::test]
async fn test_websocket_slowloris_incomplete_handshake_timeout() {
    let secret = b"ws-slowloris-secret-32-bytes!!!!";
    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(secret, None, None)
            .with_auth_timeout(Duration::from_millis(120)),
    );
    let router = Arc::new(SessionRouter::new());
    let server = Arc::new(
        GatewayServer::new(router.clone())
            .with_authenticator(auth)
            .with_handshake_timeout(Duration::from_millis(120))
            .with_default_node_address("127.0.0.1:50052"),
    );

    // Pre-upgrade HTTP Slowloris test over plain WebSocket listener
    let ws_server = server
        .bind_websocket("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind ws server");
    let ws_addr = ws_server.local_addr().unwrap();
    let (_ws_handle, ws_shutdown_tx) = ws_server.run_background();

    let mut raw_tcp = TcpStream::connect(ws_addr).await.expect("connect raw tcp");
    raw_tcp
        .write_all(b"GET / HTTP/1.1\r\n")
        .await
        .expect("write partial upgrade");
    let mut scratch = [0u8; 64];
    let eof_res = timeout(Duration::from_secs(2), raw_tcp.read(&mut scratch))
        .await
        .expect("pre-upgrade slowloris timeout did not fire");
    assert!(matches!(eof_res, Ok(0) | Err(_)));
    assert_eq!(server.active_sink_count().unwrap(), 0);

    // Post-upgrade incomplete auth packet Slowloris over WSS
    let wss_server = server
        .bind_websocket_tls(
            "127.0.0.1:0".parse().unwrap(),
            build_test_server_tls_config(),
        )
        .await
        .expect("bind wss server");
    let wss_addr = wss_server.local_addr().unwrap();
    let (_wss_handle, wss_shutdown_tx) = wss_server.run_background();

    let mut ws_stream = connect_wss(wss_addr).await;
    // Send incomplete 0x4D4D header + truncated varint length without full frame
    ws_stream
        .send(Message::Binary(vec![0x4D, 0x4D, 0x20, 0x01].into()))
        .await
        .expect("send partial auth frame");

    let post_res = timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("post-upgrade auth slowloris timeout did not fire");
    assert!(matches!(
        post_res,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
    assert_eq!(server.active_sink_count().unwrap(), 0);
    assert!(router.is_empty());

    let _ = ws_shutdown_tx.send(());
    let _ = wss_shutdown_tx.send(());
}

#[tokio::test]
async fn test_websocket_disconnect_cleans_up_router_sinks_and_cache() {
    let secret = b"ws-disconnect-cleanup-32-bytes!!";
    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(secret, None, None).with_auth_timeout(Duration::from_secs(2)),
    );
    let router = Arc::new(SessionRouter::new());
    let cache = Arc::new(MemoryCache::new());
    let cache_backend: Arc<dyn SessionCacheBackend> = cache.clone();
    let (packet_tx, _packet_rx) = mpsc::channel(16);

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_cache(cache_backend)
            .with_default_node_address("127.0.0.1:50052"),
    );

    let ws_server = server
        .bind_websocket_tls(
            "127.0.0.1:0".parse().unwrap(),
            build_test_server_tls_config(),
        )
        .await
        .expect("bind wss server");
    let local_addr = ws_server.local_addr().unwrap();
    let (_srv_handle, shutdown_tx) = ws_server.run_background();

    let mut ws_stream = connect_wss(local_addr).await;
    let player_guid = Guid::new(EntityType::Player, 1, 888);
    let token = EdgeAuthenticator::create_test_jwt(player_guid, secret, 3600, None, None);
    let auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token,
        client_version: 1,
    });
    ws_stream
        .send(Message::Binary(auth_req.encode().into()))
        .await
        .expect("send auth req");

    let auth_msg = timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("auth resp timeout")
        .expect("ws item")
        .expect("ws msg");
    let mut abuf = match auth_msg {
        Message::Binary(b) => BytesMut::from(&b[..]),
        other => panic!("Expected binary AuthResponse, got {other:?}"),
    };
    let sid = match ClientPacket::decode_frame(&mut abuf).unwrap().unwrap() {
        ClientPacket::AuthResponse(resp) => {
            assert!(resp.success);
            assert_eq!(resp.player_guid, player_guid.raw());
            resp.session_id
        }
        other => panic!("Expected AuthResponse, got {other:?}"),
    };

    // Confirm router, sinks, and cache are all populated while connected
    assert!(server.has_session_sink(sid).unwrap());
    assert_eq!(server.active_sink_count().unwrap(), 1);
    assert_eq!(
        router.get_route_by_session(sid).as_deref(),
        Some("127.0.0.1:50052")
    );
    assert_eq!(router.get_session_by_guid(player_guid), Some(sid));
    assert_eq!(
        router.get_route_by_guid(player_guid).as_deref(),
        Some("127.0.0.1:50052")
    );
    let cached = cache
        .get_session(sid)
        .await
        .unwrap()
        .expect("session cached on auth");
    assert_eq!(cached.guid, player_guid.raw());
    assert_eq!(cached.node_address, "127.0.0.1:50052");

    // Disconnect client and wait for async cache cleanup spawned by SessionDropGuard
    let _ = ws_stream.close(None).await;
    drop(ws_stream);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let sink_gone = !server.has_session_sink(sid).unwrap();
        let router_gone = router.get_route_by_session(sid).is_none()
            && router.get_session_by_guid(player_guid).is_none()
            && router.get_route_by_guid(player_guid).is_none();
        let cache_gone = cache.get_session(sid).await.unwrap().is_none();
        if sink_gone && router_gone && cache_gone {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Disconnect cleanup did not finish in time (sink_gone={sink_gone}, router_gone={router_gone}, cache_gone={cache_gone})"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(server.active_sink_count().unwrap(), 0);
    assert!(!server.has_session_sink(sid).unwrap());
    assert!(router.is_empty());

    let _ = shutdown_tx.send(());
}
