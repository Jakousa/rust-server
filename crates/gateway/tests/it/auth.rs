//! End-to-End Edge Authentication and JWT Integration Tests
//!
//! Verifies:
//! 1. Zero-cost datagram filtering: unauthenticated datagrams dropped at edge.
//! 2. Full authentication lifecycle: connect -> AuthRequest -> AuthResponse -> route registered -> datagrams forwarded.
//! 3. Anti-spoofing identity validation: packets with mismatching acting GUID dropped.
//! 4. Token validation rejection: expired tokens, tampered signatures, and none algorithm tokens.
//! 5. Handshake timeout: idle unauthenticated connections timed out.
//! 6. TCP authentication handshake and ping/pong.

use bytes::BytesMut;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use mmo_core::guid::{EntityType, Guid};
use mmo_gateway::auth::{EdgeAuthenticator, JwtClaims};
use mmo_gateway::quic::QuicClientSession;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_schema::client::{
    auth_error_codes, AuthRequestPacket, ClientPacket, MovementPacket, PingPacket, Vec3,
};

const TEST_SECRET: &[u8] = b"edge_auth_integration_test_secret_key_123456789";

#[tokio::test]
async fn test_edge_auth_quic_full_lifecycle() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    // Bind QUIC server
    let quic_endpoint = server
        .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
        .expect("bind quic server");
    let local_addr = quic_endpoint.local_addr().expect("local addr");

    let srv = server.clone();
    let srv_endpoint = quic_endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = srv_endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                let s = srv.clone();
                let sid = s.next_session_id();
                tokio::spawn(async move {
                    let _ = s.handle_quic_connection(conn, sid).await;
                });
            }
        }
    });

    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    let valid_jwt = EdgeAuthenticator::create_test_jwt(
        player_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );

    let mut client = QuicClientSession::connect(local_addr, "localhost")
        .await
        .expect("client connect");

    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(10.0, 20.0, 30.0),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1_000_000,
        sequence: 1,
    });

    // 1. Send movement BEFORE authentication - must be dropped at edge
    // Connection may already be closed cleanly by the gateway auth timeout, or datagram dropped.
    let _ = client.send_datagram(&move_pkt);

    let drop_check = timeout(Duration::from_millis(150), packet_rx.recv()).await;
    assert!(
        drop_check.is_err(),
        "Unauthenticated datagram must be dropped and never forwarded"
    );

    // 2. Perform authentication handshake via reliable stream
    let auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: valid_jwt,
        client_version: 1,
    });
    client
        .send_packet_stream(&auth_req)
        .await
        .expect("send auth request");

    let auth_resp = timeout(Duration::from_secs(2), client.recv_packet_stream())
        .await
        .expect("auth response timeout")
        .expect("recv auth response");

    let session_id = match auth_resp {
        ClientPacket::AuthResponse(resp) => {
            assert!(resp.success, "Authentication should succeed");
            assert_eq!(resp.error_code, auth_error_codes::SUCCESS);
            assert_eq!(resp.player_guid, player_guid.raw());
            assert_eq!(resp.assigned_node, "127.0.0.1:50051");
            assert!(resp.session_id > 0);
            resp.session_id
        }
        other => panic!("Expected AuthResponse, got {other:?}"),
    };

    // 3. Send movement AFTER authentication - must now be forwarded!
    // Connection may already be closed cleanly by the gateway auth timeout, or datagram dropped.
    let _ = client.send_datagram(&move_pkt);

    let routed = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("routed packet timeout")
        .expect("received routed packet");

    assert_eq!(routed.session_id, session_id);
    assert_eq!(routed.guid, Some(player_guid));
    match routed.packet {
        ClientPacket::Movement(m) => {
            assert_eq!(m.entity_id, player_guid.raw());
            assert_eq!(m.position.x, 10.0);
        }
        other => panic!("Expected Movement packet, got {other:?}"),
    }
}

