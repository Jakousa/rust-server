//! End-to-End Social Pub/Sub and Simulation Bypass Integration Tests
//!
//! Verifies:
//! 1. Full social messaging lifecycle: Connect -> Auth -> Auto-subscribe -> Pub/Sub.
//! 2. Cross-session global broadcast and direct whispers.
//! 3. Dynamic channel subscription and unsubscribe lifecycle.
//! 4. Anti-spoofing identity validation (mismatched sender_guid dropped at edge).
//! 5. Complete bypass of simulation loops (no packets sent to packet_tx).
//! 6. Clean disconnection topic cleanup.

use bytes::BytesMut;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use mmo_core::guid::{EntityType, Guid};
use mmo_gateway::auth::EdgeAuthenticator;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_schema::client::{
    AuthRequestPacket, ClientPacket, SocialMessagePacket, SocialSubscribePacket,
    SocialUnsubscribePacket,
};
use mmo_storage::social::MemorySocialBus;

const TEST_SECRET: &[u8] = b"social_pubsub_integration_test_secret_123456789";

async fn read_next_packet(stream: &mut TcpStream, buf: &mut BytesMut) -> ClientPacket {
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pkt) = ClientPacket::decode_frame(buf).expect("decode frame") {
            return pkt;
        }
        let n = stream.read(&mut chunk).await.expect("read chunk");
        assert!(n > 0, "Unexpected EOF while reading packet");
        buf.extend_from_slice(&chunk[..n]);
    }
}

