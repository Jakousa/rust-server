//! Village World, Villager NPCs, Starter Kits, and Barter/Trade Packet Handler.
//!
//! Provides the authoritative gameplay plugin and world bootstrap for the Village Vertical Slice:
//! - Static and interactive village [`WorldObject`] structures (Well, Forge, Herbalist Stall, Houses).
//! - 3 NPC villagers: Blacksmith Ilmari, Herbalist Aino (with patrol waypoints), and Elder Väinö.
//! - [`PlayerStarterKitHook`] provisioning new players with starter inventories and coins.
//! - [`VillageTradePacketHandler`] implementing modular packet handling (`module_id = 0x02`)
//!   for inspecting trade state, atomic NPC barter exchanges, and two-player atomic trades.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use mmo_core::container::ContainerOwner;
use mmo_core::guid::{EntityType, Guid};
use mmo_core::hook::{Event, EventContext, HookHandler};
use mmo_core::octree::BoundingBox3D;
use mmo_core::plugin::{GameplayPlugin, ModularPacketHandler, PacketHandlerError, PluginContext};
use mmo_core::world::{Transform3D, WorldObject};
use mmo_core::EntityStore;

use crate::inventory::{InventoryComponent, InventoryPlugin, ItemStack};
use crate::navigation::{
    NavAgentComponent, NpcMovementPlugin, WaypointComponent, WaypointLoopMode,
};
use crate::trade::{TradeManager, TradeOffer, TradePlugin, TradeSession};

/// Recipe for bartering items and currency with a villager NPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarterRecipe {
    pub recipe_id: u32,
    pub title: String,
    pub required_items: Vec<ItemStack>,
    pub required_currency: u64,
    pub reward_items: Vec<ItemStack>,
    pub reward_currency: u64,
}

impl BarterRecipe {
    pub fn new(
        recipe_id: u32,
        title: impl Into<String>,
        required_items: Vec<ItemStack>,
        required_currency: u64,
        reward_items: Vec<ItemStack>,
        reward_currency: u64,
    ) -> Self {
        Self {
            recipe_id,
            title: title.into(),
            required_items,
            required_currency,
            reward_items,
            reward_currency,
        }
    }
}

/// Dynamic merchant component attached to villager NPCs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VillagerMerchantComponent {
    pub name: String,
    pub role: String,
    pub greeting: String,
    pub recipes: Vec<BarterRecipe>,
}

impl VillagerMerchantComponent {
    pub fn new(
        name: impl Into<String>,
        role: impl Into<String>,
        greeting: impl Into<String>,
        recipes: Vec<BarterRecipe>,
    ) -> Self {
        Self {
            name: name.into(),
            role: role.into(),
            greeting: greeting.into(),
            recipes,
        }
    }
}

/// Lifecycle hook that attaches a starter kit `InventoryComponent` to newly spawned player entities.
pub struct PlayerStarterKitHook;

impl HookHandler for PlayerStarterKitHook {
    fn handle(&self, _event: &Event, _ctx: &mut EventContext) {}

    fn handle_with_entities(
        &self,
        event: &Event,
        _ctx: &mut EventContext,
        entities: &mut EntityStore,
    ) {
        if let Event::OnSpawn { entity, .. } = event {
            if entity.entity_type() == EntityType::Player {
                if let Some(ent) = entities.get_mut_by_guid(*entity) {
                    if ent.get_component::<InventoryComponent>().is_none() {
                        let mut inv = InventoryComponent::new(12, 100.0).with_currency(50);
                        let _ = inv.add_item(ItemStack::new(1001, "Iron Ore", 3, 2.0));
                        let _ = inv.add_item(ItemStack::new(1002, "Wild Herbs", 5, 0.5));
                        let _ = inv.add_item(ItemStack::new(1003, "Firewood", 2, 1.5));
                        ent.add_component(inv);
                    }
                }
            }
        }
    }
}

/// JSON request payload for sub-opcode 1 (`Inspect`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InspectRequest {
    #[serde(default)]
    pub target_guid: Option<u64>,
}

/// JSON request payload for sub-opcode 2 (`NpcBarter`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpcBarterRequest {
    pub target_guid: u64,
    pub recipe_id: u32,
}

/// JSON request payload for sub-opcode 3 (`PlayerTradeStart`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerTradeStartRequest {
    pub target_guid: u64,
}

/// JSON request payload for sub-opcode 4 (`PlayerTradeOffer`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerTradeOfferRequest {
    pub session_id: u64,
    pub items: Vec<ItemStack>,
    pub currency: u64,
}

/// JSON request payload for sub-opcodes 5 & 6 (`PlayerTradeAccept` / `PlayerTradeCancel`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerTradeSessionRequest {
    pub session_id: u64,
}

/// Player-relative projection of an active two-player trade session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeSessionView {
    pub session_id: u64,
    pub player_a: u64,
    pub player_b: u64,
    pub my_offer: TradeOffer,
    pub their_offer: TradeOffer,
    pub my_locked: bool,
    pub their_locked: bool,
    pub my_accepted: bool,
    pub their_accepted: bool,
    pub completed: bool,
}

/// Comprehensive trade & inventory state response sent back to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VillageTradeStateResponse {
    pub player_guid: u64,
    pub inventory: Option<InventoryComponent>,
    pub current_weight: f32,
    pub is_encumbered: bool,
    pub active_trade: Option<TradeSessionView>,
    pub target_merchant: Option<VillagerMerchantComponent>,
    pub target_trade_status: Option<String>,
    #[serde(default)]
    pub trade_completed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Builds a player-centric `TradeSessionView` from an active `TradeSession`.
pub fn build_trade_session_view(session: &TradeSession, caller: Guid) -> TradeSessionView {
    let is_a = caller == session.player_a;
    let (my_offer, their_offer, my_locked, their_locked, my_accepted, their_accepted) = if is_a {
        (
            session.offer_a.clone(),
            session.offer_b.clone(),
            session.locked_a,
            session.locked_b,
            session.accepted_a,
            session.accepted_b,
        )
    } else {
        (
            session.offer_b.clone(),
            session.offer_a.clone(),
            session.locked_b,
            session.locked_a,
            session.accepted_b,
            session.accepted_a,
        )
    };

    TradeSessionView {
        session_id: session.session_id,
        player_a: session.player_a.raw(),
        player_b: session.player_b.raw(),
        my_offer,
        their_offer,
        my_locked,
        their_locked,
        my_accepted,
        their_accepted,
        completed: session.completed,
    }
}