#[tokio::test]
async fn test_edge_auth_anti_spoofing() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let quic_endpoint = server
        .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
        .expect("bind quic server");
    let local_addr = quic_endpoint.local_addr().expect("local addr");

    let srv = server.clone();
    let srv_endpoint = quic_endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = srv_endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                let s = srv.clone();
                let sid = s.next_session_id();
                tokio::spawn(async move {
                    let _ = s.handle_quic_connection(conn, sid).await;
                });
            }
        }
    });

    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    let spoofed_guid = Guid::new(EntityType::Player, 1, 9999);

    let valid_jwt = EdgeAuthenticator::create_test_jwt(
        player_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );

    let mut client = QuicClientSession::connect(local_addr, "localhost")
        .await
        .expect("client connect");

    // Authenticate client as player_guid
    let auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: valid_jwt,
        client_version: 1,
    });
    client
        .send_packet_stream(&auth_req)
        .await
        .expect("send auth request");

    let auth_resp = timeout(Duration::from_secs(2), client.recv_packet_stream())
        .await
        .expect("auth response timeout")
        .expect("recv auth response");

    match auth_resp {
        ClientPacket::AuthResponse(resp) => assert!(resp.success),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // Attempt spoofing: send packet with spoofed_guid entity_id
    let spoofed_move = ClientPacket::Movement(MovementPacket {
        entity_id: spoofed_guid.raw(),
        position: Vec3::new(50.0, 50.0, 50.0),
        velocity: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1_000_001,
        sequence: 2,
    });
    client.send_datagram(&spoofed_move).expect("send datagram");

    let spoof_check = timeout(Duration::from_millis(150), packet_rx.recv()).await;
    assert!(
        spoof_check.is_err(),
        "Spoofed packet must be dropped and never forwarded"
    );

    // Send legitimate packet with authenticated player_guid
    let legit_move = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(10.0, 10.0, 10.0),
        velocity: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1_000_002,
        sequence: 3,
    });
    client.send_datagram(&legit_move).expect("send datagram");

    let routed = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("legit packet timeout")
        .expect("recv legit packet");
    assert_eq!(routed.guid, Some(player_guid));
}

