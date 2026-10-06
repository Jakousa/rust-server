# Proposal: Decoupled Social Pub/Sub Event Bus and Asynchronous Global Marketplace Service

## Motivation

In the current MMO engine architecture:
1. Spatial 3D Octree partitions simulate entities at 20 Hz on authoritative `mmo-node` instances. However, non-spatial player interactions (global chat, whispers, party coordination, guild updates, and cross-node announcements) have no need for spatial indexing or 20 Hz physics ticks. Subjecting non-spatial communication to simulation tick scheduling introduces lock contention, CPU overhead, and unnecessary packet hops.
2. Similarly, global player economies—such as auction house listings, asynchronous trades, and marketplace queries—require atomic transactional guarantees over item containers and currency stores, rather than real-time spatial simulation. Routing economy operations through the 20 Hz tick loop degrades combat and movement predictability while risking physics loop stalls during heavy trading spikes.

To address **Issue #57** (Global Social & Pub/Sub Event Bus) and **Issue #55** (Asynchronous Global Marketplace Service), we decouple non-spatial social messaging and global marketplace operations into dedicated services terminated at `mmo-gateway` and integrated with `mmo-storage`, completely bypassing `mmo-node` spatial loops.

## Scope of Work

### 1. Global Social & Pub/Sub Event Bus (Issue #57)
- **Abstract Social Schemas (`crates/core`)**: Unopinionated abstractions for social channel topologies, envelopes, message payloads, and channel routing.
- **Wire Protocol (`crates/schema`)**: Binary packet framing and codecs for `SocialMessage` (opcode `0x0013`), `SocialSubscribe` (opcode `0x0014`), and `SocialUnsubscribe` (opcode `0x0015`).
- **Edge Gateway Interception (`crates/gateway`)**:
  - Direct packet interception in connection handlers (`handle_client`, `handle_quic_connection`, `WebTransportServer`).
  - Transparent publishing to the distributed/in-memory `SocialBus` without queuing to `packet_tx` (zero `mmo-node` involvement).
  - Dynamic subscription manager routing incoming social bus broadcasts to subscribed client sessions via `SessionSink`.
  - Automatic session whisper channel subscription upon client authentication.

### 2. Asynchronous Global Marketplace Service (Issue #55)
- **Economic Primitives & Repository (`crates/storage`)**:
  - `market_listings` database schema migration supporting generic listing, bidding/buyout, status tracking, and expiration.
  - `MarketplaceRepository` / `MarketplaceService` providing transactional atomic listing (with escrow backing), purchasing, cancellation, and queries.
  - Zero hardcoded game rules: items, prices, currencies, and slots are mathematically generalized identifiers and quantities (`AGENTS.md` Prime Directive).
- **Marketplace Wire Protocol & RPCs (`crates/schema`)**:
  - Binary framing for `MarketListRequest` (opcode `0x0016`), `MarketBuyRequest` (opcode `0x0017`), `MarketCancelRequest` (opcode `0x0018`), `MarketQueryRequest` (opcode `0x0019`), and `MarketResponse` (opcode `0x001A`).
  - gRPC `MarketplaceService` in `cluster.proto` enabling direct cluster microservice RPCs.
- **Gateway Marketplace Routing (`crates/gateway`)**:
  - Gateway routes `Market*` packets directly to the asynchronous `MarketplaceService` and sends responses back to client sessions via reliable stream sinks, completely bypassing `mmo-node`.

## Non-Goals
- Game-specific auction house tax rules, faction restrictions, or item rarity filtering in core crates (these belong in `crates/gameplay` plugins).
- Replacement of `mmo-node` authoritative combat and movement ticks; social and marketplace traffic solely runs alongside without impinging on them.