/// Aggregates authoritative state for caller inventory, active trade, and target inspection.
pub fn build_state_response(
    caller_guid: Guid,
    entities: &EntityStore,
    trade_mgr: &TradeManager,
    target_guid: Option<Guid>,
    trade_completed: bool,
    error: Option<String>,
) -> VillageTradeStateResponse {
    let caller_ent = entities.get_by_guid(caller_guid);
    let inventory = caller_ent.and_then(|e| e.get_component::<InventoryComponent>().cloned());
    let current_weight = inventory
        .as_ref()
        .map(|i| i.current_weight())
        .unwrap_or(0.0);
    let is_encumbered = inventory
        .as_ref()
        .map(|i| i.is_encumbered())
        .unwrap_or(false);

    let active_trade = trade_mgr
        .get_session_by_player(caller_guid)
        .map(|s| build_trade_session_view(s, caller_guid));

    let mut target_merchant = None;
    let mut target_trade_status = None;

    if let Some(target) = target_guid {
        if let Some(target_ent) = entities.get_by_guid(target) {
            if let Some(merchant) = target_ent.get_component::<VillagerMerchantComponent>() {
                target_merchant = Some(merchant.clone());
            } else if target.entity_type() == EntityType::Player {
                let status = if let Some(session) = trade_mgr.get_session_by_player(target) {
                    if session.player_a == caller_guid || session.player_b == caller_guid {
                        "trading_with_you"
                    } else {
                        "busy"
                    }
                } else {
                    "available"
                };
                target_trade_status = Some(status.to_string());
            }
        }
    }

    VillageTradeStateResponse {
        player_guid: caller_guid.raw(),
        inventory,
        current_weight,
        is_encumbered,
        active_trade,
        target_merchant,
        target_trade_status,
        trade_completed,
        error,
    }
}

/// Hook handler that aborts active trade sessions if participants move too far apart or despawn.
pub struct TradeDistanceHook {
    pub trade_manager: Arc<std::sync::Mutex<TradeManager>>,
}

impl HookHandler for TradeDistanceHook {
    fn handle(&self, _event: &Event, _ctx: &mut EventContext) {}

    fn handle_with_entities(
        &self,
        event: &Event,
        _ctx: &mut EventContext,
        entities: &mut EntityStore,
    ) {
        let mut trade_mgr = self.trade_manager.lock().unwrap();
        match event {
            Event::BeforeMove { entity, to, .. } => {
                if let Some(session) = trade_mgr.get_session_by_player(*entity) {
                    let partner_guid = if session.player_a == *entity {
                        session.player_b
                    } else {
                        session.player_a
                    };
                    if let Some(partner) = entities.get_by_guid(partner_guid) {
                        let dx = to.0 - partner.position.0;
                        let dy = to.1 - partner.position.1;
                        let dz = to.2 - partner.position.2;
                        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                        if dist > TradeManager::MAX_TRADE_DISTANCE {
                            let session_id = session.session_id;
                            trade_mgr.cancel_session(session_id);
                        }
                    }
                }
            }
            Event::OnDespawn { entity } => {
                if let Some(session) = trade_mgr.get_session_by_player(*entity) {
                    let session_id = session.session_id;
                    trade_mgr.cancel_session(session_id);
                }
            }
            _ => {}
        }
    }
}

/// Modular packet handler for `module_id = 0x02`.
pub struct VillageTradePacketHandler {
    pub trade_manager: Arc<std::sync::Mutex<TradeManager>>,
}

impl VillageTradePacketHandler {
    pub fn new(trade_manager: Arc<std::sync::Mutex<TradeManager>>) -> Self {
        Self { trade_manager }
    }
}