#[tokio::test]
async fn test_edge_auth_token_rejections() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, _packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let quic_endpoint = server
        .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
        .expect("bind quic server");
    let local_addr = quic_endpoint.local_addr().expect("local addr");

    let srv = server.clone();
    let srv_endpoint = quic_endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = srv_endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                let s = srv.clone();
                let sid = s.next_session_id();
                tokio::spawn(async move {
                    let _ = s.handle_quic_connection(conn, sid).await;
                });
            }
        }
    });

    let player_guid = Guid::new(EntityType::Player, 1, 1001);

    // 1. Expired token rejection
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = JwtClaims {
            sub: player_guid.raw().to_string(),
            iss: Some("mmo-auth".to_string()),
            aud: Some("mmo-gateway".to_string()),
            exp: now - 100, // expired
            nbf: None,
            iat: Some(now - 200),
            guid: Some(player_guid.raw()),
            roles: None,
        };
        let expired_token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET),
        )
        .unwrap();

        let mut client = QuicClientSession::connect(local_addr, "localhost")
            .await
            .expect("client connect");

        client
            .send_packet_stream(&ClientPacket::AuthRequest(AuthRequestPacket {
                token: expired_token,
                client_version: 1,
            }))
            .await
            .unwrap();

        let resp = timeout(Duration::from_secs(2), client.recv_packet_stream())
            .await
            .unwrap()
            .unwrap();

        match resp {
            ClientPacket::AuthResponse(r) => {
                assert!(!r.success);
                assert_eq!(r.error_code, auth_error_codes::EXPIRED_TOKEN);
            }
            other => panic!("Expected AuthResponse, got {other:?}"),
        }
    }

    // 2. Tampered signature rejection
    {
        let valid_jwt = EdgeAuthenticator::create_test_jwt(
            player_guid,
            TEST_SECRET,
            3600,
            Some("mmo-auth"),
            Some("mmo-gateway"),
        );
        let tampered_jwt = format!("{}tampered", valid_jwt);

        let mut client = QuicClientSession::connect(local_addr, "localhost")
            .await
            .expect("client connect");

        client
            .send_packet_stream(&ClientPacket::AuthRequest(AuthRequestPacket {
                token: tampered_jwt,
                client_version: 1,
            }))
            .await
            .unwrap();

        let resp = timeout(Duration::from_secs(2), client.recv_packet_stream())
            .await
            .unwrap()
            .unwrap();

        match resp {
            ClientPacket::AuthResponse(r) => {
                assert!(!r.success);
                assert_eq!(r.error_code, auth_error_codes::INVALID_SIGNATURE);
            }
            other => panic!("Expected AuthResponse, got {other:?}"),
        }
    }

    // 3. Algorithm 'none' attack rejection
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let header = r#"{"alg":"none","typ":"JWT"}"#;
        let claims = format!(
            r#"{{"sub":"{}","iss":"mmo-auth","aud":"mmo-gateway","exp":{}}}"#,
            player_guid.raw(),
            now + 3600
        );
        let b64_header = data_encoding::BASE64URL_NOPAD.encode(header.as_bytes());
        let b64_claims = data_encoding::BASE64URL_NOPAD.encode(claims.as_bytes());
        let none_token = format!("{}.{}.", b64_header, b64_claims);

        let mut client = QuicClientSession::connect(local_addr, "localhost")
            .await
            .expect("client connect");

        client
            .send_packet_stream(&ClientPacket::AuthRequest(AuthRequestPacket {
                token: none_token,
                client_version: 1,
            }))
            .await
            .unwrap();

        let resp = timeout(Duration::from_secs(2), client.recv_packet_stream())
            .await
            .unwrap()
            .unwrap();

        match resp {
            ClientPacket::AuthResponse(r) => {
                assert!(!r.success);
            }
            other => panic!("Expected AuthResponse, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_edge_auth_timeout_drops_idle_client() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_millis(150)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let quic_endpoint = server
        .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
        .expect("bind quic server");
    let local_addr = quic_endpoint.local_addr().expect("local addr");

    let srv = server.clone();
    let srv_endpoint = quic_endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = srv_endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                let s = srv.clone();
                let sid = s.next_session_id();
                tokio::spawn(async move {
                    let _ = s.handle_quic_connection(conn, sid).await;
                });
            }
        }
    });

    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    let client = QuicClientSession::connect(local_addr, "localhost")
        .await
        .expect("client connect");

    // Idle past auth timeout
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Any datagram sent after timeout must be dropped
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(0.0, 0.0, 0.0),
        velocity: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1_000_000,
        sequence: 1,
    });
    // Connection may already be closed cleanly by the gateway auth timeout, or datagram dropped.
    let _ = client.send_datagram(&move_pkt);

    let drop_check = timeout(Duration::from_millis(150), packet_rx.recv()).await;
    assert!(
        drop_check.is_err(),
        "Timed out client datagram must be dropped"
    );
}

