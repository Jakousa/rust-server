# Playable Village & Barter Vertical Slice PoC Specification

## Purpose
Defines the end-to-end requirements for the playable Village & Barter Vertical Slice Proof-of-Concept (PoC), connecting browser WebTransport clients, `mmo-gateway`, authoritative simulation nodes, NPC villagers, and atomic barter/trade mechanics while preserving `mmo-node` genre-agnostic decoupling.

## Requirements

### Requirement: Genre-Agnostic Node Daemon Runner & Village World Bootstrap
`mmo-node` SHALL remain free of `mmo-gameplay` dependencies in `[dependencies]` while exposing a reusable daemon runner so `mmo-village-node` (in `crates/gameplay`) can bootstrap `InventoryPlugin`, `NpcMovementPlugin`, `TradePlugin`, and `VillagePlugin` with village `WorldObject` structures, patrolling/stationary NPC villagers, and starter player inventories.

#### Scenario: Spawning village world and provisioning new player starter inventory
- **GIVEN** a `TickSimulation` initialized with `InventoryPlugin`, `NpcMovementPlugin`, `TradePlugin`, and `VillagePlugin`
- **WHEN** `VillagePlugin::bootstrap_world` is called and a new `EntityType::Player` entity is hydrated/spawned
- **THEN** the simulation SHALL contain the village `WorldObject` structures and at least 3 NPC villagers with `VillagerMerchantComponent`, and the spawned player SHALL automatically receive a starter `InventoryComponent` containing currency and starter trade goods

### Requirement: Modular Packet Protocol for NPC Barter and Player-to-Player Trade (`module_id = 0x02`)
`VillagePlugin` SHALL register a `ModularPacketHandler` for `module_id = 0x02` handling inventory/target queries (`sub_opcode = 1`), atomic NPC barter exchanges (`sub_opcode = 2`), and two-player `TradeManager` session lifecycle operations (`sub_opcode = 3..=6`).

#### Scenario: Executing an atomic barter trade with a Villager NPC
- **GIVEN** a player within `5.0` meters of a Villager NPC (`VillagerMerchantComponent`) possessing the required input `ItemStack` or currency for a `BarterRecipe`
- **WHEN** the player dispatches an `InteractionPacket` to `module_id = 0x02`, `sub_opcode = 2` specifying the NPC `target_guid` and `recipe_index`
- **THEN** the handler SHALL atomically deduct the required items/currency, add the output `ItemStack`/currency to the player's `InventoryComponent`, and return an `InteractionResultPacket` containing the updated inventory state

#### Scenario: Executing a two-player atomic trade via modular packets
- **GIVEN** two players `player_a` and `player_b` within `5.0` meters of each other
- **WHEN** `player_a` initiates a trade (`sub_opcode = 3`), both players stage offers (`sub_opcode = 4`), and both lock and accept (`sub_opcode = 5`)
- **THEN** `TradeManager::execute_atomic_swap` SHALL swap the offered items and currency atomically between both players' `InventoryComponent`s

### Requirement: Gateway WebTransport Listener, Certificate Hash Discovery & Node Auto-Hydration
`mmo-gateway` SHALL bind a `WebTransportServer` UDP endpoint alongside TCP, expose its SHA-256 certificate digest via `/cert-hash` (with CORS headers), assign a valid player route on WebTransport session admission, and `NodeServiceImpl::forward_client_packet` SHALL auto-hydrate newly arriving player sessions.

#### Scenario: First forwarded packet auto-hydrates player session on node
- **GIVEN** an authoritative `NodeServiceImpl` and a newly connected `session_id` with a `Player` `Guid` not yet present in `EntityStore`
- **WHEN** `forward_client_packet` receives a valid client packet for that `session_id` and `Guid`
- **THEN** the node SHALL invoke `hydrate_player_session` (spawning the entity, registering the AoI observer, and dispatching `Event::OnSpawn`) before processing the packet
