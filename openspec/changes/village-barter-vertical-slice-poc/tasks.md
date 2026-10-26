# Implementation Tasks: Playable Village & Barter Vertical Slice PoC

- [ ] 1. Village Gameplay Plugin, NPCs & Barter/Trade Packet Handler (`crates/gameplay`)
  - [ ] 1.1 Create `crates/gameplay/src/village.rs` implementing `BarterRecipe`, `VillagerMerchantComponent`, `PlayerStarterKitHook`, `VillageTradePacketHandler` (`module_id = 0x02`, sub-opcodes `1..=6`), `VillagePlugin`, and `bootstrap_village_world`.
  - [ ] 1.2 Export the `village` module from `crates/gameplay/src/lib.rs` and write unit/integration tests verifying starter inventory provisioning, NPC barter exchanges, distance checks, and two-player trade execution via `InteractionPacket`.

- [ ] 2. Reusable Node Daemon, Auto-Hydration & `mmo-village-node` Binary (`crates/node`, `crates/gameplay`)
  - [ ] 2.1 Extract `run_node_daemon_with_sim` in `crates/node/src/lib.rs` and update `crates/node/src/main.rs` to delegate to it while keeping `mmo-node` `[dependencies]` free of `mmo-gameplay`.
  - [ ] 2.2 Update `NodeServiceImpl::forward_client_packet` in `crates/node/src/network/service.rs` to auto-hydrate newly arriving `EntityType::Player` sessions (`hydrate_player_session` + `Event::OnSpawn` dispatch with entities).
  - [ ] 2.3 Add `[[bin]] name = "mmo-village-node"` in `crates/gameplay/Cargo.toml` (`crates/gameplay/src/bin/mmo_village_node.rs`) bootstrapping the village world and launching the node daemon.

- [ ] 3. Gateway WebTransport Daemon, `/cert-hash` Endpoint & PoC Auth Handshake (`crates/core`, `crates/gateway`)
  - [ ] 3.1 Add `set_cert_hash` and `GET /cert-hash` (with CORS `Access-Control-Allow-Origin: *`) to `TelemetryServer` in `crates/core/src/telemetry.rs`.
  - [ ] 3.2 Update `WebTransportServer::handle_session` in `crates/gateway/src/webtransport.rs` so unauthenticated/PoC mode registers a default player route (`Guid::new(EntityType::Player, 1, session_id as u32)`) and responds to `ClientPacket::AuthRequest` with `ClientPacket::AuthResponse`.
  - [ ] 3.3 Wire `WebTransportServer` (`WEBTRANSPORT_ADDR`, default `0.0.0.0:4433`) and `GatewayInterceptHandler` into `crates/gateway/src/main.rs` and expose UDP port `4433` in `Dockerfile` and `k8s/`.

- [ ] 4. Browser WASM Client & Village Inventory/Barter UI (`crates/client-wasm`, `web/`)
  - [ ] 4.1 Update `WasmClientWorld` in `crates/client-wasm/src/lib.rs` to handle `AuthResponse`, store `InteractionResult` payloads (`take_last_interaction_result`), and encode custom interactions (`encode_custom_interaction`).
  - [ ] 4.2 Update `web/index.html`, `web/style.css`, `web/src/renderer.js`, `web/src/ui.js`, and `web/src/main.js` with auto-fetched `/cert-hash`, `AuthRequest` handshake, Village prefab/NPC rendering, Inventory & Gold HUD, NPC Barter modal, and Player-to-Player Trade UI, plus Vitest tests in `web/src/__tests__/inspector.test.js`.

- [ ] 5. Quality Gates & End-to-End Verification
  - [ ] 5.1 Run `cargo fmt --all -- --check`
  - [ ] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings`
  - [ ] 5.3 Run `cargo test --workspace` and `cd web && npm test && npm run build`