#[tokio::test]
async fn test_edge_auth_tcp_handshake() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, _packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let listener = server.bind("127.0.0.1:0").await.expect("bind tcp");
    let local_addr = listener.local_addr().expect("local addr");

    let srv = server.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let s = srv.clone();
            let sid = s.next_session_id();
            tokio::spawn(async move {
                let _ = s.handle_connection(socket, sid).await;
            });
        }
    });

    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    let valid_jwt = EdgeAuthenticator::create_test_jwt(
        player_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );

    let mut stream = TcpStream::connect(local_addr).await.expect("connect tcp");

    // Send AuthRequest frame
    let auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: valid_jwt,
        client_version: 1,
    });
    stream
        .write_all(&auth_req.encode())
        .await
        .expect("write auth req");

    // Read AuthResponse frame
    let mut buf = BytesMut::with_capacity(4096);
    let mut chunk = [0u8; 1024];

    let auth_resp = loop {
        if let Some(pkt) = ClientPacket::decode_frame(&mut buf).expect("decode frame") {
            break pkt;
        }
        let n = stream.read(&mut chunk).await.expect("read chunk");
        assert!(n > 0, "Unexpected EOF reading auth response");
        buf.extend_from_slice(&chunk[..n]);
    };

    match auth_resp {
        ClientPacket::AuthResponse(r) => {
            assert!(r.success);
            assert_eq!(r.error_code, auth_error_codes::SUCCESS);
            assert_eq!(r.player_guid, player_guid.raw());
            assert_eq!(r.assigned_node, "127.0.0.1:50051");
        }
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // Verify session can send Ping and receive Pong
    let ping = ClientPacket::Ping(PingPacket { timestamp: 123456 });
    stream.write_all(&ping.encode()).await.expect("write ping");

    let pong_resp = loop {
        if let Some(pkt) = ClientPacket::decode_frame(&mut buf).expect("decode frame") {
            break pkt;
        }
        let n = stream.read(&mut chunk).await.expect("read chunk");
        assert!(n > 0, "Unexpected EOF reading pong");
        buf.extend_from_slice(&chunk[..n]);
    };

    match pong_resp {
        ClientPacket::Pong(pong) => {
            assert_eq!(pong.client_timestamp, 123456);
            assert!(pong.server_timestamp > 0);
        }
        other => panic!("Expected Pong, got {other:?}"),
    }
}

#[tokio::test]
async fn test_edge_auth_webtransport_lifecycle() {
    use wtransport::tls::Sha256Digest;
    use wtransport::{ClientConfig, Endpoint};

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let wt_server = server
        .bind_webtransport("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind webtransport");

    let local_addr = wt_server.local_addr().expect("local addr");
    let cert_digest_bytes = *wt_server.cert_digest_bytes();

    let (_srv_handle, _shutdown_tx) = wt_server.run_background();

    let sha256_digest = Sha256Digest::new(cert_digest_bytes);
    let client_config = ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([sha256_digest])
        .build();

    let client_endpoint = Endpoint::client(client_config).expect("create client endpoint");
    let connect_url = format!("https://{}", local_addr);
    let connection = client_endpoint
        .connect(&connect_url)
        .await
        .expect("connect webtransport");

    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    let valid_jwt = EdgeAuthenticator::create_test_jwt(
        player_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );

    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(10.0, 20.0, 30.0),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1_000_000,
        sequence: 1,
    });

    // 1. Send movement BEFORE auth - must be dropped at edge
    connection
        .send_datagram(move_pkt.encode())
        .expect("send datagram");

    let drop_check = timeout(Duration::from_millis(150), packet_rx.recv()).await;
    assert!(
        drop_check.is_err(),
        "WebTransport unauthenticated datagram must be dropped at edge"
    );

    // 2. Perform authentication handshake via stream
    let (mut send, mut recv) = connection.open_bi().await.unwrap().await.unwrap();
    let auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: valid_jwt,
        client_version: 1,
    });
    send.write_all(&auth_req.encode())
        .await
        .expect("write auth req");

    let mut buf = BytesMut::with_capacity(4096);
    let mut chunk = [0u8; 1024];

    let auth_resp = loop {
        if let Some(pkt) = ClientPacket::decode_frame(&mut buf).expect("decode frame") {
            break pkt;
        }
        let n = recv
            .read(&mut chunk)
            .await
            .expect("read chunk")
            .expect("chunk");
        buf.extend_from_slice(&chunk[..n]);
    };

    match auth_resp {
        ClientPacket::AuthResponse(r) => {
            assert!(r.success);
            assert_eq!(r.error_code, auth_error_codes::SUCCESS);
            assert_eq!(r.player_guid, player_guid.raw());
            assert_eq!(r.assigned_node, "127.0.0.1:50051");
        }
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // 3. Send movement AFTER auth - must now be forwarded!
    connection
        .send_datagram(move_pkt.encode())
        .expect("send datagram");

    let routed = timeout(Duration::from_secs(2), packet_rx.recv())
        .await
        .expect("routed packet timeout")
        .expect("received routed packet");

    assert_eq!(routed.guid, Some(player_guid));
}

