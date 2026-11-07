# Technical Design: Playable Village & Barter Vertical Slice PoC

## 1. Village Gameplay Plugin & Trade/Barter Protocol (`crates/gameplay/src/village.rs`)

### 1.1 Components & Recipes
Create `crates/gameplay/src/village.rs` and export from `crates/gameplay/src/lib.rs`:
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarterRecipe {
    pub recipe_id: u32,
    pub title: String,
    pub required_items: Vec<ItemStack>,
    pub required_currency: u64,
    pub reward_items: Vec<ItemStack>,
    pub reward_currency: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VillagerMerchantComponent {
    pub name: String,
    pub role: String,
    pub greeting: String,
    pub recipes: Vec<BarterRecipe>,
}
```

### 1.2 Village World Bootstrap (`bootstrap_village_world`)
Provide `pub fn bootstrap_village_world(sim: &mut mmo_node::TickSimulation)`:
1. **Install Plugins**:
   - `InventoryPlugin` (`module_id = 0x01`)
   - `NpcMovementPlugin` (`NavSteeringSystem`)
   - `TradePlugin`
   - `VillagePlugin` (`module_id = 0x02`)
2. **Spawn Static & Interactive Village `WorldObject`s**:
   - **Village Well** (`prefab_id = 10`, pos `(0.0, 0.0, 0.0)`, scale `(2.5, 2.0, 2.5)`).
   - **Blacksmith Forge** (`prefab_id = 11`, pos `(-12.0, 0.0, -8.0)`, scale `(5.0, 4.0, 5.0)`).
   - **Herbalist Stall** (`prefab_id = 12`, pos `(12.0, 0.0, -8.0)`, scale `(4.0, 3.0, 4.0)`).
   - **Village Houses** (`prefab_id = 13`, at `(-18.0, 0.0, 10.0)` and `(18.0, 0.0, 10.0)`, scale `(6.0, 5.0, 6.0)`).
3. **Spawn Villager NPCs (`EntityType::Npc`)**:
   - **Blacksmith Ilmari (`Guid::new(EntityType::Npc, sim.node_id, 1)`)** at `(-9.0, 0.0, -6.0)`:
     - Recipes:
       1. `2x Iron Ore (1001)` + `1x Firewood (1003)` -> `1x Iron Pickaxe (2001)`
       2. `1x Iron Ore (1001)` -> `15 Gold Coins`
       3. `25 Gold Coins` -> `1x Steel Sword (2002)`
   - **Herbalist Aino (`Guid::new(EntityType::Npc, sim.node_id, 2)`)** at `(9.0, 0.0, -6.0)` with `WaypointComponent` patrolling `(9.0, 0.0, -6.0)` <-> `(4.0, 0.0, -2.0)` <-> `(9.0, 0.0, 4.0)` (`NavAgentComponent::new(2.0, 0.3)`):
     - Recipes:
       1. `3x Wild Herbs (1002)` -> `1x Healing Potion (2003)`
       2. `1x Wild Herbs (1002)` -> `8 Gold Coins`
       3. `10 Gold Coins` -> `2x Wild Herbs (1002)`
   - **Elder Väinö (`Guid::new(EntityType::Npc, sim.node_id, 3)`)** at `(0.0, 0.0, -4.0)` with `WaypointComponent` patrolling around the Village Well:
     - Recipes:
       1. `1x Iron Pickaxe (2001)` + `1x Healing Potion (2003)` -> `1x Village Charter Seal (3001)` + `50 Gold Coins`
       2. `15 Gold Coins` -> `2x Iron Ore (1001)` + `1x Firewood (1003)`
4. **Player Starter Kit Hook (`PlayerStarterKitHook`)**:
   - Implements `HookHandler::handle_with_entities` for `Event::OnSpawn { entity, .. }`:
   - When `entity.entity_type() == EntityType::Player` and the entity lacks an `InventoryComponent`, attaches `InventoryComponent::new(12, 100.0).with_currency(50)` pre-populated with:
     - `ItemStack::new(1001, "Iron Ore", 3, 2.0)`
     - `ItemStack::new(1002, "Wild Herbs", 5, 0.5)`
     - `ItemStack::new(1003, "Firewood", 2, 1.5)`

### 1.3 `VillageTradePacketHandler` (`module_id = 0x02`)
Wraps `Mutex<TradeManager>` inside `VillageTradePacketHandler` and handles JSON requests/responses over `InteractionPacket` (`action_id = (0x0002 << 16) | sub_opcode`):
- **`sub_opcode = 1` (`Inspect`)**:
  - Payload: empty or `{"target_guid": u64}`.
  - Returns JSON `VillageTradeStateResponse` containing the caller's `InventoryComponent` (slots, currency, weight), any active player-to-player `TradeSessionView`, and if `target_guid` has `VillagerMerchantComponent`, the NPC's `VillagerMerchantComponent` details (or if `target_guid` is another player, their public trade status).
- **`sub_opcode = 2` (`NpcBarter`)**:
  - Payload: `{"target_guid": u64, "recipe_id": u32}`.
  - Validates 3D distance `<= TradeManager::MAX_TRADE_DISTANCE` (`5.0m`), verifies required items and currency on the player's `InventoryComponent`, removes required stacks, adds reward items/currency, checks `!is_encumbered()`, and returns updated `VillageTradeStateResponse`.
- **`sub_opcode = 3` (`PlayerTradeStart`)**:
  - Payload: `{"target_guid": u64}`.
  - Calls `TradeManager::start_session(entity_guid, target_guid, entities)` and returns `VillageTradeStateResponse`.
- **`sub_opcode = 4` (`PlayerTradeOffer`)**:
  - Payload: `{"session_id": u64, "items": Vec<ItemStack>, "currency": u64}`.
  - Calls `session.set_offer(entity_guid, TradeOffer { items, currency })` and returns `VillageTradeStateResponse`.
- **`sub_opcode = 5` (`PlayerTradeAccept`)**:
  - Payload: `{"session_id": u64}`.
  - Calls `session.lock(entity_guid)` and `session.accept(entity_guid)`. If both players accepted (`Ok(true)`), invokes `trade_mgr.execute_atomic_swap(session_id, entities)` and returns `VillageTradeStateResponse` with `trade_completed: true`.
- **`sub_opcode = 6` (`PlayerTradeCancel`)**:
  - Payload: `{"session_id": u64}`.
  - Cancels the active session via `trade_mgr.cancel_session(session_id)`.

---

## 2. Node Daemon Runner, Auto-Hydration & Gateway WebTransport Wiring

### 2.1 Reusable Node Daemon (`crates/node/src/lib.rs` & `crates/gameplay/src/bin/mmo-village-node.rs`)
- Extract the runtime loop from `crates/node/src/main.rs` into `pub async fn run_node_daemon_with_sim(node_id: u16, bounds: BoundingBox3D, tick_sim: TickSimulation) -> Result<(), Box<dyn std::error::Error>>` in `crates/node/src/lib.rs`.
- Add `[[bin]] name = "mmo-village-node"` in `crates/gameplay/Cargo.toml` (`crates/gameplay/src/bin/mmo_village_node.rs`) that creates `TickSimulation`, calls `bootstrap_village_world(&mut tick_sim)`, and runs `run_node_daemon_with_sim`.
- In `crates/node/src/network/service.rs` (`NodeServiceImpl::forward_client_packet`):
  - Before rejecting an unknown `guid` with `"Entity not found on node"`, if `guid.entity_type() == mmo_core::EntityType::Player` and `!sim.entity_store.contains_guid(guid)`:
    - Call `let pos = sim.hydrate_player_session(guid, req.session_id, req.session_epoch, None, 80.0);`
    - Dispatch `sim.hooks.dispatch_with_entities(&mmo_core::Event::OnSpawn { entity: guid, position: pos }, &mut sim.entity_store);`

### 2.2 Gateway WebTransport Listener & `/cert-hash` Endpoint (`crates/gateway` & `crates/core`)
- In `crates/core/src/telemetry.rs`:
  - Add `pub fn set_cert_hash(&self, hash: impl Into<String>)` to `TelemetryServer` and serve `GET /cert-hash` with `Access-Control-Allow-Origin: *` returning `{"cert_hash": "..."}` so browser clients can automatically fetch the ephemeral WebTransport certificate hash.
- In `crates/gateway/src/webtransport.rs` (`WebTransportServer::handle_session`):
  - When `authenticator.is_none()`, register a default player route on session start (`Guid::new(EntityType::Player, 1, session_id as u32)`) if `default_node_address` is set, AND if the client sends `ClientPacket::AuthRequest` on a bidirectional stream, reply with `ClientPacket::AuthResponse(AuthResponsePacket { success: true, session_id, player_guid: guid.raw(), assigned_node, .. })`.
- In `crates/gateway/src/main.rs`:
  - Read `WEBTRANSPORT_ADDR` (default `"0.0.0.0:4433"`), bind `WebTransportServer`, register its `cert_digest()` on `telemetry.set_cert_hash(...)`, log the SHA-256 fingerprint to stdout, and spawn `wt_server.run_background()`.

---

## 3. Browser WASM & Village UI (`crates/client-wasm` & `web/`)

1. **`crates/client-wasm/src/lib.rs`**:
   - Handle `ClientPacket::AuthResponse(resp)` in `handle_parsed_packet`: when `resp.success`, set `self.session_id = resp.session_id` and `self.local_player_id = Some(resp.player_guid)`.
   - In `ClientPacket::InteractionResult(res)`, store `last_interaction_result: Option<String>` (UTF-8 JSON payload) and expose `pub fn take_last_interaction_result(&mut self) -> Option<String>` and `pub fn encode_custom_interaction(&self, target_guid: u64, action_id: u32, payload: &[u8]) -> Vec<u8>`.
2. **`web/` Village Frontend**:
   - Add NPC label/role rendering and distinct visual meshes in `web/src/renderer.js` (`entityType === 2` for Villager NPCs with golden merchant marker, `WorldObjectRenderer` prefabs `10..=13` for Well, Forge, Stall, Houses).
   - Add Inventory & Village Trade HUD panels in `web/index.html`, `web/style.css`, and `web/src/ui.js`:
     - Shows player Gold Coins, Weight, and Inventory Items.
     - Clicking a nearby NPC or pressing `E` queries `module_id = 0x02, sub_opcode = 1` and displays the NPC's greeting and clickable **Barter** buttons (`sub_opcode = 2`).
     - Clicking a nearby Player opens the **Player Trade** panel (`sub_opcode = 3..=6`) to offer items/gold, lock, and accept.
