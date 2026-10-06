//! End-to-End Gateway Marketplace and Simulation Bypass Integration Tests
//!
//! Verifies:
//! 1. Listing creation directly via gateway TCP connection (`MarketListRequest`).
//! 2. Active listings query via gateway TCP connection (`MarketQueryRequest`).
//! 3. Atomic purchasing via gateway TCP connection (`MarketBuyRequest`).
//! 4. Listing cancellation via gateway TCP connection (`MarketCancelRequest`).
//! 5. Error propagation and unauthorized access rejection.
//! 6. Complete bypass of simulation loops (no marketplace packets enter packet_tx).

use bytes::BytesMut;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use mmo_core::guid::{EntityType, Guid};
use mmo_gateway::auth::EdgeAuthenticator;
use mmo_gateway::marketplace::GatewayMarketplaceService;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_schema::client::{
    market_error_codes, AuthRequestPacket, ClientPacket, MarketBuyRequestPacket,
    MarketCancelRequestPacket, MarketListRequestPacket, MarketQueryRequestPacket,
    MarketResponsePacket,
};
use mmo_storage::container_repo::ContainerRepository;
use mmo_storage::db::Database;
use mmo_storage::models::{ContainerRecord, ItemRecord};
use mmo_storage::{CharacterRepository, MarketplaceRepository};

const TEST_SECRET: &[u8] = b"marketplace_integration_test_secret_123456789";

async fn read_next_packet(stream: &mut TcpStream, buf: &mut BytesMut) -> ClientPacket {
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pkt) = ClientPacket::decode_frame(buf).expect("decode frame") {
            return pkt;
        }
        let n = stream.read(&mut chunk).await.expect("read chunk");
        assert!(n > 0, "Unexpected EOF reading packet");
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn create_container(db: &Database, container_id: u64, owner_id: &str, capacity: u32) {
    let rec = ContainerRecord {
        container_id,
        owner_type: "player".into(),
        owner_id: owner_id.into(),
        name: "Inventory".into(),
        capacity,
        access_policy: "private".into(),
        version: 1,
        created_at: 100,
        updated_at: 100,
    };
    ContainerRepository::new(db.clone())
        .create_container(&rec)
        .await
        .unwrap();
}