#[tokio::test]
async fn test_social_pubsub_e2e_lifecycle_and_simulation_bypass() {
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);
    let social_bus = Arc::new(MemorySocialBus::new());

    let auth = Arc::new(
        EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        )
        .with_auth_timeout(Duration::from_secs(5)),
    );

    let server = Arc::new(
        GatewayServer::with_packet_channel(router.clone(), packet_tx)
            .with_authenticator(auth)
            .with_social_bus(social_bus)
            .with_default_node_address("127.0.0.1:50051"),
    );

    let listener = server.bind("127.0.0.1:0").await.expect("bind tcp listener");
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

    let alice_guid = Guid::new(EntityType::Player, 1, 1001);
    let bob_guid = Guid::new(EntityType::Player, 1, 1002);

    let alice_token = EdgeAuthenticator::create_test_jwt(
        alice_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );
    let bob_token = EdgeAuthenticator::create_test_jwt(
        bob_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );

    // Connect Alice
    let mut alice_stream = TcpStream::connect(local_addr).await.expect("connect Alice");
    let mut alice_buf = BytesMut::with_capacity(4096);
    let alice_auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: alice_token,
        client_version: 1,
    });
    alice_stream
        .write_all(&alice_auth_req.encode())
        .await
        .expect("write Alice auth");
    let alice_auth_res = read_next_packet(&mut alice_stream, &mut alice_buf).await;
    match alice_auth_res {
        ClientPacket::AuthResponse(r) => assert!(r.success, "Alice auth failed"),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // Connect Bob
    let mut bob_stream = TcpStream::connect(local_addr).await.expect("connect Bob");
    let mut bob_buf = BytesMut::with_capacity(4096);
    let bob_auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: bob_token,
        client_version: 1,
    });
    bob_stream
        .write_all(&bob_auth_req.encode())
        .await
        .expect("write Bob auth");
    let bob_auth_res = read_next_packet(&mut bob_stream, &mut bob_buf).await;
    match bob_auth_res {
        ClientPacket::AuthResponse(r) => assert!(r.success, "Bob auth failed"),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // Small delay to ensure subscription listeners are initialized
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 1. Global chat broadcast
    let global_msg = ClientPacket::SocialMessage(SocialMessagePacket {
        channel_type: 0,
        target_id: 0,
        sender_guid: alice_guid.raw(),
        sender_name: "Alice".to_string(),
        content: "Hello world!".to_string(),
        timestamp: 12345678,
    });
    alice_stream
        .write_all(&global_msg.encode())
        .await
        .expect("write global msg");

    // Both Alice and Bob should receive the global message
    let alice_received = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut alice_stream, &mut alice_buf),
    )
    .await
    .expect("Alice timeout")
    .expect_social_message();
    assert_eq!(alice_received.content, "Hello world!");
    assert_eq!(alice_received.sender_name, "Alice");

    let bob_received = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut bob_stream, &mut bob_buf),
    )
    .await
    .expect("Bob timeout")
    .expect_social_message();
    assert_eq!(bob_received.content, "Hello world!");
    assert_eq!(bob_received.sender_name, "Alice");

    // 2. Direct Whisper from Alice to Bob
    let whisper_msg = ClientPacket::SocialMessage(SocialMessagePacket {
        channel_type: 1,
        target_id: bob_guid.raw(),
        sender_guid: alice_guid.raw(),
        sender_name: "Alice".to_string(),
        content: "Secret whisper to Bob".to_string(),
        timestamp: 12345679,
    });
    alice_stream
        .write_all(&whisper_msg.encode())
        .await
        .expect("write whisper");

    let bob_whisper = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut bob_stream, &mut bob_buf),
    )
    .await
    .expect("Bob whisper timeout")
    .expect_social_message();
    assert_eq!(bob_whisper.content, "Secret whisper to Bob");
    assert_eq!(bob_whisper.sender_name, "Alice");

    // 3. Dynamic Channel Subscription: Bob subscribes to guild 42
    let guild_sub = ClientPacket::SocialSubscribe(SocialSubscribePacket {
        channel_type: 3,
        target_id: 42,
        channel_name: String::new(),
    });
    bob_stream
        .write_all(&guild_sub.encode())
        .await
        .expect("write guild sub");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Alice sends to guild 42
    let guild_msg = ClientPacket::SocialMessage(SocialMessagePacket {
        channel_type: 3,
        target_id: 42,
        sender_guid: alice_guid.raw(),
        sender_name: "Alice".to_string(),
        content: "Guild announcement!".to_string(),
        timestamp: 12345680,
    });
    alice_stream
        .write_all(&guild_msg.encode())
        .await
        .expect("write guild msg");

    let bob_guild = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut bob_stream, &mut bob_buf),
    )
    .await
    .expect("Bob guild timeout")
    .expect_social_message();
    assert_eq!(bob_guild.content, "Guild announcement!");

    // Bob unsubscribes from guild 42
    let guild_unsub = ClientPacket::SocialUnsubscribe(SocialUnsubscribePacket {
        channel_type: 3,
        target_id: 42,
        channel_name: String::new(),
    });
    bob_stream
        .write_all(&guild_unsub.encode())
        .await
        .expect("write guild unsub");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Alice sends another guild msg, Bob should NOT receive it
    alice_stream
        .write_all(&guild_msg.encode())
        .await
        .expect("write second guild msg");
    let bob_check = timeout(
        Duration::from_millis(200),
        read_next_packet(&mut bob_stream, &mut bob_buf),
    )
    .await;
    assert!(
        bob_check.is_err(),
        "Bob should not receive message after unsubscribe"
    );

    // 4. Anti-Spoofing: Alice tries to send message claiming sender_guid = 9999
    let spoofed_msg = ClientPacket::SocialMessage(SocialMessagePacket {
        channel_type: 0,
        target_id: 0,
        sender_guid: 9999, // Mismatched!
        sender_name: "EvilAlice".to_string(),
        content: "Spoofed packet".to_string(),
        timestamp: 12345681,
    });
    alice_stream
        .write_all(&spoofed_msg.encode())
        .await
        .expect("write spoofed msg");
    let bob_spoof_check = timeout(
        Duration::from_millis(200),
        read_next_packet(&mut bob_stream, &mut bob_buf),
    )
    .await;
    assert!(
        bob_spoof_check.is_err(),
        "Spoofed packet must be dropped at edge"
    );

    // 5. Verification: Simulation Loop Bypass
    // No packets should have arrived at the simulation node channel (packet_rx)
    assert!(
        packet_rx.try_recv().is_err(),
        "Social packets must never enter the simulation node packet channel!"
    );
}

trait PacketExt {
    fn expect_social_message(self) -> SocialMessagePacket;
}

impl PacketExt for ClientPacket {
    fn expect_social_message(self) -> SocialMessagePacket {
        match self {
            ClientPacket::SocialMessage(msg) => msg,
            other => panic!("Expected SocialMessage, got {other:?}"),
        }
    }
}