#[tokio::test]
async fn test_tcp_pre_auth_buffer_overflow_dropped() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, _packet_rx) = mpsc::channel(16);
    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let listener = server.bind("127.0.0.1:0").await.expect("bind tcp");
    let local_addr = listener.local_addr().expect("local addr");

    let srv = server.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let s = srv.clone();
            let sid = s.next_session_id();
            tokio::spawn(async move {
                let _ = s.handle_connection(socket, sid).await;
            });
        }
    });

    let mut stream = TcpStream::connect(local_addr).await.expect("connect tcp");

    // Send garbage / junk bytes exceeding MAX_PACKET_PAYLOAD_SIZE (65,536 bytes)
    let junk = vec![0x4Du8; 70_000];
    let _ = stream.write_all(&junk).await;

    // Server must reject and close connection (read returns 0 / EOF)
    let mut buf = [0u8; 128];
    let res = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
    match res {
        Ok(Ok(n)) => assert_eq!(
            n, 0,
            "Server must close connection on pre-auth buffer overflow"
        ),
        Ok(Err(_)) => {} // Connection reset / closed is also valid
        Err(_) => panic!("Timed out waiting for server to close connection"),
    }
}

#[tokio::test]
async fn test_quic_multi_stream_auth_bypass_and_post_auth_sinks() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let quic_endpoint = server
        .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
        .expect("bind quic server");
    let local_addr = quic_endpoint.local_addr().expect("local addr");

    let srv = server.clone();
    let srv_endpoint = quic_endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = srv_endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                let s = srv.clone();
                let sid = s.next_session_id();
                tokio::spawn(async move {
                    let _ = s.handle_quic_connection(conn, sid).await;
                });
            }
        }
    });

    let mut client = QuicClientSession::connect(local_addr, "localhost")
        .await
        .expect("client connect");

    // Open primary bi-stream and send a non-auth packet first? Wait: keep primary stream
    // idle waiting for AuthRequest while probing a secondary stream & session_sinks.
    client
        .ensure_bi_stream()
        .await
        .expect("open primary stream");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 1. Pre-auth connection must NOT be registered in session_sinks and must NOT receive broadcasts.
    assert_eq!(server.active_sink_count().unwrap(), 0);
    let broadcast_pkt = ClientPacket::Pong(mmo_schema::client::PongPacket {
        client_timestamp: 111,
        server_timestamp: 222,
    });
    assert_eq!(
        server.broadcast_packet(&broadcast_pkt).await.unwrap(),
        0,
        "Unauthenticated QUIC session must not receive broadcast packets"
    );

    // 2. Opening a secondary bi-stream BEFORE completing auth on stream 1 must NOT bypass auth.
    let player_guid = Guid::new(EntityType::Player, 1, 7001);
    let (mut rogue_send, mut rogue_recv) = client
        .connection
        .open_bi()
        .await
        .expect("open secondary bi-stream");
    let rogue_move = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(10.0, 0.0, 10.0),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 42,
        sequence: 1,
    });
    rogue_send
        .write_all(&rogue_move.encode())
        .await
        .expect("write rogue movement");
    let rogue_ping = ClientPacket::Ping(PingPacket { timestamp: 999 });
    rogue_send
        .write_all(&rogue_ping.encode())
        .await
        .expect("write rogue ping");

    assert!(
        timeout(Duration::from_millis(120), packet_rx.recv())
            .await
            .is_err(),
        "Packets sent on secondary QUIC stream before auth must not be routed"
    );
    let mut probe = [0u8; 64];
    assert!(
        timeout(Duration::from_millis(80), rogue_recv.read(&mut probe))
            .await
            .is_err(),
        "Secondary QUIC stream must not be handled before primary stream auth completes"
    );

    // 3. Complete auth on the primary stream -> session_sinks gets registered AFTER auth.
    let valid_jwt = EdgeAuthenticator::create_test_jwt(
        player_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );
    client
        .send_packet_stream(&ClientPacket::AuthRequest(AuthRequestPacket {
            token: valid_jwt,
            client_version: 1,
        }))
        .await
        .expect("send auth request");

    let auth_resp = timeout(Duration::from_secs(2), client.recv_packet_stream())
        .await
        .expect("auth response timeout")
        .expect("read auth response");
    match auth_resp {
        ClientPacket::AuthResponse(r) => assert!(r.success),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    assert_eq!(server.active_sink_count().unwrap(), 1);
    assert_eq!(server.broadcast_packet(&broadcast_pkt).await.unwrap(), 1);
}