async fn create_item(db: &Database, item_id: &str, container_id: u64, slot: u32, item_def_id: u32) {
    let item = ItemRecord::builder(item_id)
        .container_id(container_id)
        .slot(slot)
        .item_def_id(item_def_id)
        .build();
    CharacterRepository::new(db.clone())
        .add_item(&item)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_marketplace_e2e_lifecycle_and_simulation_bypass() {
    let db = Database::connect_memory().await.unwrap();
    let mkt_repo = Arc::new(MarketplaceRepository::new(db.clone()));
    let escrow_container_id = 999_999;
    let mkt_service = Arc::new(
        GatewayMarketplaceService::new(mkt_repo).with_escrow_container_id(escrow_container_id),
    );

    let seller_guid = Guid::new(EntityType::Player, 1, 2001);
    let buyer_guid = Guid::new(EntityType::Player, 1, 2002);
    let seller_container = 100;
    let buyer_container = 200;
    let payment_container = 201;

    create_container(&db, seller_container, &seller_guid.raw().to_string(), 10).await;
    create_container(&db, escrow_container_id, "system", 100).await;
    create_container(&db, buyer_container, &buyer_guid.raw().to_string(), 10).await;
    create_container(&db, payment_container, &buyer_guid.raw().to_string(), 10).await;

    create_item(&db, "item-sword-42", seller_container, 0, 777).await;

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(100);

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
            .with_marketplace(mkt_service)
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

    let seller_token = EdgeAuthenticator::create_test_jwt(
        seller_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );
    let buyer_token = EdgeAuthenticator::create_test_jwt(
        buyer_guid,
        TEST_SECRET,
        3600,
        Some("mmo-auth"),
        Some("mmo-gateway"),
    );

    // 1. Connect Seller
    let mut seller_stream = TcpStream::connect(local_addr)
        .await
        .expect("connect seller");
    let mut seller_buf = BytesMut::with_capacity(4096);
    let seller_auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: seller_token,
        client_version: 1,
    });
    seller_stream
        .write_all(&seller_auth_req.encode())
        .await
        .expect("write seller auth");
    let seller_auth_res = read_next_packet(&mut seller_stream, &mut seller_buf).await;
    match seller_auth_res {
        ClientPacket::AuthResponse(r) => assert!(r.success, "Seller auth failed"),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // 2. Connect Buyer
    let mut buyer_stream = TcpStream::connect(local_addr).await.expect("connect buyer");
    let mut buyer_buf = BytesMut::with_capacity(4096);
    let buyer_auth_req = ClientPacket::AuthRequest(AuthRequestPacket {
        token: buyer_token,
        client_version: 1,
    });
    buyer_stream
        .write_all(&buyer_auth_req.encode())
        .await
        .expect("write buyer auth");
    let buyer_auth_res = read_next_packet(&mut buyer_stream, &mut buyer_buf).await;
    match buyer_auth_res {
        ClientPacket::AuthResponse(r) => assert!(r.success, "Buyer auth failed"),
        other => panic!("Expected AuthResponse, got {other:?}"),
    }

    // 3. Seller creates a listing
    let list_req = ClientPacket::MarketListRequest(MarketListRequestPacket {
        item_instance_id: "item-sword-42".to_string(),
        src_container_id: seller_container,
        src_slot: 0,
        price: 250,
        duration_secs: 3600,
    });
    seller_stream
        .write_all(&list_req.encode())
        .await
        .expect("write list req");

    let list_res = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut seller_stream, &mut seller_buf),
    )
    .await
    .expect("list timeout")
    .expect_market_response();

    assert!(list_res.success);
    assert_eq!(list_res.request_type, 1);
    assert_eq!(list_res.listings.len(), 1);
    let listing_id = list_res.listings[0].listing_id.clone();
    assert_eq!(list_res.listings[0].price, 250);
    assert_eq!(list_res.listings[0].seller_guid, seller_guid.raw());

    // 4. Buyer queries marketplace listings
    let query_req = ClientPacket::MarketQueryRequest(MarketQueryRequestPacket {
        item_def_id: 777,
        min_price: 0,
        max_price: 500,
        offset: 0,
        limit: 10,
    });
    buyer_stream
        .write_all(&query_req.encode())
        .await
        .expect("write query req");

    let query_res = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut buyer_stream, &mut buyer_buf),
    )
    .await
    .expect("query timeout")
    .expect_market_response();

    assert!(query_res.success);
    assert_eq!(query_res.request_type, 4);
    assert_eq!(query_res.listings.len(), 1);
    assert_eq!(query_res.listings[0].listing_id, listing_id);

    // 5. Buyer purchases the listing
    let buy_req = ClientPacket::MarketBuyRequest(MarketBuyRequestPacket {
        listing_id: listing_id.clone(),
        buyer_container_id: buyer_container,
        payment_container_id: payment_container,
        price: 250,
    });
    buyer_stream
        .write_all(&buy_req.encode())
        .await
        .expect("write buy req");

    let buy_res = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut buyer_stream, &mut buyer_buf),
    )
    .await
    .expect("buy timeout")
    .expect_market_response();

    assert!(buy_res.success);
    assert_eq!(buy_res.request_type, 2);

    // Verify item is now in buyer container
    let crepo = ContainerRepository::new(db.clone());
    let buyer_items = crepo.get_container_items(buyer_container).await.unwrap();
    assert_eq!(buyer_items.len(), 1);
    assert_eq!(buyer_items[0].item_instance_id, "item-sword-42");

    // 6. Test Listing Cancellation
    // Create another item in seller container slot 0
    create_item(&db, "item-shield-99", seller_container, 0, 888).await;
    let list_req_2 = ClientPacket::MarketListRequest(MarketListRequestPacket {
        item_instance_id: "item-shield-99".to_string(),
        src_container_id: seller_container,
        src_slot: 0,
        price: 500,
        duration_secs: 3600,
    });
    seller_stream
        .write_all(&list_req_2.encode())
        .await
        .expect("write second list req");
    let list_res_2 = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut seller_stream, &mut seller_buf),
    )
    .await
    .expect("list 2 timeout")
    .expect_market_response();
    assert!(list_res_2.success);
    let listing_id_2 = list_res_2.listings[0].listing_id.clone();

    // Seller cancels listing
    let cancel_req = ClientPacket::MarketCancelRequest(MarketCancelRequestPacket {
        listing_id: listing_id_2.clone(),
        dst_container_id: seller_container,
    });
    seller_stream
        .write_all(&cancel_req.encode())
        .await
        .expect("write cancel req");
    let cancel_res = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut seller_stream, &mut seller_buf),
    )
    .await
    .expect("cancel timeout")
    .expect_market_response();
    assert!(cancel_res.success);
    assert_eq!(cancel_res.request_type, 3);

    // Verify item refunded to seller container
    let seller_items = crepo.get_container_items(seller_container).await.unwrap();
    assert_eq!(seller_items.len(), 1);
    assert_eq!(seller_items[0].item_instance_id, "item-shield-99");

    // 7. Test Unauthorized Cancellation: Buyer tries to cancel Seller's listing
    // Create another listing for seller
    let list_req_3 = ClientPacket::MarketListRequest(MarketListRequestPacket {
        item_instance_id: "item-shield-99".to_string(),
        src_container_id: seller_container,
        src_slot: 0,
        price: 600,
        duration_secs: 3600,
    });
    seller_stream
        .write_all(&list_req_3.encode())
        .await
        .expect("write list 3");
    let list_res_3 = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut seller_stream, &mut seller_buf),
    )
    .await
    .expect("list 3 timeout")
    .expect_market_response();
    assert!(list_res_3.success);
    let listing_id_3 = list_res_3.listings[0].listing_id.clone();

    // Buyer attempts to cancel listing_id_3
    let unauthorized_cancel = ClientPacket::MarketCancelRequest(MarketCancelRequestPacket {
        listing_id: listing_id_3,
        dst_container_id: buyer_container,
    });
    buyer_stream
        .write_all(&unauthorized_cancel.encode())
        .await
        .expect("write unauthorized cancel");
    let unauth_res = timeout(
        Duration::from_secs(2),
        read_next_packet(&mut buyer_stream, &mut buyer_buf),
    )
    .await
    .expect("unauthorized cancel timeout")
    .expect_market_response();
    assert!(!unauth_res.success);
    assert_eq!(unauth_res.error_code, market_error_codes::UNAUTHORIZED);

    // 8. Verification: Simulation Loop Bypass
    // No packets should have arrived at the simulation node channel (packet_rx)
    assert!(
        packet_rx.try_recv().is_err(),
        "Marketplace packets must never enter the simulation node packet channel!"
    );
}

trait MarketPacketExt {
    fn expect_market_response(self) -> MarketResponsePacket;
}

impl MarketPacketExt for ClientPacket {
    fn expect_market_response(self) -> MarketResponsePacket {
        match self {
            ClientPacket::MarketResponse(res) => res,
            other => panic!("Expected MarketResponse, got {other:?}"),
        }
    }
}
