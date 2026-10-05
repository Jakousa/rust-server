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
    client.send_datagram(&move_pkt).expect("send datagram");

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
    client.send_datagram(&move_pkt).expect("send datagram");

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
    client.send_datagram(&move_pkt).expect("send datagram");

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
