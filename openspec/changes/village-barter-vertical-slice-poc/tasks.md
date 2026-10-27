# Implementation Tasks: Playable Village & Barter Vertical Slice PoC

- [x] 1. Village Gameplay Plugin, NPCs & Barter/Trade Packet Handler (`crates/gameplay`)
  - [x] 1.1 Create `crates/gameplay/src/village.rs` implementing `BarterRecipe`, `VillagerMerchantComponent`, `PlayerStarterKitHook`, `VillageTradePacketHandler` (`module_id = 0x02`, sub-opcodes `1..=6`), `VillagePlugin`, and `bootstrap_village_world`.
  - [x] 1.2 Export the `village` module from `crates/gameplay/src/lib.rs` and write unit/integration tests verifying starter inventory provisioning, NPC barter exchanges, distance checks, and two-player trade execution via `InteractionPacket`.

- [x] 2. Reusable Node Daemon, Auto-Hydration & `mmo-village-node` Binary (`crates/node`, `crates/gameplay`)
  - [x] 2.1 Extract `run_node_daemon_with_sim` in `crates/node/src/lib.rs` and update `crates/node/src/main.rs` to delegate to it while keeping `mmo-node` `[dependencies]` free of `mmo-gameplay`.
  - [x] 2.2 Update `NodeServiceImpl::forward_client_packet` in `crates/node/src/network/service.rs` to auto-hydrate newly arriving `EntityType::Player` sessions (`hydrate_player_session` + `Event::OnSpawn` dispatch with entities).
  - [x] 2.3 Add `[[bin]] name = "mmo-village-node"` in `crates/gameplay/Cargo.toml` (`crates/gameplay/src/bin/mmo_village_node.rs`) bootstrapping the village world and launching the node daemon.

- [x] 3. Gateway WebTransport Daemon, `/cert-hash` Endpoint & PoC Auth Handshake (`crates/core`, `crates/gateway`)
  - [x] 3.1 Add `set_cert_hash` and `GET /cert-hash` (with CORS `Access-Control-Allow-Origin: *`) to `TelemetryServer` in `crates/core/src/telemetry.rs`.
  - [x] 3.2 Update `WebTransportServer::handle_session` in `crates/gateway/src/webtransport.rs` so unauthenticated/PoC mode registers a default player route (`Guid::new(EntityType::Player, 1, session_id as u32)`) and responds to `ClientPacket::AuthRequest` with `ClientPacket::AuthResponse`.
  - [x] 3.3 Wire `WebTransportServer` (`WEBTRANSPORT_ADDR`, default `0.0.0.0:4433`) and `GatewayInterceptHandler` into `crates/gateway/src/main.rs` and expose UDP port `4433` in `Dockerfile` and `k8s/`.

- [x] 4. Browser WASM Client & Village Inventory/Barter UI (`crates/client-wasm`, `web/`)
  - [x] 4.1 Update `WasmClientWorld` in `crates/client-wasm/src/lib.rs` to handle `AuthResponse`, store `InteractionResult` payloads (`take_last_interaction_result`), and encode custom interactions (`encode_custom_interaction`).
  - [x] 4.2 Update `web/index.html`, `web/style.css`, `web/src/renderer.js`, `web/src/ui.js`, and `web/src/main.js` with auto-fetched `/cert-hash`, `AuthRequest` handshake, Village prefab/NPC rendering, Inventory & Gold HUD, NPC Barter modal, and Player-to-Player Trade UI, plus Vitest tests in `web/src/__tests__/inspector.test.js`.

- [x] 5. Quality Gates & End-to-End Verification
  - [x] 5.1 Run `cargo fmt --all -- --check`
  - [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings`
  - [x] 5.3 Run `cargo test --workspace` and `cd web && npm test && npm run build`