impl ModularPacketHandler for VillageTradePacketHandler {
    fn handle(
        &self,
        entity_guid: Guid,
        sub_opcode: u16,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketHandlerError> {
        let mut trade_mgr = self.trade_manager.lock().unwrap();

        match sub_opcode {
            // Sub-opcode 1: Inspect trade state, inventory, and optional target
            1 => {
                let req: InspectRequest = if payload.is_empty() {
                    InspectRequest::default()
                } else {
                    serde_json::from_slice(payload).map_err(|e| {
                        PacketHandlerError::MalformedPayload {
                            module_id: VillagePlugin::MODULE_ID,
                            message: e.to_string(),
                        }
                    })?
                };

                let target = req.target_guid.filter(|&g| g != 0).map(Guid::from_raw);
                let resp =
                    build_state_response(entity_guid, entities, &trade_mgr, target, false, None);
                let json = serde_json::to_vec(&resp)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                Ok(Some(json))
            }

            // Sub-opcode 2: Atomic NPC barter exchange
            2 => {
                let req: NpcBarterRequest = serde_json::from_slice(payload).map_err(|e| {
                    PacketHandlerError::MalformedPayload {
                        module_id: VillagePlugin::MODULE_ID,
                        message: e.to_string(),
                    }
                })?;

                let target_guid = Guid::from_raw(req.target_guid);
                let (caller_pos, target_pos) = {
                    let caller = entities
                        .get_by_guid(entity_guid)
                        .ok_or(PacketHandlerError::EntityNotFound(entity_guid))?;
                    let target = entities
                        .get_by_guid(target_guid)
                        .ok_or(PacketHandlerError::EntityNotFound(target_guid))?;
                    (caller.position, target.position)
                };

                let dx = caller_pos.0 - target_pos.0;
                let dy = caller_pos.1 - target_pos.1;
                let dz = caller_pos.2 - target_pos.2;
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();

                if dist > TradeManager::MAX_TRADE_DISTANCE {
                    return Err(PacketHandlerError::Custom(format!(
                        "Target NPC is too far away ({:.1}m > {:.1}m)",
                        dist,
                        TradeManager::MAX_TRADE_DISTANCE
                    )));
                }

                let recipe = {
                    let target = entities.get_by_guid(target_guid).unwrap();
                    let merchant = target
                        .get_component::<VillagerMerchantComponent>()
                        .ok_or_else(|| {
                            PacketHandlerError::Custom(
                                "Target entity is not a villager merchant".to_string(),
                            )
                        })?;
                    merchant
                        .recipes
                        .iter()
                        .find(|r| r.recipe_id == req.recipe_id)
                        .cloned()
                        .ok_or_else(|| {
                            PacketHandlerError::Custom(format!(
                                "Recipe {} not found on merchant",
                                req.recipe_id
                            ))
                        })?
                };

                let caller = entities
                    .get_mut_by_guid(entity_guid)
                    .ok_or(PacketHandlerError::EntityNotFound(entity_guid))?;
                let inv = caller
                    .get_component_mut::<InventoryComponent>()
                    .ok_or_else(|| {
                        PacketHandlerError::Custom(
                            "Caller entity has no InventoryComponent".to_string(),
                        )
                    })?;

                if inv.currency < recipe.required_currency {
                    return Err(PacketHandlerError::Custom(format!(
                        "Insufficient currency (required {}, current {})",
                        recipe.required_currency, inv.currency
                    )));
                }

                for item in &recipe.required_items {
                    if !inv.has_item_stack(item) {
                        return Err(PacketHandlerError::Custom(format!(
                            "Missing required item: {} (quantity {})",
                            item.name, item.quantity
                        )));
                    }
                }

                // Atomic inventory mutation with full rollback guarantee
                let rollback = inv.clone();
                inv.currency = match inv.currency.checked_sub(recipe.required_currency) {
                    Some(c) => c,
                    None => {
                        *inv = rollback;
                        return Err(PacketHandlerError::Custom(
                            "Insufficient currency".to_string(),
                        ));
                    }
                };
                for item in &recipe.required_items {
                    if !inv.remove_item_stack(item) {
                        *inv = rollback;
                        return Err(PacketHandlerError::Custom(
                            "Failed to deduct required items".to_string(),
                        ));
                    }
                }

                inv.currency = match inv.currency.checked_add(recipe.reward_currency) {
                    Some(c) => c,
                    None => {
                        *inv = rollback;
                        return Err(PacketHandlerError::Custom("Currency overflow".to_string()));
                    }
                };
                for item in &recipe.reward_items {
                    if let Err(e) = inv.add_item(item.clone()) {
                        *inv = rollback;
                        return Err(PacketHandlerError::Custom(format!(
                            "Failed to add reward item: {}",
                            e
                        )));
                    }
                }

                if inv.is_encumbered() {
                    *inv = rollback;
                    return Err(PacketHandlerError::Custom(
                        "Barter transaction would cause inventory encumbrance".to_string(),
                    ));
                }

                let resp = build_state_response(
                    entity_guid,
                    entities,
                    &trade_mgr,
                    Some(target_guid),
                    false,
                    None,
                );
                let json = serde_json::to_vec(&resp)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                Ok(Some(json))
            }

            // Sub-opcode 3: Start player-to-player trade session
            3 => {
                let req: PlayerTradeStartRequest =
                    serde_json::from_slice(payload).map_err(|e| {
                        PacketHandlerError::MalformedPayload {
                            module_id: VillagePlugin::MODULE_ID,
                            message: e.to_string(),
                        }
                    })?;

                let target = Guid::from_raw(req.target_guid);
                trade_mgr
                    .start_session(entity_guid, target, entities)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;

                let resp = build_state_response(
                    entity_guid,
                    entities,
                    &trade_mgr,
                    Some(target),
                    false,
                    None,
                );
                let json = serde_json::to_vec(&resp)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                Ok(Some(json))
            }

            // Sub-opcode 4: Submit or revise trade offer
            4 => {
                let req: PlayerTradeOfferRequest =
                    serde_json::from_slice(payload).map_err(|e| {
                        PacketHandlerError::MalformedPayload {
                            module_id: VillagePlugin::MODULE_ID,
                            message: e.to_string(),
                        }
                    })?;

                let caller = entities
                    .get_by_guid(entity_guid)
                    .ok_or(PacketHandlerError::EntityNotFound(entity_guid))?;
                let caller_inv = caller
                    .get_component::<InventoryComponent>()
                    .ok_or_else(|| {
                        PacketHandlerError::Custom(
                            "Caller entity has no InventoryComponent".to_string(),
                        )
                    })?;

                if req.currency > caller_inv.currency {
                    return Err(PacketHandlerError::Custom(format!(
                        "Insufficient currency for trade offer (offered {}, current {})",
                        req.currency, caller_inv.currency
                    )));
                }

                // Validate items and resolve canonical metadata strictly from authoritative slots
                let mut canonical_items = Vec::with_capacity(req.items.len());
                let mut test_inv = caller_inv.clone();

                for item in req.items {
                    if item.quantity == 0 {
                        return Err(PacketHandlerError::Custom(
                            "Offered item quantity must be greater than zero".to_string(),
                        ));
                    }
                    if !item.unit_weight.is_finite() || item.unit_weight < 0.0 {
                        return Err(PacketHandlerError::Custom(
                            "Invalid item unit weight in trade offer".to_string(),
                        ));
                    }

                    // Look up authoritative item definition in caller's inventory
                    let canonical_slot = caller_inv
                        .slots
                        .iter()
                        .flatten()
                        .find(|s| s.item_id == item.item_id)
                        .ok_or_else(|| {
                            PacketHandlerError::Custom(format!(
                                "Offered item {} not found in inventory",
                                item.item_id
                            ))
                        })?;

                    let canonical_stack = ItemStack::new(
                        item.item_id,
                        &canonical_slot.name,
                        item.quantity,
                        canonical_slot.unit_weight,
                    );

                    // Multi-stack test deduction prevents over-offering duplicate item IDs
                    if !test_inv.remove_item_stack(&canonical_stack) {
                        return Err(PacketHandlerError::Custom(format!(
                            "Insufficient inventory quantity for item: {}",
                            canonical_stack.name
                        )));
                    }

                    canonical_items.push(canonical_stack);
                }

                let session = trade_mgr.get_session_mut(req.session_id).ok_or_else(|| {
                    PacketHandlerError::Custom(format!(
                        "Active trade session {} not found",
                        req.session_id
                    ))
                })?;

                if session.player_a != entity_guid && session.player_b != entity_guid {
                    return Err(PacketHandlerError::Custom(
                        "Caller is not a participant in this trade session".to_string(),
                    ));
                }

                let offer = TradeOffer {
                    items: canonical_items,
                    currency: req.currency,
                };

                session
                    .set_offer(entity_guid, offer)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;

                let resp =
                    build_state_response(entity_guid, entities, &trade_mgr, None, false, None);
                let json = serde_json::to_vec(&resp)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                Ok(Some(json))
            }

            // Sub-opcode 5: Lock and accept trade
            5 => {
                let req: PlayerTradeSessionRequest =
                    serde_json::from_slice(payload).map_err(|e| {
                        PacketHandlerError::MalformedPayload {
                            module_id: VillagePlugin::MODULE_ID,
                            message: e.to_string(),
                        }
                    })?;

                let both_accepted = {
                    let session = trade_mgr.get_session_mut(req.session_id).ok_or_else(|| {
                        PacketHandlerError::Custom(format!(
                            "Active trade session {} not found",
                            req.session_id
                        ))
                    })?;

                    if session.player_a != entity_guid && session.player_b != entity_guid {
                        return Err(PacketHandlerError::Custom(
                            "Caller is not a participant in this trade session".to_string(),
                        ));
                    }

                    let is_caller_locked = if entity_guid == session.player_a {
                        session.locked_a
                    } else {
                        session.locked_b
                    };

                    if !is_caller_locked {
                        session
                            .lock(entity_guid)
                            .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                        false
                    } else {
                        session
                            .accept(entity_guid)
                            .map_err(|e| PacketHandlerError::Custom(e.to_string()))?
                    }
                };

                let mut trade_completed = false;
                if both_accepted {
                    trade_mgr
                        .execute_atomic_swap(req.session_id, entities)
                        .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                    trade_completed = true;
                }

                let resp = build_state_response(
                    entity_guid,
                    entities,
                    &trade_mgr,
                    None,
                    trade_completed,
                    None,
                );
                let json = serde_json::to_vec(&resp)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                Ok(Some(json))
            }

            // Sub-opcode 6: Cancel active trade
            6 => {
                let req: PlayerTradeSessionRequest =
                    serde_json::from_slice(payload).map_err(|e| {
                        PacketHandlerError::MalformedPayload {
                            module_id: VillagePlugin::MODULE_ID,
                            message: e.to_string(),
                        }
                    })?;

                let session = trade_mgr.get_session(req.session_id).ok_or_else(|| {
                    PacketHandlerError::Custom(format!(
                        "Active trade session {} not found",
                        req.session_id
                    ))
                })?;

                if session.player_a != entity_guid && session.player_b != entity_guid {
                    return Err(PacketHandlerError::Custom(
                        "Caller is not a participant in this trade session".to_string(),
                    ));
                }

                trade_mgr.cancel_session(req.session_id);
                let resp =
                    build_state_response(entity_guid, entities, &trade_mgr, None, false, None);
                let json = serde_json::to_vec(&resp)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                Ok(Some(json))
            }

            _ => Err(PacketHandlerError::MalformedPayload {
                module_id: VillagePlugin::MODULE_ID,
                message: format!("Unknown village trade sub_opcode {}", sub_opcode),
            }),
        }
    }
}

/// Standalone gameplay plugin for village barter, starter inventories, and merchant components.
pub struct VillagePlugin {
    pub trade_manager: Arc<std::sync::Mutex<TradeManager>>,
}

impl Default for VillagePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl VillagePlugin {
    pub const MODULE_ID: u16 = 0x02;