#[tokio::test]
async fn test_webtransport_multi_stream_auth_bypass_and_post_auth_sinks() {
    use wtransport::tls::Sha256Digest;
    use wtransport::{ClientConfig, Endpoint};

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(3)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let wt_server = server
        .bind_webtransport("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind webtransport");

    let local_addr = wt_server.local_addr().expect("local addr");
    let cert_digest_bytes = *wt_server.cert_digest_bytes();
    let (_srv_handle, shutdown_tx) = wt_server.run_background();

    let client_config = ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([Sha256Digest::new(cert_digest_bytes)])
        .build();
    let client_endpoint = Endpoint::client(client_config).expect("client endpoint");
    let connection = client_endpoint
        .connect(format!("https://{local_addr}"))
        .await
        .expect("connect webtransport");

    // Open initial bi-stream without sending AuthRequest yet
    let (mut auth_send, mut auth_recv) = connection
        .open_bi()
        .await
        .expect("open_bi")
        .await
        .expect("await primary stream");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 1. Pre-auth WebTransport session must NOT be in session_sinks
    assert_eq!(server.active_sink_count().unwrap(), 0);
    let broadcast_pkt = ClientPacket::Pong(mmo_schema::client::PongPacket {
        client_timestamp: 123,
        server_timestamp: 456,
    });
    assert_eq!(server.broadcast_packet(&broadcast_pkt).await.unwrap(), 0);

    // 2. Secondary stream opened before initial stream completes auth must NOT route packets
    let player_guid = Guid::new(EntityType::Player, 1, 7002);
    let (mut sec_send, _sec_recv) = connection
        .open_bi()
        .await
        .expect("open secondary bi")
        .await
        .expect("await secondary stream");
    let rogue_move = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(5.0, 0.0, 5.0),
        velocity: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 55,
        sequence: 1,
    });
    sec_send
        .write_all(&rogue_move.encode())
        .await
        .expect("write rogue wt move");

    assert!(
        timeout(Duration::from_millis(120), packet_rx.recv())
            .await
            .is_err(),
        "Pre-auth secondary WebTransport stream packets must not reach node channel"
    );

    // 3. Authenticate on primary stream -> session_sinks registered and broadcasts work
    let valid_jwt = EdgeAuthenticator::create_test_jwt(
        player_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );
    auth_send
        .write_all(
            &ClientPacket::AuthRequest(AuthRequestPacket {
                token: valid_jwt,
                client_version: 1,
            })
            .encode(),
        )
        .await
        .expect("write auth req");

    let mut buf = BytesMut::with_capacity(4096);
    let mut chunk = [0u8; 1024];
    let auth_resp = timeout(Duration::from_secs(2), async {
        loop {
            if let Some(pkt) = ClientPacket::decode_frame(&mut buf).expect("decode frame") {
                break pkt;
            }
            let n = auth_recv
                .read(&mut chunk)
                .await
                .expect("read")
                .expect("eof");
            buf.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .expect("auth response timeout");
    match auth_resp {
        ClientPacket::AuthResponse(r) => assert!(r.success),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    assert_eq!(server.active_sink_count().unwrap(), 1);
    assert_eq!(server.broadcast_packet(&broadcast_pkt).await.unwrap(), 1);

    let _ = shutdown_tx.send(());
}
