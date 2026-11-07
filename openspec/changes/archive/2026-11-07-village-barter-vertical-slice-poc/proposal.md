# Playable Village & Barter Vertical Slice PoC Proposal

## Context & Problem Statement
All 15 foundational engine, storage, networking, spatial, and verification milestones (`openspec/ROADMAP.md` Phases 1–4) are complete. However, the runnable daemon binaries (`mmo-gateway`, `mmo-node`) and the browser frontend (`web/`) are not yet wired into a cohesive, playable end-to-end game:

1. **Gateway WebTransport Daemon & Dev/PoC Handshake Gap**:
   - `crates/gateway/src/main.rs` only binds the TCP listener (`0.0.0.0:8080`) and does not start `WebTransportServer` (`0.0.0.0:4433`) or expose its self-signed ECDSA SHA-256 certificate digest (`cert_digest`) for browser clients.
   - `WebTransportServer::handle_session` requires a guest/PoC authentication fallback when `EdgeAuthenticator` is not enforcing external JWTs so browser clients receive an `AuthResponsePacket` containing their assigned `session_id` and `player_guid` and have their route registered in `SessionRouter`.

2. **Node Session Auto-Hydration & Decoupled Village World Bootstrap Gap**:
   - `NodeServiceImpl::forward_client_packet` in `crates/node/src/network/service.rs` drops packets if `sim.entity_store.contains_guid(guid)` is false, without invoking `TickSimulation::hydrate_player_session` when a newly authenticated player session sends its first packet.
   - `mmo-node` is strictly genre-agnostic (`crates/node/Cargo.toml` must not depend on `mmo-gameplay`). To run a playable Village PoC without violating the `AGENTS.md` Prime Directive, the node daemon loop needs a reusable runner entry point in `mmo-node` that a gameplay binary (`mmo-village-node` in `crates/gameplay`) can launch with `InventoryPlugin`, `NpcMovementPlugin`, `TradePlugin`, and a new `VillagePlugin`.

3. **Village NPC Barter & Player-to-Player Trade Protocol Wiring Gap**:
   - `crates/gameplay/src/trade.rs` implements `TradeManager::execute_atomic_swap` and `crates/gameplay/src/inventory.rs` implements `InventoryComponent`, but `TradePlugin` does not yet register a `ModularPacketHandler` (`module_id = 0x02`) to handle client-initiated inventory inspection, NPC villager barter exchanges, and player-to-player trade session steps over `ClientPacket::Interaction` / `ClientPacket::InteractionResult`.

4. **Browser WASM & Village Trade UI Gap (`crates/client-wasm` & `web/`)**:
   - `web/src/main.js` currently uses a minimal mock `ClientWorldBridge` with a hardcoded `localPlayerGuid = 1` and has no UI for viewing the player's inventory/coins, interacting with Village NPCs to barter items, or trading with other players.

## Proposed Solution

1. **Village Gameplay Plugin & Barter/Trade Packet Handler (`crates/gameplay/src/village.rs` & `trade.rs`)**:
   - Implement `VillagePlugin` and `VillagerMerchantComponent` in `crates/gameplay/src/village.rs`:
     - Spawns a village scene containing static/interactive `WorldObject` structures (Village Well, Blacksmith Forge, Herbalist Stall, Village Houses with collision bounds) and 3 NPC villagers (`Blacksmith Ilmari`, `Herbalist Aino` with patrol waypoints, and `Elder Väinö`).
     - Hooks `Event::OnSpawn` to provision newly hydrated players with a starter `InventoryComponent` (`50` Gold Coins, `3x Iron Ore`, `5x Wild Herbs`, `2x Firewood`).
     - Registers `VillageTradePacketHandler` on `module_id = 0x02` supporting JSON-encoded sub-opcodes for querying inventory/target state (`sub_opcode = 1`), executing atomic NPC villager barter recipes (`sub_opcode = 2`), and driving two-player atomic `TradeManager` sessions (`sub_opcode = 3..=6`).
   - Add a `mmo-village-node` binary target in `crates/gameplay` (reusing `mmo_node::run_node_daemon`) so `mmo-node` library retains zero dependency on `mmo-gameplay`.

2. **Gateway WebTransport Daemon, Cert Hash Discovery & Node Auto-Hydration (`crates/gateway`, `crates/node`, `crates/core`)**:
   - Expose `TelemetryServer::with_metadata` (or `/cert-hash` HTTP endpoint with CORS on the admin/telemetry server) and start `WebTransportServer` on `WEBTRANSPORT_ADDR` (default `0.0.0.0:4433`) in `crates/gateway/src/main.rs`.
   - Support frictionless PoC admission in `WebTransportServer::handle_session`: when a client connects or sends `AuthRequest`, assign or validate the player `Guid`, register the route in `SessionRouter`, and reply with `AuthResponsePacket`.
   - In `NodeServiceImpl::forward_client_packet`, automatically call `sim.hydrate_player_session` (and dispatch `Event::OnSpawn`) when an authenticated player `Guid` is not yet present in `entity_store` or `aoi`.

3. **Browser Village Client, Inventory & Barter/Trade HUD (`crates/client-wasm` & `web/`)**:
   - Update `WasmClientWorld` (`crates/client-wasm/src/lib.rs`) to handle `ClientPacket::AuthResponse` (setting `session_id` and `local_player_id`) and store the latest `InteractionResult` payload for JS inspection.
   - Upgrade `web/` with an interactive Village HUD: automatic `/cert-hash` lookup, player inventory & coin purse panel, click/proximity target selection (NPC Villagers & other Players), NPC Barter modal, and Player-to-Player Trade modal.

## Scope & Non-Goals
- **In Scope**: `crates/core/src/telemetry.rs`, `crates/node/src/{lib.rs, main.rs, network/service.rs}`, `crates/gateway/src/{main.rs, webtransport.rs}`, `crates/gameplay/{Cargo.toml, src/lib.rs, src/village.rs}`, `crates/client-wasm/src/lib.rs`, `web/`, `Dockerfile`, and `k8s/`.
- **Non-Goals**: Complex combat skill trees, dungeon instancing, or voxel terrain modification in the Village PoC.