    pub fn new() -> Self {
        Self {
            trade_manager: Arc::new(std::sync::Mutex::new(TradeManager::new())),
        }
    }

    pub fn with_trade_manager(trade_manager: Arc<std::sync::Mutex<TradeManager>>) -> Self {
        Self { trade_manager }
    }
}

impl GameplayPlugin for VillagePlugin {
    fn id(&self) -> &'static str {
        "village"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs
            .register::<VillagerMerchantComponent>("villager_merchant");
        ctx.hooks.register_handler(Box::new(PlayerStarterKitHook));
        ctx.hooks.register_handler(Box::new(TradeDistanceHook {
            trade_manager: self.trade_manager.clone(),
        }));
        ctx.packets.register(
            Self::MODULE_ID,
            Box::new(VillageTradePacketHandler::new(self.trade_manager.clone())),
        );
    }
}

/// Bootstraps the village world by installing plugins and placing structures and NPCs.
pub fn bootstrap_village_world(sim: &mut mmo_node::TickSimulation) {
    // 1. Install required gameplay plugins
    sim.add_plugin(Box::new(InventoryPlugin));
    sim.add_plugin(Box::new(NpcMovementPlugin));
    sim.add_plugin(Box::new(TradePlugin));
    sim.add_plugin(Box::new(VillagePlugin::new()));
    sim.init_plugins();

    // 2. Spawn Static & Interactive Village WorldObjects
    let structures = [
        // Village Well (prefab_id = 10, pos (0, 0, 0), scale (2.5, 2.0, 2.5))
        (
            10,
            (0.0, 0.0, 0.0),
            (2.5, 2.0, 2.5),
            BoundingBox3D::new((-1.25, -1.0, -1.25), (1.25, 1.0, 1.25)),
        ),
        // Blacksmith Forge (prefab_id = 11, pos (-12, 0, -8), scale (5.0, 4.0, 5.0))
        (
            11,
            (-12.0, 0.0, -8.0),
            (5.0, 4.0, 5.0),
            BoundingBox3D::new((-14.5, -2.0, -10.5), (-9.5, 2.0, -5.5)),
        ),
        // Herbalist Stall (prefab_id = 12, pos (12, 0, -8), scale (4.0, 3.0, 4.0))
        (
            12,
            (12.0, 0.0, -8.0),
            (4.0, 3.0, 4.0),
            BoundingBox3D::new((10.0, -1.5, -10.0), (14.0, 1.5, -6.0)),
        ),
        // Village House West (prefab_id = 13, pos (-18, 0, 10), scale (6.0, 5.0, 6.0))
        (
            13,
            (-18.0, 0.0, 10.0),
            (6.0, 5.0, 6.0),
            BoundingBox3D::new((-21.0, -2.5, 7.0), (-15.0, 2.5, 13.0)),
        ),
        // Village House East (prefab_id = 13, pos (18, 0, 10), scale (6.0, 5.0, 6.0))
        (
            13,
            (18.0, 0.0, 10.0),
            (6.0, 5.0, 6.0),
            BoundingBox3D::new((15.0, -2.5, 7.0), (21.0, 2.5, 13.0)),
        ),
    ];

    for (prefab_id, pos, scale, bounds) in structures {
        let seq = sim.next_world_object_seq;
        sim.next_world_object_seq = sim.next_world_object_seq.wrapping_add(1);
        let obj_guid = Guid::new(EntityType::DynamicObject, sim.node_id, seq);
        let transform = Transform3D::new(pos, (0.0, 0.0, 0.0, 1.0), scale);
        let obj = WorldObject::new(
            obj_guid,
            prefab_id,
            transform,
            bounds,
            ContainerOwner::System("village".to_string()),
        );
        sim.insert_world_object(obj);
    }

    // 3. Spawn Villager NPCs
    // NPC 1: Blacksmith Ilmari (Guid::new(EntityType::Npc, sim.node_id, 1)) at (-9.0, 0.0, -6.0)
    let ilmari_guid = Guid::new(EntityType::Npc, sim.node_id, 1);
    let ilmari_pos = (-9.0, 0.0, -6.0);
    sim.spawn_entity(ilmari_guid, ilmari_pos);
    if let Some(ilmari) = sim.entity_store.get_mut_by_guid(ilmari_guid) {
        let recipes = vec![
            BarterRecipe::new(
                1,
                "Forge Iron Pickaxe",
                vec![
                    ItemStack::new(1001, "Iron Ore", 2, 2.0),
                    ItemStack::new(1003, "Firewood", 1, 1.5),
                ],
                0,
                vec![ItemStack::new(2001, "Iron Pickaxe", 1, 3.5)],
                0,
            ),
            BarterRecipe::new(
                2,
                "Sell Iron Ore",
                vec![ItemStack::new(1001, "Iron Ore", 1, 2.0)],
                0,
                vec![],
                15,
            ),
            BarterRecipe::new(
                3,
                "Buy Steel Sword",
                vec![],
                25,
                vec![ItemStack::new(2002, "Steel Sword", 1, 4.0)],
                0,
            ),
        ];
        ilmari.add_component(VillagerMerchantComponent::new(
            "Blacksmith Ilmari",
            "Blacksmith",
            "Greetings, traveler! Bring me iron and wood, and I shall forge you the finest tools.",
            recipes,
        ));
    }

    // NPC 2: Herbalist Aino (Guid::new(EntityType::Npc, sim.node_id, 2)) at (9.0, 0.0, -6.0)
    let aino_guid = Guid::new(EntityType::Npc, sim.node_id, 2);
    let aino_pos = (9.0, 0.0, -6.0);
    sim.spawn_entity(aino_guid, aino_pos);
    if let Some(aino) = sim.entity_store.get_mut_by_guid(aino_guid) {
        let recipes = vec![
            BarterRecipe::new(
                1,
                "Brew Healing Potion",
                vec![ItemStack::new(1002, "Wild Herbs", 3, 0.5)],
                0,
                vec![ItemStack::new(2003, "Healing Potion", 1, 0.5)],
                0,
            ),
            BarterRecipe::new(
                2,
                "Sell Wild Herbs",
                vec![ItemStack::new(1002, "Wild Herbs", 1, 0.5)],
                0,
                vec![],
                8,
            ),
            BarterRecipe::new(
                3,
                "Buy Wild Herbs",
                vec![],
                10,
                vec![ItemStack::new(1002, "Wild Herbs", 2, 0.5)],
                0,
            ),
        ];
        aino.add_component(VillagerMerchantComponent::new(
            "Herbalist Aino",
            "Herbalist",
            "Welcome to my stall. The forest provides all the remedies you need.",
            recipes,
        ));
        aino.add_component(WaypointComponent::new(
            vec![(9.0, 0.0, -6.0), (4.0, 0.0, -2.0), (9.0, 0.0, 4.0)],
            WaypointLoopMode::PingPong,
        ));
        aino.add_component(NavAgentComponent::new(2.0, 0.3));
    }

    // NPC 3: Elder Väinö (Guid::new(EntityType::Npc, sim.node_id, 3)) at (0.0, 0.0, -4.0)
    let vaino_guid = Guid::new(EntityType::Npc, sim.node_id, 3);
    let vaino_pos = (0.0, 0.0, -4.0);
    sim.spawn_entity(vaino_guid, vaino_pos);
    if let Some(vaino) = sim.entity_store.get_mut_by_guid(vaino_guid) {
        let recipes = vec![
            BarterRecipe::new(
                1,
                "Obtain Village Charter Seal",
                vec![
                    ItemStack::new(2001, "Iron Pickaxe", 1, 3.5),
                    ItemStack::new(2003, "Healing Potion", 1, 0.5),
                ],
                0,
                vec![ItemStack::new(3001, "Village Charter Seal", 1, 1.0)],
                50,
            ),
            BarterRecipe::new(
                2,
                "Village Supplies Kit",
                vec![],
                15,
                vec![
                    ItemStack::new(1001, "Iron Ore", 2, 2.0),
                    ItemStack::new(1003, "Firewood", 1, 1.5),
                ],
                0,
            ),
        ];
        vaino.add_component(VillagerMerchantComponent::new(
            "Elder Väinö",
            "Village Elder",
            "Peace be with you, child. Prove your dedication to the village, and you shall earn our charter seal.",
            recipes,
        ));
        vaino.add_component(WaypointComponent::new(
            vec![
                (0.0, 0.0, -4.0),
                (3.0, 0.0, 0.0),
                (0.0, 0.0, 4.0),
                (-3.0, 0.0, 0.0),
            ],
            WaypointLoopMode::Loop,
        ));
        vaino.add_component(NavAgentComponent::new(1.5, 0.3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_node::TickSimulation;

    fn setup_village_sim() -> TickSimulation {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut sim = TickSimulation::new(1, bounds);
        bootstrap_village_world(&mut sim);
        sim
    }

    #[test]
    fn test_village_world_bootstrap() {
        let sim = setup_village_sim();

        // 1. Verify static structures indexed in sectors
        assert_eq!(sim.sectors.spatial_index().len(), 5);

        // 2. Verify all 3 NPCs exist with expected components
        let ilmari_guid = Guid::new(EntityType::Npc, sim.node_id, 1);
        let aino_guid = Guid::new(EntityType::Npc, sim.node_id, 2);
        let vaino_guid = Guid::new(EntityType::Npc, sim.node_id, 3);

        let ilmari = sim
            .entity_store
            .get_by_guid(ilmari_guid)
            .expect("Ilmari exists");
        let ilmari_merchant = ilmari
            .get_component::<VillagerMerchantComponent>()
            .expect("Ilmari has merchant component");
        assert_eq!(ilmari_merchant.name, "Blacksmith Ilmari");
        assert_eq!(ilmari_merchant.recipes.len(), 3);

        let aino = sim
            .entity_store
            .get_by_guid(aino_guid)
            .expect("Aino exists");
        let aino_merchant = aino
            .get_component::<VillagerMerchantComponent>()
            .expect("Aino has merchant component");
        assert_eq!(aino_merchant.name, "Herbalist Aino");
        assert_eq!(aino_merchant.recipes.len(), 3);
        assert!(aino.get_component::<WaypointComponent>().is_some());
        assert!(aino.get_component::<NavAgentComponent>().is_some());

        let vaino = sim
            .entity_store
            .get_by_guid(vaino_guid)
            .expect("Väinö exists");
        let vaino_merchant = vaino
            .get_component::<VillagerMerchantComponent>()
            .expect("Väinö has merchant component");
        assert_eq!(vaino_merchant.name, "Elder Väinö");
        assert_eq!(vaino_merchant.recipes.len(), 2);
        assert!(vaino.get_component::<WaypointComponent>().is_some());
        assert!(vaino.get_component::<NavAgentComponent>().is_some());
    }

    #[test]
    fn test_starter_kit_provisioning() {
        let mut sim = setup_village_sim();

        let player_guid = Guid::new(EntityType::Player, sim.node_id, 100);
        let pos = (0.0, 0.0, 0.0);
        sim.spawn_entity(player_guid, pos);

        let player = sim
            .entity_store
            .get_by_guid(player_guid)
            .expect("Player exists");
        let inv = player
            .get_component::<InventoryComponent>()
            .expect("Player has starter inventory");

        assert_eq!(inv.currency, 50);
        assert_eq!(inv.slots.len(), 12);
        assert!(inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 3, 2.0)));
        assert!(inv.has_item_stack(&ItemStack::new(1002, "Wild Herbs", 5, 0.5)));
        assert!(inv.has_item_stack(&ItemStack::new(1003, "Firewood", 2, 1.5)));

        // Spawning an NPC does not attach starter kit
        let dummy_npc = Guid::new(EntityType::Npc, sim.node_id, 999);
        sim.spawn_entity(dummy_npc, (1.0, 1.0, 1.0));
        let npc_ent = sim.entity_store.get_by_guid(dummy_npc).expect("NPC exists");
        assert!(npc_ent.get_component::<InventoryComponent>().is_none());
    }

    #[test]
    fn test_inspect_packet_handling() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 101);
        let p2 = Guid::new(EntityType::Player, sim.node_id, 102);
        let ilmari_guid = Guid::new(EntityType::Npc, sim.node_id, 1);

        sim.spawn_entity(p1, (0.0, 0.0, 0.0));
        sim.spawn_entity(p2, (2.0, 0.0, 0.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr);

        // 1. Inspect without target
        let res = handler
            .handle(p1, 1, &[], &mut sim.entity_store)
            .expect("inspect succeeds")
            .expect("response payload returned");
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        assert_eq!(state.player_guid, p1.raw());
        assert_eq!(state.inventory.as_ref().unwrap().currency, 50);
        assert!(state.target_merchant.is_none());

        // 2. Inspect NPC Ilmari
        let inspect_ilmari = serde_json::to_vec(&InspectRequest {
            target_guid: Some(ilmari_guid.raw()),
        })
        .unwrap();
        let res = handler
            .handle(p1, 1, &inspect_ilmari, &mut sim.entity_store)
            .expect("inspect succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let merchant = state.target_merchant.expect("Target merchant present");
        assert_eq!(merchant.name, "Blacksmith Ilmari");

        // 3. Inspect other player
        let inspect_p2 = serde_json::to_vec(&InspectRequest {
            target_guid: Some(p2.raw()),
        })
        .unwrap();
        let res = handler
            .handle(p1, 1, &inspect_p2, &mut sim.entity_store)
            .expect("inspect succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        assert_eq!(state.target_trade_status.as_deref(), Some("available"));
    }

    #[test]
    fn test_npc_barter_exchanges_and_distance_guard() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 105);
        let ilmari_guid = Guid::new(EntityType::Npc, sim.node_id, 1);

        // Spawn player near Blacksmith Ilmari (-9.0, 0.0, -6.0)
        sim.spawn_entity(p1, (-9.0, 0.0, -5.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr);

        // Recipe 1: 2 Iron Ore + 1 Firewood -> 1 Iron Pickaxe
        let barter_req = serde_json::to_vec(&NpcBarterRequest {
            target_guid: ilmari_guid.raw(),
            recipe_id: 1,
        })
        .unwrap();

        let res = handler
            .handle(p1, 2, &barter_req, &mut sim.entity_store)
            .expect("barter 1 succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let inv = state.inventory.expect("inventory present");
        assert!(inv.has_item_stack(&ItemStack::new(2001, "Iron Pickaxe", 1, 3.5)));
        // Remaining: 1 Iron Ore, 1 Firewood
        assert!(inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 1, 2.0)));
        assert!(!inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 2, 2.0)));

        // Recipe 2: Sell 1 Iron Ore -> 15 Gold Coins
        let barter_req_sell = serde_json::to_vec(&NpcBarterRequest {
            target_guid: ilmari_guid.raw(),
            recipe_id: 2,
        })
        .unwrap();
        let res = handler
            .handle(p1, 2, &barter_req_sell, &mut sim.entity_store)
            .expect("barter 2 succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let inv = state.inventory.expect("inventory present");
        assert_eq!(inv.currency, 65); // 50 + 15
        assert!(!inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 1, 2.0)));

        // Recipe 3: Buy Steel Sword for 25 Gold Coins
        let barter_req_buy = serde_json::to_vec(&NpcBarterRequest {
            target_guid: ilmari_guid.raw(),
            recipe_id: 3,
        })
        .unwrap();
        let res = handler
            .handle(p1, 2, &barter_req_buy, &mut sim.entity_store)
            .expect("barter 3 succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let inv = state.inventory.expect("inventory present");
        assert_eq!(inv.currency, 40); // 65 - 25
        assert!(inv.has_item_stack(&ItemStack::new(2002, "Steel Sword", 1, 4.0)));

        // Distance guard: Move player far away (> 5.0m)
        if let Some(player) = sim.entity_store.get_mut_by_guid(p1) {
            player.position = (50.0, 0.0, 50.0);
        }
        let err = handler
            .handle(p1, 2, &barter_req_buy, &mut sim.entity_store)
            .unwrap_err();
        match err {
            PacketHandlerError::Custom(msg) => assert!(msg.contains("too far away")),
            other => panic!("Unexpected error: {:?}", other),
        }
    }

    #[test]
    fn test_npc_barter_insufficient_requirements_and_encumbrance() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 110);
        let ilmari_guid = Guid::new(EntityType::Npc, sim.node_id, 1);

        sim.spawn_entity(p1, (-9.0, 0.0, -5.0));
        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr);

        // Clear currency to test insufficient currency
        if let Some(player) = sim.entity_store.get_mut_by_guid(p1) {
            let inv = player.get_component_mut::<InventoryComponent>().unwrap();
            inv.currency = 0;
        }

        let buy_sword_req = serde_json::to_vec(&NpcBarterRequest {
            target_guid: ilmari_guid.raw(),
            recipe_id: 3, // Requires 25 coins
        })
        .unwrap();

        let err = handler
            .handle(p1, 2, &buy_sword_req, &mut sim.entity_store)
            .unwrap_err();
        match err {
            PacketHandlerError::Custom(msg) => assert!(msg.contains("Insufficient currency")),
            other => panic!("Unexpected error: {:?}", other),
        }

        // Test encumbrance rollback
        if let Some(player) = sim.entity_store.get_mut_by_guid(p1) {
            let inv = player.get_component_mut::<InventoryComponent>().unwrap();
            inv.currency = 100;
            inv.max_weight = 5.0; // Current weight with ore + herbs + wood > 5.0
        }

        let err = handler
            .handle(p1, 2, &buy_sword_req, &mut sim.entity_store)
            .unwrap_err();
        match err {
            PacketHandlerError::Custom(msg) => assert!(msg.contains("encumbrance")),
            other => panic!("Unexpected error: {:?}", other),
        }
    }

    #[test]
    fn test_two_player_trade_via_modular_packets() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 201);
        let p2 = Guid::new(EntityType::Player, sim.node_id, 202);

        sim.spawn_entity(p1, (0.0, 0.0, 0.0));
        sim.spawn_entity(p2, (2.0, 0.0, 0.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr);

        // 1. P1 starts trade with P2 (sub-opcode 3)
        let start_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: p2.raw(),
        })
        .unwrap();
        let res = handler
            .handle(p1, 3, &start_req, &mut sim.entity_store)
            .expect("trade start succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let active = state.active_trade.expect("Active trade present");
        let session_id = active.session_id;

        // 2. P1 stages offer: 1 Iron Ore and 10 Gold (sub-opcode 4)
        let offer_p1 = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(1001, "Iron Ore", 1, 2.0)],
            currency: 10,
        })
        .unwrap();
        handler
            .handle(p1, 4, &offer_p1, &mut sim.entity_store)
            .expect("P1 offer succeeds");

        // 3. P2 stages offer: 2 Wild Herbs and 5 Gold (sub-opcode 4)
        let offer_p2 = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(1002, "Wild Herbs", 2, 0.5)],
            currency: 5,
        })
        .unwrap();
        handler
            .handle(p2, 4, &offer_p2, &mut sim.entity_store)
            .expect("P2 offer succeeds");

        // 4. P1 locks offer (sub-opcode 5)
        let accept_req = serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap();
        let res_p1 = handler
            .handle(p1, 5, &accept_req, &mut sim.entity_store)
            .expect("P1 lock succeeds")
            .unwrap();
        let state_p1: VillageTradeStateResponse = serde_json::from_slice(&res_p1).unwrap();
        assert!(!state_p1.trade_completed);
        assert!(state_p1.active_trade.as_ref().unwrap().my_locked);
        assert!(!state_p1.active_trade.as_ref().unwrap().their_locked);

        // 5. P2 locks offer (sub-opcode 5)
        let res_p2 = handler
            .handle(p2, 5, &accept_req, &mut sim.entity_store)
            .expect("P2 lock succeeds")
            .unwrap();
        let state_p2: VillageTradeStateResponse = serde_json::from_slice(&res_p2).unwrap();
        assert!(!state_p2.trade_completed);
        assert!(state_p2.active_trade.as_ref().unwrap().my_locked);
        assert!(state_p2.active_trade.as_ref().unwrap().their_locked);

        // 6. P1 accepts now that both are locked (sub-opcode 5)
        let res_p1_accept = handler
            .handle(p1, 5, &accept_req, &mut sim.entity_store)
            .expect("P1 accept succeeds")
            .unwrap();
        let state_p1_accept: VillageTradeStateResponse =
            serde_json::from_slice(&res_p1_accept).unwrap();
        assert!(!state_p1_accept.trade_completed);
        assert!(state_p1_accept.active_trade.as_ref().unwrap().my_accepted);

        // 7. P2 accepts -> atomic swap executes!
        let res_p2_final = handler
            .handle(p2, 5, &accept_req, &mut sim.entity_store)
            .expect("P2 final accept succeeds")
            .unwrap();
        let state_p2_final: VillageTradeStateResponse =
            serde_json::from_slice(&res_p2_final).unwrap();
        assert!(state_p2_final.trade_completed);

        // 8. Verify inventory states after swap
        let p1_inv = sim
            .entity_store
            .get_by_guid(p1)
            .unwrap()
            .get_component::<InventoryComponent>()
            .unwrap();
        let p2_inv = sim
            .entity_store
            .get_by_guid(p2)
            .unwrap()
            .get_component::<InventoryComponent>()
            .unwrap();

        // P1: had 50 gold, sent 10, received 5 -> 45 gold. Received 2 wild herbs.
        assert_eq!(p1_inv.currency, 45);
        assert!(p1_inv.has_item_stack(&ItemStack::new(1002, "Wild Herbs", 2, 0.5)));

        // P2: had 50 gold, sent 5, received 10 -> 55 gold. Received 1 iron ore.
        assert_eq!(p2_inv.currency, 55);
        assert!(p2_inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 1, 2.0)));
    }

    #[test]
    fn test_two_player_trade_cancel() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 301);
        let p2 = Guid::new(EntityType::Player, sim.node_id, 302);

        sim.spawn_entity(p1, (0.0, 0.0, 0.0));
        sim.spawn_entity(p2, (1.0, 0.0, 0.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr);

        let start_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: p2.raw(),
        })
        .unwrap();
        let res = handler
            .handle(p1, 3, &start_req, &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let session_id = state.active_trade.unwrap().session_id;

        // Cancel via sub-opcode 6
        let cancel_req = serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap();
        let res_cancel = handler
            .handle(p1, 6, &cancel_req, &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let state_cancel: VillageTradeStateResponse = serde_json::from_slice(&res_cancel).unwrap();
        assert!(state_cancel.active_trade.is_none());
    }

    #[test]
    fn test_trade_zero_quantity_weight_spoofing_and_multi_stack_guards() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 401);
        let p2 = Guid::new(EntityType::Player, sim.node_id, 402);

        sim.spawn_entity(p1, (0.0, 0.0, 0.0));
        sim.spawn_entity(p2, (1.0, 0.0, 0.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr.clone());

        // Start session
        let start_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: p2.raw(),
        })
        .unwrap();
        let res = handler
            .handle(p1, 3, &start_req, &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let session_id = state.active_trade.unwrap().session_id;

        // 1. Zero quantity item offer rejected
        let zero_offer = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(1001, "Iron Ore", 0, 2.0)],
            currency: 0,
        })
        .unwrap();
        let err = handler
            .handle(p1, 4, &zero_offer, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err, PacketHandlerError::Custom(msg) if msg.contains("greater than zero"))
        );

        // 2. Negative and NaN unit weights rejected
        let neg_weight = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(1001, "Iron Ore", 1, -10.0)],
            currency: 0,
        })
        .unwrap();
        let err = handler
            .handle(p1, 4, &neg_weight, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err, PacketHandlerError::Custom(msg) if msg.contains("Invalid item unit weight"))
        );

        let nan_weight = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(1001, "Iron Ore", 1, f32::NAN)],
            currency: 0,
        })
        .unwrap_or_else(|_| br#"{"session_id":1,"items":[{"item_id":1001,"name":"Iron Ore","quantity":1,"unit_weight":"NaN"}],"currency":0}"#.to_vec());
        let err = handler
            .handle(p1, 4, &nan_weight, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err, PacketHandlerError::Custom(ref msg) if msg.contains("Invalid item unit weight"))
                || matches!(err, PacketHandlerError::MalformedPayload { .. })
        );

        // 3. Multi-stack over-offering: P1 only owns 3 Iron Ore (starter kit). Offering two stacks of 2 (total 4) fails.
        let over_offer = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![
                ItemStack::new(1001, "Iron Ore", 2, 2.0),
                ItemStack::new(1001, "Iron Ore", 2, 2.0),
            ],
            currency: 0,
        })
        .unwrap();
        let err = handler
            .handle(p1, 4, &over_offer, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err, PacketHandlerError::Custom(msg) if msg.contains("Insufficient inventory quantity"))
        );

        // 4. Client submits spoofed name & unit_weight: server resolves canonical metadata
        let spoofed_offer = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(1001, "Hacked Gold Diamond", 1, 0.0)],
            currency: 5,
        })
        .unwrap();
        let res = handler
            .handle(p1, 4, &spoofed_offer, &mut sim.entity_store)
            .expect("canonical resolution succeeds")
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let active = state.active_trade.unwrap();
        assert_eq!(active.my_offer.items.len(), 1);
        // Canonical name & weight should match starter kit (Iron Ore, 2.0)
        assert_eq!(active.my_offer.items[0].name, "Iron Ore");
        assert_eq!(active.my_offer.items[0].unit_weight, 2.0);
    }

    #[test]
    fn test_trade_third_party_idor_protection() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 501);
        let p2 = Guid::new(EntityType::Player, sim.node_id, 502);
        let p3_attacker = Guid::new(EntityType::Player, sim.node_id, 503);

        sim.spawn_entity(p1, (0.0, 0.0, 0.0));
        sim.spawn_entity(p2, (1.0, 0.0, 0.0));
        sim.spawn_entity(p3_attacker, (2.0, 0.0, 0.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr.clone());

        // Start session between P1 and P2
        let start_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: p2.raw(),
        })
        .unwrap();
        let res = handler
            .handle(p1, 3, &start_req, &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let state: VillageTradeStateResponse = serde_json::from_slice(&res).unwrap();
        let session_id = state.active_trade.unwrap().session_id;

        // Attacker P3 tries to cancel session
        let cancel_req = serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap();
        let err_cancel = handler
            .handle(p3_attacker, 6, &cancel_req, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err_cancel, PacketHandlerError::Custom(msg) if msg.contains("not a participant"))
        );

        // Attacker P3 tries to accept session
        let accept_req = serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap();
        let err_accept = handler
            .handle(p3_attacker, 5, &accept_req, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err_accept, PacketHandlerError::Custom(msg) if msg.contains("not a participant"))
        );

        // Session should remain active
        assert!(trade_mgr.lock().unwrap().get_session(session_id).is_some());
    }

    #[test]
    fn test_trade_distance_hook_predictive_move_and_despawn() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 601);
        let p2 = Guid::new(EntityType::Player, sim.node_id, 602);

        sim.spawn_entity(p1, (0.0, 0.0, 0.0));
        sim.spawn_entity(p2, (2.0, 0.0, 0.0));

        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let hook = TradeDistanceHook {
            trade_manager: trade_mgr.clone(),
        };

        // Start session
        let s_id = trade_mgr
            .lock()
            .unwrap()
            .start_session(p1, p2, &sim.entity_store)
            .unwrap();

        // 1. Move within 5m: BeforeMove to (3.0, 0.0, 0.0) -> distance to p2(2.0, 0, 0) is 1m (< 5m)
        let ev_safe = Event::BeforeMove {
            entity: p1,
            from: (0.0, 0.0, 0.0),
            to: (3.0, 0.0, 0.0),
        };
        let mut ctx = EventContext::new();
        hook.handle_with_entities(&ev_safe, &mut ctx, &mut sim.entity_store);
        assert!(trade_mgr.lock().unwrap().get_session(s_id).is_some());

        // 2. Move out of range (> 5m): BeforeMove to (10.0, 0.0, 0.0) -> distance to p2 is 8m (> 5m)
        let ev_breach = Event::BeforeMove {
            entity: p1,
            from: (0.0, 0.0, 0.0),
            to: (10.0, 0.0, 0.0),
        };
        hook.handle_with_entities(&ev_breach, &mut ctx, &mut sim.entity_store);
        assert!(trade_mgr.lock().unwrap().get_session(s_id).is_none());

        // 3. Despawn cleans up session
        let s_id2 = trade_mgr
            .lock()
            .unwrap()
            .start_session(p1, p2, &sim.entity_store)
            .unwrap();
        let ev_despawn = Event::OnDespawn { entity: p1 };
        hook.handle_with_entities(&ev_despawn, &mut ctx, &mut sim.entity_store);
        assert!(trade_mgr.lock().unwrap().get_session(s_id2).is_none());
        assert!(!trade_mgr.lock().unwrap().is_player_trading(p2));
    }

    #[test]
    fn test_npc_barter_currency_overflow_rollback() {
        let mut sim = setup_village_sim();
        let p1 = Guid::new(EntityType::Player, sim.node_id, 701);
        let ilmari_guid = Guid::new(EntityType::Npc, sim.node_id, 1);

        sim.spawn_entity(p1, (-9.0, 0.0, -5.0));
        let trade_mgr = Arc::new(std::sync::Mutex::new(TradeManager::new()));
        let handler = VillageTradePacketHandler::new(trade_mgr);

        // Maximize player currency to test overflow on sell recipe 2 (rewards 15 gold)
        if let Some(player) = sim.entity_store.get_mut_by_guid(p1) {
            let inv = player.get_component_mut::<InventoryComponent>().unwrap();
            inv.currency = u64::MAX - 5; // adding 15 would overflow u64
        }

        let barter_req_sell = serde_json::to_vec(&NpcBarterRequest {
            target_guid: ilmari_guid.raw(),
            recipe_id: 2,
        })
        .unwrap();

        let err = handler
            .handle(p1, 2, &barter_req_sell, &mut sim.entity_store)
            .unwrap_err();
        assert!(
            matches!(err, PacketHandlerError::Custom(msg) if msg.contains("Currency overflow"))
        );

        // Verify currency and inventory rolled back
        let inv = sim
            .entity_store
            .get_by_guid(p1)
            .unwrap()
            .get_component::<InventoryComponent>()
            .unwrap();
        assert_eq!(inv.currency, u64::MAX - 5);
        // Did not consume Iron Ore
        assert!(inv.has_item_stack(&ItemStack::new(1001, "Iron Ore", 3, 2.0)));
    }
}
