# Implementation Tasks: Decoupled Social Pub/Sub and Asynchronous Marketplace

## 1. Core Abstractions & Schema Wire Codecs
- [x] 1.1 Define abstract social events & traits in `crates/core/src/social.rs` and export from `crates/core/src/lib.rs`.
- [x] 1.2 Implement wire codecs, opcodes, and packets for Social Pub/Sub (`SocialMessage = 0x0013`, `SocialSubscribe = 0x0014`, `SocialUnsubscribe = 0x0015`) in `crates/schema/src/client.rs`.
- [x] 1.3 Implement wire codecs, opcodes, and packets for Marketplace (`MarketListRequest = 0x0016`, `MarketBuyRequest = 0x0017`, `MarketCancelRequest = 0x0018`, `MarketQueryRequest = 0x0019`, `MarketResponse = 0x001A`) in `crates/schema/src/client.rs`.
- [x] 1.4 Add unit and property tests for all new wire packets in `crates/schema`.

## 2. Storage Economic Primitives & Marketplace Service
- [x] 2.1 Add `market_listings` table migration and indexes to `crates/storage/src/db.rs`.
- [x] 2.2 Implement `MarketplaceRepository` and transactional trade/auction primitives in `crates/storage/src/marketplace.rs`.
- [x] 2.3 Implement atomic listing lifecycle with ownership verification: validate container ownership against player GUID, move item to market escrow container and insert listing row in a single ACID transaction.
- [x] 2.4 Implement purchase with ACID settlement (with ascending container ID locking) and cancellation with atomic item return.
- [x] 2.5 Implement background expiration reaper loop in `MarketplaceService` to periodically query expired listings and refund items to sellers.
- [x] 2.6 Add unit and concurrency tests for `MarketplaceRepository` in `crates/storage`.

## 3. Edge Gateway Social Packet Interception & Subscription Fan-Out
- [x] 3.1 Implement `GatewaySubscriptionManager` in `crates/gateway/src/social.rs` with lock-isolation (no nested DashMap locks) and bounded fan-out channels.
- [x] 3.2 Update `GatewayServer` to intercept `SocialMessage`, `SocialSubscribe`, and `SocialUnsubscribe` packets in TCP, QUIC, and WebTransport streams, bypassing `packet_tx`.
- [x] 3.3 Add anti-spoofing identity validation ensuring `sender_guid` matches session authenticated GUID.
- [x] 3.4 Wire session disconnect cleanup in `SessionDropGuard` to deregister all topic subscriptions cleanly without lock inversion.

## 4. Edge Gateway Marketplace Service Integration
- [x] 4.1 Implement `GatewayMarketplaceService` in `crates/gateway/src/marketplace.rs` interfacing `GatewayServer` directly with `MarketplaceRepository`.
- [x] 4.2 Intercept `MarketListRequest`, `MarketBuyRequest`, `MarketCancelRequest`, and `MarketQueryRequest` in gateway connection loops, executing asynchronously and returning `MarketResponse` via `SessionSink.stream_tx`.
- [x] 4.3 Verify that marketplace operations do not enqueue to `packet_tx` or interact with `mmo-node`.

## 5. End-to-End Integration Tests & Verification
- [x] 5.1 Implement E2E integration test `gateway_social_pubsub_test.rs` validating cross-gateway whispers, guild chat, and simulation loop bypass.
- [x] 5.2 Implement E2E integration test `gateway_marketplace_test.rs` validating listing, atomic buy, cancel, query, and simulation loop bypass.
- [x] 5.3 Run quality gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
