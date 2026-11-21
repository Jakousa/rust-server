//! Competitive 6DoF Voxel Asteroid Miners Gameplay Plugin.
//!
//! Composes shared plugins (`InventoryPlugin`, `TradePlugin`) with genre-specific
//! plugins (`VoxelTerrainPlugin`, `CombatPlugin`, `AsteroidMiningPlugin`) over
//! `MotionMode::Newtonian6DoF`:
//! - Volumetric voxel asteroid seeding (`Crust = 1`, `Titanium = 2`, `Solar Gold = 3`, `Void Crystal = 4`).
//! - Event-driven ore harvesting from `VoxelChangeEvent` notifications into `InventoryComponent`.
//! - Dynamic ship loot-shininess tier (`0..=255`) encoded into `Entity::state_flags` bits `16..23`.
//! - Authoritative PvP ship combat (`module_id = 0x03`, `sub_opcode = 2`) with safe-zone,
//!   range, and cooldown validation plus full-loot transfer and station respawn on destruction.
//! - Home Orbital Station vault banking (`sub_opcode = 3`) and pilot-to-pilot trade (`sub_opcode = 4..=7`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mmo_core::container::ContainerOwner;
use mmo_core::entity::state_flags;
use mmo_core::guid::{EntityType, Guid};
use mmo_core::hook::{Event, EventContext, HookHandler};
use mmo_core::octree::{BoundingBox3D, Octree};
use mmo_core::plugin::{
    GameplayPlugin, ModularPacketHandler, PacketHandlerError, PluginContext, TickContext,
    TickSystem,
};
use mmo_core::world::{world_object_flags, Transform3D, WorldObject};
use mmo_core::{Entity, EntityStore};
use mmo_node::kinematics::MotionMode;
use mmo_node::TickSimulation;
use mmo_plugin_voxel::{
    index_from_local, ExcavationConfig, PaletteChunk, VoxelChangeEvent, VoxelChunkCoord,
    VoxelTerrainPlugin, VoxelWorld, CHUNK_SIZE, CHUNK_VOXELS,
};

use crate::combat::{CombatPlugin, EntityDiedEvent, HealthComponent};
use crate::inventory::{InventoryComponent, InventoryPlugin, ItemStack};
use crate::trade::{TradeManager, TradeOffer, TradePlugin};
use crate::village::{
    build_trade_session_view, InspectRequest, PlayerTradeOfferRequest, PlayerTradeSessionRequest,
    PlayerTradeStartRequest, TradeDistanceHook, TradeSessionView,
};

/// Home Orbital Station world position.
pub const HOME_STATION_POS: (f32, f32, f32) = (0.0, 0.0, 0.0);
/// Safe-zone & docking radius around the Home Orbital Station.
pub const HOME_STATION_SAFE_RADIUS: f32 = 15.0;
/// Default spawn and respawn position for player ships inside the station docking ring.
pub const DEFAULT_SHIP_SPAWN_POS: (f32, f32, f32) = (0.0, 0.0, 8.0);
/// Maximum PvP laser firing range in world units.
pub const PVP_LASER_RANGE: f32 = 45.0;
/// Authoritative hull damage dealt per PvP laser hit.
pub const PVP_LASER_DAMAGE: f32 = 25.0;
/// Minimum interval in milliseconds between PvP laser shots per ship.
pub const PVP_LASER_COOLDOWN_MS: u64 = 500;
/// Prefab ID for the Home Orbital Station `WorldObject`.
pub const STATION_PREFAB_ID: u32 = 100;
/// Modular packet routing ID for `AsteroidMiningPlugin`.
pub const ASTEROID_MODULE_ID: u16 = 0x03;

/// Voxel material ID: empty space.
pub const VOXEL_EMPTY: u16 = 0;
/// Voxel material ID: outer silicate asteroid crust (structural rock, yields no ore).
pub const VOXEL_CRUST_ROCK: u16 = 1;
/// Voxel material ID: Titanium vein.
pub const VOXEL_TITANIUM: u16 = 2;
/// Voxel material ID: Solar Gold vein.
pub const VOXEL_SOLAR_GOLD: u16 = 3;
/// Voxel material ID: Void Crystal core.
pub const VOXEL_VOID_CRYSTAL: u16 = 4;

/// Item ID for Titanium Ore (`unit_weight = 2.0`, `value = 10`).
pub const ITEM_TITANIUM_ORE_ID: u32 = 2001;
/// Item ID for Solar Gold (`unit_weight = 3.0`, `value = 30`).
pub const ITEM_SOLAR_GOLD_ID: u32 = 2002;
/// Item ID for Void Crystal (`unit_weight = 1.0`, `value = 75`).
pub const ITEM_VOID_CRYSTAL_ID: u32 = 2003;

/// Chunk coordinates seeded with volumetric 3D asteroids around the Home Orbital Station.
pub const SEEDED_ASTEROID_CHUNKS: [VoxelChunkCoord; 10] = [
    VoxelChunkCoord::new(0, 0, -2),
    VoxelChunkCoord::new(1, 0, -1),
    VoxelChunkCoord::new(-2, 0, 0),
    VoxelChunkCoord::new(0, 1, -1),
    VoxelChunkCoord::new(1, 0, 1),
    VoxelChunkCoord::new(-1, -1, -2),
    VoxelChunkCoord::new(-1, 0, -2),
    VoxelChunkCoord::new(0, -1, -2),
    VoxelChunkCoord::new(-1, 0, 1),
    VoxelChunkCoord::new(1, -1, -1),
];

/// Anchor positions for autonomous sector patrol drones outside the station safe zone.
pub const ROGUE_DRONE_SPAWNS: [(f32, f32, f32); 2] = [(22.0, 0.0, -18.0), (-22.0, 2.0, -18.0)];

/// Orbital patrol state attached to autonomous sector patrol drones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DronePatrolState {
    pub anchor: (f32, f32, f32),
    pub radius: f32,
    pub angular_speed: f32,
    pub angle: f32,
}

/// Permanent station vault storage & combat statistics attached to player ship entities.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StationVaultComponent {
    pub banked_credits: u64,
    pub banked_items: Vec<ItemStack>,
    pub kills: u32,
    pub deaths: u32,
}

/// JSON request payload for `sub_opcode = 2` (`FirePvpLaser`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirePvpLaserRequest {
    pub target_guid: u64,
}

/// Summary view of an inspected or targeted rival ship.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetShipInfo {
    pub target_guid: u64,
    pub position: (f32, f32, f32),
    pub hull_hp: f32,
    pub max_hull_hp: f32,
    pub shininess_tier: u8,
    pub in_safe_zone: bool,
    pub distance: f32,
    pub trade_status: Option<String>,
}

/// Authoritative sector, cargo, vault, hull, and trade state returned by `module_id = 0x03`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsteroidSectorStateResponse {
    pub player_guid: u64,
    pub inventory: Option<InventoryComponent>,
    pub current_weight: f32,
    pub max_weight: f32,
    pub station_vault: Option<StationVaultComponent>,
    pub hull_hp: f32,
    pub max_hull_hp: f32,
    pub shininess_tier: u8,
    pub unbanked_cargo_value: u64,
    pub in_safe_zone: bool,
    pub active_trade: Option<TradeSessionView>,
    pub target_info: Option<TargetShipInfo>,
    #[serde(default)]
    pub trade_completed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[inline]
fn distance_3d(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    let dz = a.2 - b.2;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Returns `true` when `pos` lies inside the Home Orbital Station safe-docking sphere.
#[inline]
pub fn is_in_safe_zone(pos: (f32, f32, f32)) -> bool {
    distance_3d(pos, HOME_STATION_POS) <= HOME_STATION_SAFE_RADIUS
}

/// Maps an excavated asteroid voxel ID (`2..=4`) to its harvested ore `ItemStack`.
pub fn ore_item_for_voxel(voxel_id: u16) -> Option<ItemStack> {
    match voxel_id {
        VOXEL_TITANIUM => Some(ItemStack::new(ITEM_TITANIUM_ORE_ID, "Titanium Ore", 1, 2.0)),
        VOXEL_SOLAR_GOLD => Some(ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0)),
        VOXEL_VOID_CRYSTAL => Some(ItemStack::new(ITEM_VOID_CRYSTAL_ID, "Void Crystal", 1, 1.0)),
        _ => None,
    }
}

/// Returns the unit credit value for an ore item ID.
#[inline]
pub const fn ore_unit_value(item_id: u32) -> u64 {
    match item_id {
        ITEM_TITANIUM_ORE_ID => 10,
        ITEM_SOLAR_GOLD_ID => 30,
        ITEM_VOID_CRYSTAL_ID => 75,
        _ => 0,
    }
}

/// Computes total unbanked cargo value (ore items + unbanked credits) in an inventory.
pub fn compute_cargo_value(inv: &InventoryComponent) -> u64 {
    let item_value: u64 = inv
        .slots
        .iter()
        .flatten()
        .map(|stack| ore_unit_value(stack.item_id).saturating_mul(u64::from(stack.quantity)))
        .fold(0u64, u64::saturating_add);
    item_value.saturating_add(inv.currency)
}

/// Maps total unbanked cargo value to a normalized `0..=255` visual shininess tier.
#[inline]
pub fn compute_shininess_tier(cargo_value: u64) -> u8 {
    (cargo_value / 5).min(255) as u8
}

/// Encodes a `0..=255` shininess tier into `bits 16..23` of `Entity::state_flags`,
/// preserving all other state flag bits.
#[inline]
pub const fn encode_shininess_state_flags(flags: u32, tier: u8) -> u32 {
    (flags & 0xFF00_FFFF) | ((tier as u32) << 16)
}

/// Extracts the `0..=255` shininess tier from `bits 16..23` of `Entity::state_flags`.
#[inline]
pub const fn extract_shininess_tier(flags: u32) -> u8 {
    ((flags >> 16) & 0xFF) as u8
}

/// Recomputes and writes an entity's loot-shininess tier into `entity.state_flags`.
pub fn update_entity_shininess(entity: &mut Entity) -> u8 {
    let tier = entity
        .get_component::<InventoryComponent>()
        .map(compute_cargo_value)
        .map(compute_shininess_tier)
        .unwrap_or(0);
    entity.state_flags = encode_shininess_state_flags(entity.state_flags, tier);
    tier
}

fn sync_entity_health_properties(entity: &mut Entity) {
    if let Some(health) = entity.get_component::<HealthComponent>() {
        let hp = health.current_hp.round().max(0.0) as i64;
        let max_hp = health.max_hp.round().max(1.0) as i64;
        if entity.get_property_i64("health") != Some(hp) {
            entity.properties.set("health", hp);
        }
        if entity.get_property_i64("max_health") != Some(max_hp) {
            entity.properties.set("max_health", max_hp);
        }
    }
}

fn merge_item_stack_into_vec(items: &mut Vec<ItemStack>, stack: ItemStack) {
    if stack.quantity == 0 {
        return;
    }
    if let Some(existing) = items.iter_mut().find(|s| {
        s.item_id == stack.item_id
            && s.name == stack.name
            && (s.unit_weight - stack.unit_weight).abs() < 1e-6
    }) {
        existing.quantity = existing.quantity.saturating_add(stack.quantity);
    } else {
        items.push(stack);
    }
}

/// Merges or adds `stack` (up to available weight and slot capacity) into `inv`.
/// Returns the number of units transferred without exceeding `inv.max_weight`.
fn add_or_merge_item_respecting_capacity(inv: &mut InventoryComponent, stack: ItemStack) -> u32 {
    if stack.quantity == 0 || !stack.unit_weight.is_finite() || stack.unit_weight < 0.0 {
        return 0;
    }

    let available_weight = (inv.max_weight - inv.current_weight()).max(0.0);
    let max_units_by_weight = if stack.unit_weight == 0.0 {
        stack.quantity
    } else {
        (available_weight / stack.unit_weight).floor().max(0.0) as u32
    };
    let units_to_add = stack.quantity.min(max_units_by_weight);
    if units_to_add == 0 {
        return 0;
    }

    if let Some(existing) = inv.slots.iter_mut().flatten().find(|s| {
        s.item_id == stack.item_id
            && s.name == stack.name
            && (s.unit_weight - stack.unit_weight).abs() < 1e-6
    }) {
        existing.quantity = existing.quantity.saturating_add(units_to_add);
        return units_to_add;
    }

    let candidate = ItemStack::new(stack.item_id, stack.name, units_to_add, stack.unit_weight);
    if inv.add_item(candidate).is_ok() {
        units_to_add
    } else {
        0
    }
}

fn restock_drone_cargo(inv: &mut InventoryComponent) {
    inv.slots.fill(None);
    inv.currency = 0;
    let _ = inv.add_item(ItemStack::new(ITEM_TITANIUM_ORE_ID, "Titanium Ore", 2, 2.0));
    let _ = inv.add_item(ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0));
}

type OctreeRelocation = (Guid, (f32, f32, f32), (f32, f32, f32));
type PendingOctreeRelocations = Arc<Mutex<Vec<OctreeRelocation>>>;

/// Handles full-loot transfer from victim to killer, kill/death counters,
/// hull restoration, station respawn, and shininess recalculation.
fn handle_ship_destruction(
    victim_guid: Guid,
    killer_guid: Option<Guid>,
    entities: &mut EntityStore,
    mut octree: Option<&mut Octree>,
    pending_relocations: Option<&PendingOctreeRelocations>,
    trade_manager: Option<&Arc<Mutex<TradeManager>>>,
) {
    // Cancel any active trade session involving the destroyed ship
    if let Some(tm_arc) = trade_manager {
        if let Ok(mut trade_mgr) = tm_arc.lock() {
            if let Some(session_id) = trade_mgr
                .get_session_by_player(victim_guid)
                .map(|s| s.session_id)
            {
                let _ = trade_mgr.cancel_session(session_id, victim_guid);
            }
        }
    }

    // Transfer unbanked cargo & currency when destroyed by another ship
    if let Some(killer) = killer_guid.filter(|&k| k != victim_guid) {
        if let Some((killer_ent, victim_ent)) = entities.get_two_mut(killer, victim_guid) {
            let (looted_items, looted_currency) =
                if let Some(victim_inv) = victim_ent.get_component_mut::<InventoryComponent>() {
                    let mut items = Vec::new();
                    for slot in &mut victim_inv.slots {
                        if let Some(stack) = slot.take() {
                            items.push(stack);
                        }
                    }
                    let currency = std::mem::take(&mut victim_inv.currency);
                    (items, currency)
                } else {
                    (Vec::new(), 0)
                };

            if let Some(killer_inv) = killer_ent.get_component_mut::<InventoryComponent>() {
                for stack in looted_items {
                    add_or_merge_item_respecting_capacity(killer_inv, stack);
                }
                killer_inv.currency = killer_inv.currency.saturating_add(looted_currency);
            }

            if let Some(vault) = killer_ent.get_component_mut::<StationVaultComponent>() {
                vault.kills = vault.kills.saturating_add(1);
            }

            update_entity_shininess(killer_ent);
        }
    }

    // Reset victim cargo, increment death counter, restore hull, and respawn at dock or patrol anchor
    let Some(victim_ent) = entities.get_mut_by_guid(victim_guid) else {
        return;
    };

    let is_npc = victim_guid.entity_type() == EntityType::Npc;
    let respawn_pos = if is_npc {
        if let Some(patrol) = victim_ent.get_component_mut::<DronePatrolState>() {
            patrol.angle = 0.0;
            (
                patrol.anchor.0 + patrol.radius,
                patrol.anchor.1,
                patrol.anchor.2,
            )
        } else {
            (
                ROGUE_DRONE_SPAWNS[0].0 + 4.5,
                ROGUE_DRONE_SPAWNS[0].1,
                ROGUE_DRONE_SPAWNS[0].2,
            )
        }
    } else {
        DEFAULT_SHIP_SPAWN_POS
    };

    if let Some(victim_inv) = victim_ent.get_component_mut::<InventoryComponent>() {
        if is_npc {
            restock_drone_cargo(victim_inv);
        } else {
            victim_inv.slots.fill(None);
            victim_inv.currency = 0;
        }
    }

    if let Some(vault) = victim_ent.get_component_mut::<StationVaultComponent>() {
        vault.deaths = vault.deaths.saturating_add(1);
    }

    if let Some(health) = victim_ent.get_component_mut::<HealthComponent>() {
        let respawn_hp = if health.max_hp.is_finite() && health.max_hp > 0.0 {
            health.max_hp
        } else {
            100.0
        };
        health.respawn(respawn_hp);
    }

    victim_ent.state_flags &= !state_flags::INCAPACITATED;
    sync_entity_health_properties(victim_ent);

    let old_pos = victim_ent.position;
    victim_ent.position = respawn_pos;
    victim_ent.velocity = (0.0, 0.0, 0.0);
    update_entity_shininess(victim_ent);

    if let Some(ref mut tree) = octree {
        if !tree.remove(victim_guid, old_pos) {
            tree.remove_by_guid(victim_guid);
        }
        tree.insert(victim_guid, respawn_pos);
    } else if let Some(reloc) = pending_relocations {
        if let Ok(mut queue) = reloc.lock() {
            queue.push((victim_guid, old_pos, respawn_pos));
        }
    }
}

/// Lifecycle hook provisioning newly spawned player ships with starter cargo hold,
/// hull health, station vault, and initial shininess state flags.
pub struct PilotStarterKitHook;

impl HookHandler for PilotStarterKitHook {
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
                        ent.add_component(InventoryComponent::new(16, 150.0));
                    }
                    if ent.get_component::<HealthComponent>().is_none() {
                        ent.add_component(HealthComponent::new(100.0, 1.0));
                    }
                    if ent.get_component::<StationVaultComponent>().is_none() {
                        ent.add_component(StationVaultComponent::default());
                    }
                    sync_entity_health_properties(ent);
                    update_entity_shininess(ent);
                }
            }
        }
    }
}

/// Aggregates authoritative state for the caller's ship, cargo, station vault,
/// active trade session, and optional inspected target ship.
pub fn build_asteroid_state_response(
    caller_guid: Guid,
    entities: &EntityStore,
    trade_mgr: &TradeManager,
    target_guid: Option<Guid>,
    trade_completed: bool,
    error: Option<String>,
) -> AsteroidSectorStateResponse {
    let caller_ent = entities.get_by_guid(caller_guid);
    let inventory = caller_ent.and_then(|e| e.get_component::<InventoryComponent>().cloned());
    let current_weight = inventory
        .as_ref()
        .map(|i| i.current_weight())
        .unwrap_or(0.0);
    let max_weight = inventory.as_ref().map(|i| i.max_weight).unwrap_or(0.0);
    let unbanked_cargo_value = inventory.as_ref().map(compute_cargo_value).unwrap_or(0);

    let station_vault =
        caller_ent.and_then(|e| e.get_component::<StationVaultComponent>().cloned());

    let (hull_hp, max_hull_hp) = caller_ent
        .and_then(|e| e.get_component::<HealthComponent>())
        .map(|h| (h.current_hp, h.max_hp))
        .unwrap_or((0.0, 0.0));

    let shininess_tier = caller_ent
        .map(|e| extract_shininess_tier(e.state_flags))
        .unwrap_or(0);
    let in_safe_zone = caller_ent
        .map(|e| is_in_safe_zone(e.position))
        .unwrap_or(false);

    let active_trade = trade_mgr
        .get_session_by_player(caller_guid)
        .map(|s| build_trade_session_view(s, caller_guid));

    let target_info = target_guid.and_then(|target| {
        let target_ent = entities.get_by_guid(target)?;
        let (t_hp, t_max_hp) = target_ent
            .get_component::<HealthComponent>()
            .map(|h| (h.current_hp, h.max_hp))
            .unwrap_or((0.0, 0.0));
        let distance = caller_ent
            .map(|c| distance_3d(c.position, target_ent.position))
            .unwrap_or(0.0);
        let trade_status = if target.entity_type() == EntityType::Player && target != caller_guid {
            let status = if let Some(session) = trade_mgr.get_session_by_player(target) {
                if session.player_a == caller_guid || session.player_b == caller_guid {
                    "trading_with_you"
                } else {
                    "busy"
                }
            } else {
                "available"
            };
            Some(status.to_string())
        } else {
            None
        };
        Some(TargetShipInfo {
            target_guid: target.raw(),
            position: target_ent.position,
            hull_hp: t_hp,
            max_hull_hp: t_max_hp,
            shininess_tier: extract_shininess_tier(target_ent.state_flags),
            in_safe_zone: is_in_safe_zone(target_ent.position),
            distance,
            trade_status,
        })
    });

    AsteroidSectorStateResponse {
        player_guid: caller_guid.raw(),
        inventory,
        current_weight,
        max_weight,
        station_vault,
        hull_hp,
        max_hull_hp,
        shininess_tier,
        unbanked_cargo_value,
        in_safe_zone,
        active_trade,
        target_info,
        trade_completed,
        error,
    }
}

/// Tick system connecting `VoxelTerrainPlugin` (`VoxelChangeEvent`) and `CombatPlugin`
/// (`EntityDiedEvent`) to `AsteroidMiningPlugin` cargo harvesting, full-loot transfer,
/// station respawn, and `state_flags` shininess synchronization.
pub struct AsteroidMiningTickSystem {
    trade_manager: Arc<Mutex<TradeManager>>,
    laser_cooldowns: Arc<Mutex<HashMap<Guid, Instant>>>,
    pending_octree_relocations: PendingOctreeRelocations,
}

impl Default for AsteroidMiningTickSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl AsteroidMiningTickSystem {
    pub fn new() -> Self {
        Self {
            trade_manager: Arc::new(Mutex::new(TradeManager::new())),
            laser_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            pending_octree_relocations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_shared(
        trade_manager: Arc<Mutex<TradeManager>>,
        laser_cooldowns: Arc<Mutex<HashMap<Guid, Instant>>>,
        pending_octree_relocations: PendingOctreeRelocations,
    ) -> Self {
        Self {
            trade_manager,
            laser_cooldowns,
            pending_octree_relocations,
        }
    }
}

impl TickSystem for AsteroidMiningTickSystem {
    fn name(&self) -> &'static str {
        "asteroid_mining_tick_system"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        ctx.events.register_ephemeral::<VoxelChangeEvent>();
        ctx.events.register_ephemeral::<EntityDiedEvent>();

        // 1. Flush any octree relocations queued by packet-driven PvP kills
        if let Ok(mut pending) = self.pending_octree_relocations.lock() {
            for (guid, old_pos, new_pos) in pending.drain(..) {
                if !ctx.octree.remove(guid, old_pos) {
                    ctx.octree.remove_by_guid(guid);
                }
                ctx.octree.insert(guid, new_pos);
            }
        }

        // 2. Prune stale laser cooldown tracking for despawned entities
        if let Ok(mut cooldowns) = self.laser_cooldowns.lock() {
            cooldowns.retain(|guid, _| ctx.entities.contains_guid(*guid));
        }

        // 3. Process voxel excavation events -> credit ore into InventoryComponent & update shininess
        let voxel_events: Vec<(Guid, u16, u16)> = ctx
            .events
            .read::<VoxelChangeEvent>()
            .into_iter()
            .map(|ev| (ev.entity_guid, ev.old_voxel, ev.new_voxel))
            .collect();

        for (actor_guid, old_voxel, new_voxel) in voxel_events {
            if new_voxel != VOXEL_EMPTY {
                continue;
            }
            let Some(ore_stack) = ore_item_for_voxel(old_voxel) else {
                continue;
            };
            if let Some(ent) = ctx.entities.get_mut_by_guid(actor_guid) {
                if let Some(inv) = ent.get_component_mut::<InventoryComponent>() {
                    add_or_merge_item_respecting_capacity(inv, ore_stack);
                }
                update_entity_shininess(ent);
            }
        }

        // 4. Process EntityDiedEvents -> full-loot cargo transfer, kills/deaths, station respawn
        let death_events: Vec<(Guid, Option<Guid>)> = ctx
            .events
            .read::<EntityDiedEvent>()
            .into_iter()
            .map(|ev| (ev.entity, ev.killer))
            .collect();

        for (victim_guid, killer_guid) in death_events {
            handle_ship_destruction(
                victim_guid,
                killer_guid,
                ctx.entities,
                Some(ctx.octree),
                None,
                Some(&self.trade_manager),
            );
        }

        // 4.5. Advance autonomous NPC patrol drones along orbital paths and sync octree
        let mut drone_moves: Vec<OctreeRelocation> = Vec::new();
        let octree_bounds = ctx.octree.bounds;
        for (_key, entity) in ctx.entities.iter_mut() {
            if entity.guid.entity_type() != EntityType::Npc || entity.is_incapacitated() {
                continue;
            }
            if entity
                .get_component::<HealthComponent>()
                .is_some_and(|h| h.is_dead)
            {
                continue;
            }
            let Some(patrol) = entity.get_component_mut::<DronePatrolState>() else {
                continue;
            };
            patrol.angle = (patrol.angle + patrol.angular_speed * ctx.dt_sec)
                .rem_euclid(std::f32::consts::TAU);
            let raw_pos = (
                patrol.anchor.0 + patrol.radius * patrol.angle.cos(),
                patrol.anchor.1,
                patrol.anchor.2 + patrol.radius * patrol.angle.sin(),
            );
            let new_pos = if octree_bounds.contains(raw_pos) {
                raw_pos
            } else {
                octree_bounds.clamp_point(raw_pos)
            };
            let vx = -patrol.radius * patrol.angular_speed * patrol.angle.sin();
            let vz = patrol.radius * patrol.angular_speed * patrol.angle.cos();
            let old_pos = entity.position;
            entity.position = new_pos;
            entity.velocity = (vx, 0.0, vz);
            entity.rotation = vx.atan2(vz);
            drone_moves.push((entity.guid, old_pos, new_pos));
        }
        for (guid, old_pos, new_pos) in drone_moves {
            if !ctx.octree.remove(guid, old_pos) {
                ctx.octree.remove_by_guid(guid);
            }
            ctx.octree.insert(guid, new_pos);
        }

        // 5. Keep AoI-replicated health properties synchronized with HealthComponent (e.g. regen)
        for (_key, entity) in ctx.entities.iter_mut() {
            sync_entity_health_properties(entity);
        }
    }
}

/// Modular packet handler for `ASTEROID_MODULE_ID` (`0x03`).
pub struct AsteroidPacketHandler {
    pub trade_manager: Arc<Mutex<TradeManager>>,
    pub laser_cooldowns: Arc<Mutex<HashMap<Guid, Instant>>>,
    pending_octree_relocations: PendingOctreeRelocations,
}

impl AsteroidPacketHandler {
    pub fn new(trade_manager: Arc<Mutex<TradeManager>>) -> Self {
        Self {
            trade_manager,
            laser_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            pending_octree_relocations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_shared(
        trade_manager: Arc<Mutex<TradeManager>>,
        laser_cooldowns: Arc<Mutex<HashMap<Guid, Instant>>>,
        pending_octree_relocations: PendingOctreeRelocations,
    ) -> Self {
        Self {
            trade_manager,
            laser_cooldowns,
            pending_octree_relocations,
        }
    }

    /// Processes a PvP laser fire command using an explicit monotonic timestamp (`now`),
    /// enabling deterministic unit testing of the `500ms` weapon cooldown.
    pub fn fire_pvp_laser_at(
        &self,
        caller_guid: Guid,
        target_guid: Guid,
        entities: &mut EntityStore,
        now: Instant,
    ) -> Result<AsteroidSectorStateResponse, PacketHandlerError> {
        if target_guid.raw() == 0 || target_guid == caller_guid {
            return Err(PacketHandlerError::Custom(
                "Cannot fire PvP laser at self or invalid target".to_string(),
            ));
        }

        let (caller_pos, caller_incapacitated, target_pos, target_dead) = {
            let caller = entities
                .get_by_guid(caller_guid)
                .ok_or(PacketHandlerError::EntityNotFound(caller_guid))?;
            let target = entities
                .get_by_guid(target_guid)
                .ok_or(PacketHandlerError::EntityNotFound(target_guid))?;

            let c_dead = caller.is_incapacitated()
                || caller
                    .get_component::<HealthComponent>()
                    .is_some_and(|h| h.is_dead);
            let t_health = target.get_component::<HealthComponent>().ok_or_else(|| {
                PacketHandlerError::Custom("Target entity has no HealthComponent".to_string())
            })?;
            (caller.position, c_dead, target.position, t_health.is_dead)
        };

        if caller_incapacitated {
            return Err(PacketHandlerError::Custom(
                "Cannot fire weapons while ship is incapacitated".to_string(),
            ));
        }

        if target_dead {
            return Err(PacketHandlerError::Custom(
                "Target ship is already destroyed".to_string(),
            ));
        }

        if is_in_safe_zone(caller_pos) {
            return Err(PacketHandlerError::Custom(
                "Cannot fire PvP laser inside Home Orbital Station safe zone".to_string(),
            ));
        }

        if is_in_safe_zone(target_pos) {
            return Err(PacketHandlerError::Custom(
                "Target ship is protected inside Home Orbital Station safe zone".to_string(),
            ));
        }

        let dist = distance_3d(caller_pos, target_pos);
        if dist > PVP_LASER_RANGE {
            return Err(PacketHandlerError::Custom(format!(
                "Target ship out of laser range ({:.1}m > {:.1}m)",
                dist, PVP_LASER_RANGE
            )));
        }

        {
            let mut cooldowns = self
                .laser_cooldowns
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let min_interval = Duration::from_millis(PVP_LASER_COOLDOWN_MS);
            if let Some(&last_shot) = cooldowns.get(&caller_guid) {
                if now.saturating_duration_since(last_shot) < min_interval {
                    return Err(PacketHandlerError::Custom(format!(
                        "PvP laser cooling down ({}ms cooldown)",
                        PVP_LASER_COOLDOWN_MS
                    )));
                }
            }
            cooldowns.insert(caller_guid, now);
        }

        let died = {
            let target = entities
                .get_mut_by_guid(target_guid)
                .ok_or(PacketHandlerError::EntityNotFound(target_guid))?;
            let health = target
                .get_component_mut::<HealthComponent>()
                .ok_or_else(|| {
                    PacketHandlerError::Custom("Target entity has no HealthComponent".to_string())
                })?;
            let died = health.take_damage(PVP_LASER_DAMAGE);
            sync_entity_health_properties(target);
            died
        };

        if died {
            handle_ship_destruction(
                target_guid,
                Some(caller_guid),
                entities,
                None,
                Some(&self.pending_octree_relocations),
                Some(&self.trade_manager),
            );
        }

        let trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
        Ok(build_asteroid_state_response(
            caller_guid,
            entities,
            &trade_mgr,
            Some(target_guid),
            false,
            None,
        ))
    }

    fn deposit_to_station_vault(
        &self,
        caller_guid: Guid,
        entities: &mut EntityStore,
    ) -> Result<AsteroidSectorStateResponse, PacketHandlerError> {
        let caller = entities
            .get_mut_by_guid(caller_guid)
            .ok_or(PacketHandlerError::EntityNotFound(caller_guid))?;

        if !is_in_safe_zone(caller.position) {
            return Err(PacketHandlerError::Custom(format!(
                "Must dock within {:.1}m of Home Orbital Station to deposit cargo",
                HOME_STATION_SAFE_RADIUS
            )));
        }

        let (drained_items, cargo_value) = {
            let inv = caller
                .get_component_mut::<InventoryComponent>()
                .ok_or_else(|| {
                    PacketHandlerError::Custom(
                        "Caller entity has no InventoryComponent".to_string(),
                    )
                })?;
            let cargo_value = compute_cargo_value(inv);
            let mut items = Vec::new();
            for slot in &mut inv.slots {
                if let Some(stack) = slot.take() {
                    items.push(stack);
                }
            }
            inv.currency = 0;
            (items, cargo_value)
        };

        if caller.get_component::<StationVaultComponent>().is_none() {
            caller.add_component(StationVaultComponent::default());
        }
        if let Some(vault) = caller.get_component_mut::<StationVaultComponent>() {
            vault.banked_credits = vault.banked_credits.saturating_add(cargo_value);
            for stack in drained_items {
                merge_item_stack_into_vec(&mut vault.banked_items, stack);
            }
        }

        update_entity_shininess(caller);

        let trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
        Ok(build_asteroid_state_response(
            caller_guid,
            entities,
            &trade_mgr,
            None,
            false,
            None,
        ))
    }

    fn handle_trade_offer(
        &self,
        caller_guid: Guid,
        req: PlayerTradeOfferRequest,
        entities: &EntityStore,
    ) -> Result<AsteroidSectorStateResponse, PacketHandlerError> {
        let caller = entities
            .get_by_guid(caller_guid)
            .ok_or(PacketHandlerError::EntityNotFound(caller_guid))?;
        let caller_inv = caller
            .get_component::<InventoryComponent>()
            .ok_or_else(|| {
                PacketHandlerError::Custom("Caller entity has no InventoryComponent".to_string())
            })?;

        if req.currency > caller_inv.currency {
            return Err(PacketHandlerError::Custom(format!(
                "Insufficient currency for trade offer (offered {}, current {})",
                req.currency, caller_inv.currency
            )));
        }

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

            if !test_inv.remove_item_stack(&canonical_stack) {
                return Err(PacketHandlerError::Custom(format!(
                    "Insufficient inventory quantity for item: {}",
                    canonical_stack.name
                )));
            }

            canonical_items.push(canonical_stack);
        }

        let mut trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
        let session = trade_mgr.get_session_mut(req.session_id).ok_or_else(|| {
            PacketHandlerError::Custom(format!("Active trade session {} not found", req.session_id))
        })?;

        if session.player_a != caller_guid && session.player_b != caller_guid {
            return Err(PacketHandlerError::Custom(
                "Caller is not a participant in this trade session".to_string(),
            ));
        }

        session
            .set_offer(
                caller_guid,
                TradeOffer {
                    items: canonical_items,
                    currency: req.currency,
                },
            )
            .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;

        Ok(build_asteroid_state_response(
            caller_guid,
            entities,
            &trade_mgr,
            None,
            false,
            None,
        ))
    }

    fn handle_trade_accept(
        &self,
        caller_guid: Guid,
        req: PlayerTradeSessionRequest,
        entities: &mut EntityStore,
    ) -> Result<AsteroidSectorStateResponse, PacketHandlerError> {
        let mut trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
        let both_accepted = {
            let session = trade_mgr.get_session_mut(req.session_id).ok_or_else(|| {
                PacketHandlerError::Custom(format!(
                    "Active trade session {} not found",
                    req.session_id
                ))
            })?;

            if session.player_a != caller_guid && session.player_b != caller_guid {
                return Err(PacketHandlerError::Custom(
                    "Caller is not a participant in this trade session".to_string(),
                ));
            }

            let is_caller_locked = if caller_guid == session.player_a {
                session.locked_a
            } else {
                session.locked_b
            };

            if !is_caller_locked {
                session
                    .lock(caller_guid)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                false
            } else {
                session
                    .accept(caller_guid)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?
            }
        };

        let mut trade_completed = false;
        if both_accepted {
            let completed = trade_mgr
                .execute_atomic_swap(req.session_id, entities)
                .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
            if let Some(ent_a) = entities.get_mut_by_guid(completed.player_a) {
                update_entity_shininess(ent_a);
            }
            if let Some(ent_b) = entities.get_mut_by_guid(completed.player_b) {
                update_entity_shininess(ent_b);
            }
            trade_completed = true;
        }

        Ok(build_asteroid_state_response(
            caller_guid,
            entities,
            &trade_mgr,
            None,
            trade_completed,
            None,
        ))
    }
}

fn parse_json_payload<T: serde::de::DeserializeOwned>(
    payload: &[u8],
) -> Result<T, PacketHandlerError> {
    serde_json::from_slice(payload).map_err(|e| PacketHandlerError::MalformedPayload {
        module_id: AsteroidMiningPlugin::MODULE_ID,
        message: e.to_string(),
    })
}

fn encode_response_json(resp: &AsteroidSectorStateResponse) -> Result<Vec<u8>, PacketHandlerError> {
    serde_json::to_vec(resp).map_err(|e| PacketHandlerError::Custom(e.to_string()))
}

impl ModularPacketHandler for AsteroidPacketHandler {
    fn handle(
        &self,
        entity_guid: Guid,
        sub_opcode: u16,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketHandlerError> {
        match sub_opcode {
            // Sub-opcode 1: Inspect sector state, cargo, vault, hull, and optional target ship
            1 => {
                let req: InspectRequest = if payload.is_empty() {
                    InspectRequest::default()
                } else {
                    parse_json_payload(payload)?
                };
                let target = req.target_guid.filter(|&g| g != 0).map(Guid::from_raw);
                let trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
                let resp = build_asteroid_state_response(
                    entity_guid,
                    entities,
                    &trade_mgr,
                    target,
                    false,
                    None,
                );
                Ok(Some(encode_response_json(&resp)?))
            }

            // Sub-opcode 2: Authoritative PvP laser fire
            2 => {
                let req: FirePvpLaserRequest = parse_json_payload(payload)?;
                let target_guid = Guid::from_raw(req.target_guid);
                let resp =
                    self.fire_pvp_laser_at(entity_guid, target_guid, entities, Instant::now())?;
                Ok(Some(encode_response_json(&resp)?))
            }

            // Sub-opcode 3: Deposit unbanked cargo into Home Orbital Station vault
            3 => {
                if !payload.is_empty() {
                    let _: serde_json::Value = parse_json_payload(payload)?;
                }
                let resp = self.deposit_to_station_vault(entity_guid, entities)?;
                Ok(Some(encode_response_json(&resp)?))
            }

            // Sub-opcode 4: Start player-to-player trade session
            4 => {
                let req: PlayerTradeStartRequest = parse_json_payload(payload)?;
                let target = Guid::from_raw(req.target_guid);
                if entity_guid.entity_type() != EntityType::Player
                    || target.entity_type() != EntityType::Player
                {
                    return Err(PacketHandlerError::Custom(
                        "Pilot trade requires a player target".to_string(),
                    ));
                }
                let mut trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
                trade_mgr
                    .start_session(entity_guid, target, entities)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                let resp = build_asteroid_state_response(
                    entity_guid,
                    entities,
                    &trade_mgr,
                    Some(target),
                    false,
                    None,
                );
                Ok(Some(encode_response_json(&resp)?))
            }

            // Sub-opcode 5: Submit or revise trade offer
            5 => {
                let req: PlayerTradeOfferRequest = parse_json_payload(payload)?;
                let resp = self.handle_trade_offer(entity_guid, req, entities)?;
                Ok(Some(encode_response_json(&resp)?))
            }

            // Sub-opcode 6: Lock and accept trade (updates both ships' shininess on swap)
            6 => {
                let req: PlayerTradeSessionRequest = parse_json_payload(payload)?;
                let resp = self.handle_trade_accept(entity_guid, req, entities)?;
                Ok(Some(encode_response_json(&resp)?))
            }

            // Sub-opcode 7: Cancel active trade session
            7 => {
                let req: PlayerTradeSessionRequest = parse_json_payload(payload)?;
                let mut trade_mgr = self.trade_manager.lock().unwrap_or_else(|e| e.into_inner());
                if trade_mgr.get_session(req.session_id).is_none() {
                    return Err(PacketHandlerError::Custom(format!(
                        "Active trade session {} not found",
                        req.session_id
                    )));
                }
                trade_mgr
                    .cancel_session(req.session_id, entity_guid)
                    .map_err(|e| PacketHandlerError::Custom(e.to_string()))?;
                let resp = build_asteroid_state_response(
                    entity_guid,
                    entities,
                    &trade_mgr,
                    None,
                    false,
                    None,
                );
                Ok(Some(encode_response_json(&resp)?))
            }

            _ => Err(PacketHandlerError::MalformedPayload {
                module_id: ASTEROID_MODULE_ID,
                message: format!("Unknown asteroid mining sub_opcode {}", sub_opcode),
            }),
        }
    }
}

/// Standalone gameplay plugin for Competitive 6DoF Voxel Asteroid Miners.
pub struct AsteroidMiningPlugin {
    pub trade_manager: Arc<Mutex<TradeManager>>,
    pub laser_cooldowns: Arc<Mutex<HashMap<Guid, Instant>>>,
    pending_octree_relocations: PendingOctreeRelocations,
}

impl Default for AsteroidMiningPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl AsteroidMiningPlugin {
    pub const MODULE_ID: u16 = ASTEROID_MODULE_ID;

    pub fn new() -> Self {
        Self {
            trade_manager: Arc::new(Mutex::new(TradeManager::new())),
            laser_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            pending_octree_relocations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_trade_manager(trade_manager: Arc<Mutex<TradeManager>>) -> Self {
        Self {
            trade_manager,
            laser_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            pending_octree_relocations: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl GameplayPlugin for AsteroidMiningPlugin {
    fn id(&self) -> &'static str {
        "asteroid_mining"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.event_queue.register_ephemeral::<VoxelChangeEvent>();
        ctx.event_queue.register_ephemeral::<EntityDiedEvent>();
        ctx.codecs
            .register::<StationVaultComponent>("station_vault");
        ctx.hooks.register_handler(Box::new(PilotStarterKitHook));
        ctx.hooks.register_handler(Box::new(TradeDistanceHook {
            trade_manager: self.trade_manager.clone(),
        }));
        ctx.packets.register(
            Self::MODULE_ID,
            Box::new(AsteroidPacketHandler::with_shared(
                self.trade_manager.clone(),
                self.laser_cooldowns.clone(),
                self.pending_octree_relocations.clone(),
            )),
        );
        ctx.tick_systems
            .push(Box::new(AsteroidMiningTickSystem::with_shared(
                self.trade_manager.clone(),
                self.laser_cooldowns.clone(),
                self.pending_octree_relocations.clone(),
            )));
    }
}

/// Builds a procedural 16x16x16 asteroid chunk with crust rock, Titanium, Solar Gold,
/// and a Void Crystal core, padded by vacuum along chunk borders so all exterior faces mesh.
fn generate_asteroid_chunk(coord: VoxelChunkCoord) -> PaletteChunk {
    let mut voxels = [VOXEL_EMPTY; CHUNK_VOXELS];
    let seed_salt = coord.x.unsigned_abs() as usize * 7
        + coord.y.unsigned_abs() as usize * 13
        + coord.z.unsigned_abs() as usize * 19;

    for lz in 1..(CHUNK_SIZE - 1) {
        for ly in 1..(CHUNK_SIZE - 1) {
            for lx in 1..(CHUNK_SIZE - 1) {
                let dx = lx as f32 - 7.5;
                let dy = ly as f32 - 7.5;
                let dz = lz as f32 - 7.5;
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                let bump = ((lx * 31 + ly * 17 + lz * 13 + seed_salt) % 5) as f32 * 0.2;

                let voxel = if dist <= 1.8 {
                    VOXEL_VOID_CRYSTAL
                } else if dist <= 3.1 {
                    VOXEL_SOLAR_GOLD
                } else if dist <= 4.5 {
                    VOXEL_TITANIUM
                } else if dist <= 5.6 + bump {
                    let surface_hash = (lx * 3 + ly * 5 + lz * 7 + seed_salt) % 11;
                    if surface_hash == 0 {
                        VOXEL_SOLAR_GOLD
                    } else if surface_hash <= 2 {
                        VOXEL_TITANIUM
                    } else {
                        VOXEL_CRUST_ROCK
                    }
                } else {
                    VOXEL_EMPTY
                };

                voxels[index_from_local(lx, ly, lz)] = voxel;
            }
        }
    }

    PaletteChunk::from_raw_voxels(&voxels)
}

/// Seeds volumetric 3D voxel asteroids around the Home Orbital Station sector.
pub fn seed_asteroid_voxel_world() -> VoxelWorld {
    let world = VoxelWorld::new(ExcavationConfig {
        max_reach: 20.0,
        reach_tolerance: 1.0,
        cooldown_ms: 100,
    });

    for coord in SEEDED_ASTEROID_CHUNKS {
        world.insert_chunk(coord, generate_asteroid_chunk(coord));
    }

    world
}

/// Bootstraps a `TickSimulation` for Game B (Competitive 6DoF Voxel Asteroid Miners):
/// configures `MotionMode::Newtonian6DoF`, installs shared (`InventoryPlugin`, `TradePlugin`)
/// and exclusive (`CombatPlugin`, `VoxelTerrainPlugin`, `AsteroidMiningPlugin`) plugins,
/// and spawns the Home Orbital Station `WorldObject` at `(0.0, 0.0, 0.0)`.
pub fn bootstrap_asteroid_world(sim: &mut TickSimulation) {
    sim.set_default_spawn_position(DEFAULT_SHIP_SPAWN_POS);
    sim.set_default_motion_mode(MotionMode::Newtonian6DoF {
        max_acceleration: 250.0,
        linear_damping: 0.2,
    });

    let voxel_world = seed_asteroid_voxel_world();

    // Register VoxelTerrainPlugin and CombatPlugin BEFORE AsteroidMiningPlugin so tick events
    // emitted during the tick are observed by AsteroidMiningTickSystem on the same tick.
    sim.add_plugin(Box::new(InventoryPlugin));
    sim.add_plugin(Box::new(TradePlugin));
    sim.add_plugin(Box::new(CombatPlugin));
    sim.add_plugin(Box::new(VoxelTerrainPlugin::new(voxel_world)));
    sim.add_plugin(Box::new(AsteroidMiningPlugin::new()));
    sim.init_plugins();

    let seq = sim.next_world_object_seq;
    sim.next_world_object_seq = sim.next_world_object_seq.wrapping_add(1);
    let station_guid = Guid::new(EntityType::DynamicObject, sim.node_id, seq);
    let mut station_obj = WorldObject::new(
        station_guid,
        STATION_PREFAB_ID,
        Transform3D::new(HOME_STATION_POS, (0.0, 0.0, 0.0, 1.0), (6.0, 6.0, 6.0)),
        BoundingBox3D::new((-3.0, -3.0, -3.0), (3.0, 3.0, 3.0)),
        ContainerOwner::System("asteroid".to_string()),
    );
    station_obj.state_flags |= world_object_flags::INTERACTIVE;
    sim.insert_world_object(station_obj);

    for (idx, &anchor) in ROGUE_DRONE_SPAWNS.iter().enumerate() {
        let drone_guid = Guid::new(EntityType::Npc, sim.node_id, (idx + 1) as u32);
        let start_pos = (anchor.0 + 4.5, anchor.1, anchor.2);
        sim.spawn_entity(drone_guid, start_pos);
        if let Some(drone_ent) = sim.entity_store.get_mut_by_guid(drone_guid) {
            let mut inv = InventoryComponent::new(16, 150.0);
            restock_drone_cargo(&mut inv);
            drone_ent.add_component(inv);
            drone_ent.add_component(HealthComponent::new(100.0, 0.5));
            drone_ent.add_component(StationVaultComponent::default());
            drone_ent.add_component(DronePatrolState {
                anchor,
                radius: 4.5,
                angular_speed: if idx == 0 { 0.45 } else { -0.4 },
                angle: 0.0,
            });
            sync_entity_health_properties(drone_ent);
            update_entity_shininess(drone_ent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::DamageEvent;
    use mmo_core::plugin::ComponentCodecRegistry;
    use mmo_core::ComponentStore;
    use mmo_plugin_voxel::{
        VoxelCoord, VoxelModifyRequestPacket, VoxelModifyResponsePacket, SUB_OPCODE_MODIFY_REQUEST,
        VOXEL_MODULE_ID,
    };

    fn setup_asteroid_sim() -> TickSimulation {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut sim = TickSimulation::new(1, bounds);
        bootstrap_asteroid_world(&mut sim);
        sim
    }

    #[test]
    fn test_station_vault_codec_roundtrip() {
        let sim = setup_asteroid_sim();
        let mut store = ComponentStore::new();
        let vault = StationVaultComponent {
            banked_credits: 450,
            banked_items: vec![
                ItemStack::new(ITEM_TITANIUM_ORE_ID, "Titanium Ore", 5, 2.0),
                ItemStack::new(ITEM_VOID_CRYSTAL_ID, "Void Crystal", 2, 1.0),
            ],
            kills: 3,
            deaths: 1,
        };
        store.add_component(vault.clone());

        let json = sim.codecs.serialize_all(&store);
        assert!(json.get("station_vault").is_some());

        let mut restored_json = ComponentStore::new();
        sim.codecs
            .deserialize_into(&json, &mut restored_json)
            .expect("json deserialize");
        assert_eq!(
            restored_json.get_component::<StationVaultComponent>(),
            Some(&vault)
        );

        let bin = sim.codecs.serialize_all_binary(&store).expect("binary enc");
        let mut restored_bin = ComponentStore::new();
        sim.codecs
            .deserialize_binary_into(&bin, &mut restored_bin)
            .expect("binary dec");
        assert_eq!(
            restored_bin.get_component::<StationVaultComponent>(),
            Some(&vault)
        );

        // Standalone registry check
        let mut codecs = ComponentCodecRegistry::new();
        codecs.register::<StationVaultComponent>("station_vault");
        let serialized = codecs.serialize_all(&store);
        let mut out = ComponentStore::new();
        assert_eq!(codecs.deserialize_into(&serialized, &mut out).unwrap(), 1);
    }

    #[test]
    fn test_shininess_tier_and_state_flags_encoding() {
        assert_eq!(ore_unit_value(ITEM_TITANIUM_ORE_ID), 10);
        assert_eq!(ore_unit_value(ITEM_SOLAR_GOLD_ID), 30);
        assert_eq!(ore_unit_value(ITEM_VOID_CRYSTAL_ID), 75);
        assert_eq!(ore_unit_value(9999), 0);

        let mut inv = InventoryComponent::new(16, 150.0).with_currency(25);
        inv.add_item(ItemStack::new(ITEM_TITANIUM_ORE_ID, "Titanium Ore", 2, 2.0))
            .unwrap();
        inv.add_item(ItemStack::new(ITEM_VOID_CRYSTAL_ID, "Void Crystal", 1, 1.0))
            .unwrap();
        // 2*10 + 75 + 25 = 120 -> 120 / 5 = 24
        assert_eq!(compute_cargo_value(&inv), 120);
        assert_eq!(compute_shininess_tier(120), 24);
        assert_eq!(compute_shininess_tier(5_000), 255);

        let base_flags = state_flags::COMBAT_ACTIVE | (0xABu32 << 24);
        let encoded = encode_shininess_state_flags(base_flags, 173);
        assert_eq!(extract_shininess_tier(encoded), 173);
        assert_ne!(encoded & state_flags::COMBAT_ACTIVE, 0);
        assert_eq!((encoded >> 24) & 0xFF, 0xAB);
    }

    #[test]
    fn test_bootstrap_asteroid_world_and_pilot_starter_kit() {
        let mut sim = setup_asteroid_sim();
        assert_eq!(sim.default_spawn_position, DEFAULT_SHIP_SPAWN_POS);
        assert_eq!(
            sim.default_motion_mode,
            MotionMode::Newtonian6DoF {
                max_acceleration: 250.0,
                linear_damping: 0.2,
            }
        );

        // Home Orbital Station WorldObject exists at (0, 0, 0) with prefab_id 100
        let objs = sim.sectors.spatial_index().len();
        assert_eq!(objs, 1);
        let station_guid = Guid::new(EntityType::DynamicObject, sim.node_id, 1);
        let station = sim.sectors.get_object(station_guid).expect("station");
        assert_eq!(station.prefab_id, STATION_PREFAB_ID);
        assert_eq!(station.transform.position, HOME_STATION_POS);

        // Hydrate new player session -> 6DoF kinematic state + starter components
        let p1 = Guid::new(EntityType::Player, 1, 101);
        let pos = sim.hydrate_player_session(p1, 1, 1, None, 80.0);
        assert_eq!(pos, DEFAULT_SHIP_SPAWN_POS);
        assert_eq!(
            sim.kinematic_states[&p1].motion_mode,
            sim.default_motion_mode
        );

        // Trigger OnSpawn hook as handle_client_input does
        sim.hooks.dispatch_with_entities(
            &Event::OnSpawn {
                entity: p1,
                position: pos,
            },
            &mut sim.entity_store,
        );

        let ent = sim.entity_store.get_by_guid(p1).unwrap();
        let inv = ent.get_component::<InventoryComponent>().unwrap();
        assert_eq!(inv.slots.len(), 16);
        assert_eq!(inv.max_weight, 150.0);
        assert_eq!(inv.currency, 0);

        let hp = ent.get_component::<HealthComponent>().unwrap();
        assert_eq!(hp.current_hp, 100.0);
        assert_eq!(hp.max_hp, 100.0);
        assert!(ent.get_component::<StationVaultComponent>().is_some());
        assert_eq!(extract_shininess_tier(ent.state_flags), 0);

        // 6DoF 3D movement (including Y-axis thrust) succeeds from spawn
        let move_res = sim.process_client_move_deterministic(p1, (0.2, 0.3, 8.2), 0.05);
        assert!(move_res.is_ok(), "6DoF movement from spawn must succeed");
    }

    #[test]
    fn test_seeded_voxel_asteroids_and_ore_harvesting_tick() {
        let mut sim = setup_asteroid_sim();
        let p1 = Guid::new(EntityType::Player, 1, 201);
        sim.spawn_entity(p1, (7.5, 7.5, -18.5));

        let chunk_coord = VoxelChunkCoord::new(0, 0, -2);
        // Check deterministic positions inside chunk (0, 0, -2) (world x/y in [0..16), z in [-32..-16)):
        // local (7,7,7) -> world (7, 7, -25) = Void Crystal (4)
        // local (5,7,7) -> world (5, 7, -25) = Solar Gold (3)
        // local (4,7,7) -> world (4, 7, -25) = Titanium (2)
        // local (2,7,7) -> world (2, 7, -25) = Crust (1) or surface vein
        let target_void = VoxelCoord::new(7, 7, -25);
        let target_gold = VoxelCoord::new(5, 7, -25);
        let target_ti = VoxelCoord::new(4, 7, -25);

        // Move ship within reach of core and clear vacuum/crust if needed by testing direct packets
        for (idx, target) in [target_ti, target_gold, target_void]
            .into_iter()
            .enumerate()
        {
            sim.advance_time(Duration::from_millis(200));
            // Wait slightly in real time or pace ExcavationRateLimiter using std::thread::sleep only if needed;
            // here VoxelPacketHandler uses SystemTime::now(), so space calls by >100ms.
            if idx > 0 {
                std::thread::sleep(Duration::from_millis(110));
            }
            let req = VoxelModifyRequestPacket {
                target_voxel: target,
                new_voxel: VOXEL_EMPTY,
            };
            let resp_bytes = sim
                .packets
                .dispatch(
                    VOXEL_MODULE_ID,
                    SUB_OPCODE_MODIFY_REQUEST,
                    p1,
                    &req.encode(),
                    &mut sim.entity_store,
                )
                .expect("dispatch ok")
                .expect("resp present");
            let resp = VoxelModifyResponsePacket::decode(&resp_bytes).unwrap();
            assert_eq!(resp.status, 0, "excavation {} must succeed", idx);
        }

        // Verify chunk_coord exists and can be fetched via VOXEL_MODULE_ID SUB_OPCODE_CHUNK_REQUEST
        assert!(sim.packets.has_module(VOXEL_MODULE_ID));
        let chunk_req = mmo_plugin_voxel::VoxelChunkRequestPacket { chunk: chunk_coord };
        let chunk_resp = sim
            .packets
            .dispatch(
                VOXEL_MODULE_ID,
                mmo_plugin_voxel::SUB_OPCODE_CHUNK_REQUEST,
                p1,
                &chunk_req.encode(),
                &mut sim.entity_store,
            )
            .expect("chunk req dispatch")
            .expect("chunk data returned");
        let decoded_chunk =
            mmo_plugin_voxel::VoxelChunkDataPacket::decode(&chunk_resp).expect("decode chunk");
        assert_eq!(decoded_chunk.chunk, chunk_coord);
        assert!(!decoded_chunk.rle_payload.is_empty());

        // Mining VOXEL_CRUST_ROCK (1) yields zero ore
        sim.event_queue.push(VoxelChangeEvent {
            entity_guid: p1,
            chunk_coord,
            voxel_coord: VoxelCoord::new(2, 7, -25),
            local_index: 0,
            old_voxel: VOXEL_CRUST_ROCK,
            new_voxel: VOXEL_EMPTY,
        });

        // Tick 1: VoxelTickSystem drains events -> AsteroidMiningTickSystem harvests all 3 ores (crust ignored)
        sim.step_tick();

        {
            let ent = sim.entity_store.get_by_guid(p1).unwrap();
            let inv = ent.get_component::<InventoryComponent>().unwrap();
            assert!(inv.has_item_stack(&ItemStack::new(
                ITEM_TITANIUM_ORE_ID,
                "Titanium Ore",
                1,
                2.0
            )));
            assert!(inv.has_item_stack(&ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0)));
            assert!(inv.has_item_stack(&ItemStack::new(
                ITEM_VOID_CRYSTAL_ID,
                "Void Crystal",
                1,
                1.0
            )));
            // Value: 10 + 30 + 75 = 115 -> tier = 115 / 5 = 23
            assert_eq!(compute_cargo_value(inv), 115);
            assert_eq!(extract_shininess_tier(ent.state_flags), 23);
        }

        // Tick 2: Ephemeral VoxelChangeEvents must NOT be double-credited
        sim.step_tick();
        {
            let ent = sim.entity_store.get_by_guid(p1).unwrap();
            let inv = ent.get_component::<InventoryComponent>().unwrap();
            assert_eq!(compute_cargo_value(inv), 115);
        }
    }

    #[test]
    fn test_ore_harvest_respects_weight_and_merges_stacks() {
        let mut sim = setup_asteroid_sim();
        let p1 = Guid::new(EntityType::Player, 1, 205);
        sim.spawn_entity(p1, (0.0, 0.0, 20.0));

        // Shrink capacity so only 1 Titanium Ore (weight 2.0) fits
        {
            let ent = sim.entity_store.get_mut_by_guid(p1).unwrap();
            let inv = ent.get_component_mut::<InventoryComponent>().unwrap();
            inv.max_weight = 4.5;
        }

        for _ in 0..3 {
            sim.event_queue.push(VoxelChangeEvent {
                entity_guid: p1,
                chunk_coord: VoxelChunkCoord::new(0, 0, -2),
                voxel_coord: VoxelCoord::new(4, 7, -25),
                local_index: 0,
                old_voxel: VOXEL_TITANIUM,
                new_voxel: VOXEL_EMPTY,
            });
        }

        sim.step_tick();

        let ent = sim.entity_store.get_by_guid(p1).unwrap();
        let inv = ent.get_component::<InventoryComponent>().unwrap();
        // 2 Titanium Ore (4.0 kg) fit into max_weight 4.5 and merge into a single stack
        assert_eq!(inv.slots[0].as_ref().unwrap().quantity, 2);
        assert!(inv.slots[1].is_none());
        assert!(!inv.is_encumbered());
        assert_eq!(extract_shininess_tier(ent.state_flags), 4);
    }

    #[test]
    fn test_pvp_laser_safe_zone_range_cooldown_and_full_loot_respawn() {
        let mut sim = setup_asteroid_sim();
        let attacker = Guid::new(EntityType::Player, 1, 301);
        let victim = Guid::new(EntityType::Player, 1, 302);

        // Spawn both outside safe zone initially
        sim.spawn_entity(attacker, (25.0, 0.0, 0.0));
        sim.spawn_entity(victim, (40.0, 0.0, 0.0));

        // Give victim rich unbanked loot (Solar Gold + Void Crystal + 50 credits = 155 value -> tier 31)
        {
            let v_ent = sim.entity_store.get_mut_by_guid(victim).unwrap();
            let v_inv = v_ent.get_component_mut::<InventoryComponent>().unwrap();
            v_inv
                .add_item(ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0))
                .unwrap();
            v_inv
                .add_item(ItemStack::new(ITEM_VOID_CRYSTAL_ID, "Void Crystal", 1, 1.0))
                .unwrap();
            v_inv.currency = 50;
            update_entity_shininess(v_ent);
            assert_eq!(extract_shininess_tier(v_ent.state_flags), 31);
        }

        let handler = AsteroidPacketHandler::new(Arc::new(Mutex::new(TradeManager::new())));

        // 1. Self-fire rejected
        let self_req = serde_json::to_vec(&FirePvpLaserRequest {
            target_guid: attacker.raw(),
        })
        .unwrap();
        assert!(handler
            .handle(attacker, 2, &self_req, &mut sim.entity_store)
            .is_err());

        // 2. Out-of-range rejected (> 45.0m)
        sim.entity_store.get_mut_by_guid(victim).unwrap().position = (80.0, 0.0, 0.0);
        let fire_req = serde_json::to_vec(&FirePvpLaserRequest {
            target_guid: victim.raw(),
        })
        .unwrap();
        assert!(handler
            .handle(attacker, 2, &fire_req, &mut sim.entity_store)
            .is_err());

        // 3. Safe-zone protection: target inside 15.0m station radius rejected
        sim.entity_store.get_mut_by_guid(victim).unwrap().position = (0.0, 0.0, 10.0);
        assert!(handler
            .handle(attacker, 2, &fire_req, &mut sim.entity_store)
            .is_err());

        // 4. Safe-zone protection: attacker inside 15.0m station radius rejected
        sim.entity_store.get_mut_by_guid(victim).unwrap().position = (30.0, 0.0, 0.0);
        sim.entity_store.get_mut_by_guid(attacker).unwrap().position = (0.0, 0.0, 5.0);
        assert!(handler
            .handle(attacker, 2, &fire_req, &mut sim.entity_store)
            .is_err());
        assert_eq!(
            sim.entity_store
                .get_by_guid(victim)
                .unwrap()
                .get_component::<HealthComponent>()
                .unwrap()
                .current_hp,
            100.0
        );

        // 5. Move attacker outside safe zone & test deterministic 500ms cooldown + 4-shot kill
        sim.entity_store.get_mut_by_guid(attacker).unwrap().position = (20.0, 0.0, 0.0);
        let t0 = Instant::now();

        // Shot 1 -> 75 HP
        let resp1 = handler
            .fire_pvp_laser_at(attacker, victim, &mut sim.entity_store, t0)
            .expect("shot 1 succeeds");
        assert_eq!(resp1.target_info.as_ref().unwrap().hull_hp, 75.0);

        // Cooldown violation at t0 + 200ms (< 500ms)
        assert!(handler
            .fire_pvp_laser_at(
                attacker,
                victim,
                &mut sim.entity_store,
                t0 + Duration::from_millis(200),
            )
            .is_err());

        // Shot 2 at t0 + 500ms -> 50 HP
        let resp2 = handler
            .fire_pvp_laser_at(
                attacker,
                victim,
                &mut sim.entity_store,
                t0 + Duration::from_millis(500),
            )
            .expect("shot 2 succeeds");
        assert_eq!(resp2.target_info.as_ref().unwrap().hull_hp, 50.0);

        // Shot 3 at t0 + 1000ms -> 25 HP
        handler
            .fire_pvp_laser_at(
                attacker,
                victim,
                &mut sim.entity_store,
                t0 + Duration::from_millis(1000),
            )
            .expect("shot 3 succeeds");

        // Shot 4 at t0 + 1500ms -> lethal! Full loot transfer + victim respawn at station dock
        let resp4 = handler
            .fire_pvp_laser_at(
                attacker,
                victim,
                &mut sim.entity_store,
                t0 + Duration::from_millis(1500),
            )
            .expect("shot 4 lethal succeeds");

        // Attacker received all victim loot & shininess tier 31 + 1 kill
        assert_eq!(resp4.shininess_tier, 31);
        assert_eq!(resp4.unbanked_cargo_value, 155);
        assert_eq!(resp4.station_vault.as_ref().unwrap().kills, 1);

        let a_ent = sim.entity_store.get_by_guid(attacker).unwrap();
        let a_inv = a_ent.get_component::<InventoryComponent>().unwrap();
        assert!(a_inv.has_item_stack(&ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0)));
        assert!(a_inv.has_item_stack(&ItemStack::new(
            ITEM_VOID_CRYSTAL_ID,
            "Void Crystal",
            1,
            1.0
        )));
        assert_eq!(a_inv.currency, 50);

        // Victim respawned at DEFAULT_SHIP_SPAWN_POS with 100 HP, 0 loot, shininess 0, 1 death
        let v_ent = sim.entity_store.get_by_guid(victim).unwrap();
        assert_eq!(v_ent.position, DEFAULT_SHIP_SPAWN_POS);
        assert_eq!(v_ent.velocity, (0.0, 0.0, 0.0));
        assert!(!v_ent.is_incapacitated());
        assert_eq!(extract_shininess_tier(v_ent.state_flags), 0);
        let v_hp = v_ent.get_component::<HealthComponent>().unwrap();
        assert!(!v_hp.is_dead);
        assert_eq!(v_hp.current_hp, 100.0);
        assert_eq!(
            v_ent
                .get_component::<StationVaultComponent>()
                .unwrap()
                .deaths,
            1
        );
        let v_inv = v_ent.get_component::<InventoryComponent>().unwrap();
        assert_eq!(compute_cargo_value(v_inv), 0);
    }

    #[test]
    fn test_entity_died_event_in_tick_system_full_loot_and_respawn() {
        let mut sim = setup_asteroid_sim();
        let killer = Guid::new(EntityType::Player, 1, 401);
        let victim = Guid::new(EntityType::Player, 1, 402);

        sim.spawn_entity(killer, (30.0, 0.0, 0.0));
        sim.spawn_entity(victim, (35.0, 0.0, 0.0));

        {
            let v_ent = sim.entity_store.get_mut_by_guid(victim).unwrap();
            v_ent
                .get_component_mut::<InventoryComponent>()
                .unwrap()
                .add_item(ItemStack::new(ITEM_TITANIUM_ORE_ID, "Titanium Ore", 3, 2.0))
                .unwrap();
            update_entity_shininess(v_ent);
        }

        // Push lethal CombatPlugin DamageEvent
        sim.event_queue.push(DamageEvent {
            source: killer,
            target: victim,
            raw_damage: 150.0,
        });

        sim.step_tick();

        let k_ent = sim.entity_store.get_by_guid(killer).unwrap();
        assert_eq!(extract_shininess_tier(k_ent.state_flags), 6); // 30 / 5 = 6
        assert_eq!(
            k_ent
                .get_component::<StationVaultComponent>()
                .unwrap()
                .kills,
            1
        );

        let v_ent = sim.entity_store.get_by_guid(victim).unwrap();
        assert_eq!(v_ent.position, DEFAULT_SHIP_SPAWN_POS);
        assert!(!v_ent.is_incapacitated());
        assert_eq!(
            v_ent.get_component::<HealthComponent>().unwrap().current_hp,
            100.0
        );
        assert_eq!(extract_shininess_tier(v_ent.state_flags), 0);
        // Respawned victim must be able to move again
        assert!(sim
            .process_client_move_deterministic(victim, (0.0, 0.0, 8.2), 0.05)
            .is_ok());

        // Subsequent tick must not re-process cleared EntityDiedEvent
        sim.step_tick();
        assert_eq!(
            sim.entity_store
                .get_by_guid(killer)
                .unwrap()
                .get_component::<StationVaultComponent>()
                .unwrap()
                .kills,
            1
        );
    }

    #[test]
    fn test_station_vault_deposit_and_two_player_trade_updates_shininess() {
        let mut sim = setup_asteroid_sim();
        let p1 = Guid::new(EntityType::Player, 1, 501);
        let p2 = Guid::new(EntityType::Player, 1, 502);

        sim.spawn_entity(p1, (30.0, 0.0, 0.0));
        sim.spawn_entity(p2, (0.0, 0.0, 9.0));

        // Give P1 unbanked Titanium + Solar Gold
        {
            let e1 = sim.entity_store.get_mut_by_guid(p1).unwrap();
            let inv1 = e1.get_component_mut::<InventoryComponent>().unwrap();
            inv1.add_item(ItemStack::new(ITEM_TITANIUM_ORE_ID, "Titanium Ore", 2, 2.0))
                .unwrap();
            inv1.add_item(ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0))
                .unwrap();
            update_entity_shininess(e1);
            // 2*10 + 30 = 50 -> tier 10
            assert_eq!(extract_shininess_tier(e1.state_flags), 10);
        }

        // 1. Deposit outside safe zone must fail
        let deposit_err = sim
            .packets
            .dispatch(ASTEROID_MODULE_ID, 3, p1, b"{}", &mut sim.entity_store)
            .unwrap_err();
        assert!(matches!(deposit_err, PacketHandlerError::Custom(_)));

        // 2. Move P1 inside Home Orbital Station docking ring -> Deposit succeeds
        sim.entity_store.get_mut_by_guid(p1).unwrap().position = (0.0, 0.0, 8.0);
        let dep_bytes = sim
            .packets
            .dispatch(ASTEROID_MODULE_ID, 3, p1, b"{}", &mut sim.entity_store)
            .expect("deposit ok")
            .expect("resp");
        let dep_state: AsteroidSectorStateResponse = serde_json::from_slice(&dep_bytes).unwrap();
        assert!(dep_state.in_safe_zone);
        assert_eq!(dep_state.shininess_tier, 0);
        assert_eq!(dep_state.unbanked_cargo_value, 0);
        let vault = dep_state.station_vault.unwrap();
        assert_eq!(vault.banked_credits, 50);
        assert_eq!(vault.banked_items.len(), 2);

        // 3. Give P1 Void Crystal (75 -> tier 15) and P2 40 unbanked credits (tier 8), then trade!
        {
            let e1 = sim.entity_store.get_mut_by_guid(p1).unwrap();
            e1.get_component_mut::<InventoryComponent>()
                .unwrap()
                .add_item(ItemStack::new(ITEM_VOID_CRYSTAL_ID, "Void Crystal", 1, 1.0))
                .unwrap();
            update_entity_shininess(e1);

            let e2 = sim.entity_store.get_mut_by_guid(p2).unwrap();
            e2.get_component_mut::<InventoryComponent>()
                .unwrap()
                .currency = 40;
            update_entity_shininess(e2);
        }

        // Start trade (sub-opcode 4)
        let start_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: p2.raw(),
        })
        .unwrap();
        let start_bytes = sim
            .packets
            .dispatch(ASTEROID_MODULE_ID, 4, p1, &start_req, &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let start_state: AsteroidSectorStateResponse =
            serde_json::from_slice(&start_bytes).unwrap();
        let session_id = start_state.active_trade.unwrap().session_id;

        // Offers (sub-opcode 5)
        let offer_p1 = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![ItemStack::new(ITEM_VOID_CRYSTAL_ID, "Void Crystal", 1, 1.0)],
            currency: 0,
        })
        .unwrap();
        sim.packets
            .dispatch(ASTEROID_MODULE_ID, 5, p1, &offer_p1, &mut sim.entity_store)
            .unwrap();

        let offer_p2 = serde_json::to_vec(&PlayerTradeOfferRequest {
            session_id,
            items: vec![],
            currency: 40,
        })
        .unwrap();
        sim.packets
            .dispatch(ASTEROID_MODULE_ID, 5, p2, &offer_p2, &mut sim.entity_store)
            .unwrap();

        // Lock P1, Lock P2, Accept P1, Accept P2 (sub-opcode 6)
        let session_req = serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap();
        sim.packets
            .dispatch(
                ASTEROID_MODULE_ID,
                6,
                p1,
                &session_req,
                &mut sim.entity_store,
            )
            .unwrap();
        sim.packets
            .dispatch(
                ASTEROID_MODULE_ID,
                6,
                p2,
                &session_req,
                &mut sim.entity_store,
            )
            .unwrap();
        sim.packets
            .dispatch(
                ASTEROID_MODULE_ID,
                6,
                p1,
                &session_req,
                &mut sim.entity_store,
            )
            .unwrap();
        let final_bytes = sim
            .packets
            .dispatch(
                ASTEROID_MODULE_ID,
                6,
                p2,
                &session_req,
                &mut sim.entity_store,
            )
            .unwrap()
            .unwrap();
        let final_state: AsteroidSectorStateResponse =
            serde_json::from_slice(&final_bytes).unwrap();
        assert!(final_state.trade_completed);

        // Both ships' shininess tiers swapped accurately:
        // P1 now has 40 credits -> value 40 -> tier 8
        // P2 now has 1 Void Crystal -> value 75 -> tier 15
        let e1 = sim.entity_store.get_by_guid(p1).unwrap();
        let e2 = sim.entity_store.get_by_guid(p2).unwrap();
        assert_eq!(extract_shininess_tier(e1.state_flags), 8);
        assert_eq!(extract_shininess_tier(e2.state_flags), 15);
    }

    #[test]
    fn test_inspect_trade_cancel_and_distance_abort() {
        let mut sim = setup_asteroid_sim();
        let p1 = Guid::new(EntityType::Player, 1, 601);
        let p2 = Guid::new(EntityType::Player, 1, 602);

        sim.hydrate_player_session(p1, 11, 1, None, 80.0);
        sim.hydrate_player_session(p2, 12, 1, None, 80.0);
        for guid in [p1, p2] {
            let pos = sim.entity_store.get_by_guid(guid).unwrap().position;
            sim.hooks.dispatch_with_entities(
                &Event::OnSpawn {
                    entity: guid,
                    position: pos,
                },
                &mut sim.entity_store,
            );
        }

        // Inspect P2 from P1 (sub-opcode 1)
        let inspect_req = serde_json::to_vec(&InspectRequest {
            target_guid: Some(p2.raw()),
        })
        .unwrap();
        let inspect_bytes = sim
            .packets
            .dispatch(
                ASTEROID_MODULE_ID,
                1,
                p1,
                &inspect_req,
                &mut sim.entity_store,
            )
            .unwrap()
            .unwrap();
        let inspect_state: AsteroidSectorStateResponse =
            serde_json::from_slice(&inspect_bytes).unwrap();
        assert!(inspect_state.in_safe_zone);
        let target_info = inspect_state.target_info.expect("target info present");
        assert_eq!(target_info.target_guid, p2.raw());
        assert_eq!(target_info.hull_hp, 100.0);
        assert!(target_info.in_safe_zone);
        assert_eq!(target_info.trade_status.as_deref(), Some("available"));

        // Start trade (sub-opcode 4), then cancel (sub-opcode 7)
        let start_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: p2.raw(),
        })
        .unwrap();
        let start_bytes = sim
            .packets
            .dispatch(ASTEROID_MODULE_ID, 4, p1, &start_req, &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let start_state: AsteroidSectorStateResponse =
            serde_json::from_slice(&start_bytes).unwrap();
        let session_id = start_state.active_trade.unwrap().session_id;

        let cancel_req = serde_json::to_vec(&PlayerTradeSessionRequest { session_id }).unwrap();
        let cancel_bytes = sim
            .packets
            .dispatch(
                ASTEROID_MODULE_ID,
                7,
                p1,
                &cancel_req,
                &mut sim.entity_store,
            )
            .unwrap()
            .unwrap();
        let cancel_state: AsteroidSectorStateResponse =
            serde_json::from_slice(&cancel_bytes).unwrap();
        assert!(cancel_state.active_trade.is_none());

        // Start a second trade and verify TradeDistanceHook aborts when P2 moves > 5m away
        sim.packets
            .dispatch(ASTEROID_MODULE_ID, 4, p1, &start_req, &mut sim.entity_store)
            .unwrap();
        let move_res = sim.process_client_move_deterministic(p2, (0.0, 0.0, 14.0), 1.0);
        assert!(move_res.is_ok());
        // Inspect P1 -> active_trade should be canceled by TradeDistanceHook on BeforeMove
        let after_move_bytes = sim
            .packets
            .dispatch(ASTEROID_MODULE_ID, 1, p1, b"{}", &mut sim.entity_store)
            .unwrap()
            .unwrap();
        let after_move_state: AsteroidSectorStateResponse =
            serde_json::from_slice(&after_move_bytes).unwrap();
        assert!(after_move_state.active_trade.is_none());
    }

    #[test]
    fn test_all_seeded_asteroid_chunks_populated_outside_safe_zone() {
        assert_eq!(SEEDED_ASTEROID_CHUNKS.len(), 10);
        let world = seed_asteroid_voxel_world();

        for coord in SEEDED_ASTEROID_CHUNKS {
            let chunk_arc = world
                .get_chunk(coord)
                .unwrap_or_else(|| panic!("seeded chunk {:?} must exist", coord));
            let chunk = chunk_arc.read().unwrap();

            let mut solid_count = 0usize;
            let mut ore_count = 0usize;
            let mut crust_count = 0usize;

            for lz in 0..CHUNK_SIZE {
                for ly in 0..CHUNK_SIZE {
                    for lx in 0..CHUNK_SIZE {
                        let voxel = chunk.get(index_from_local(lx, ly, lz));
                        if voxel == VOXEL_EMPTY {
                            continue;
                        }
                        solid_count += 1;
                        if voxel == VOXEL_CRUST_ROCK {
                            crust_count += 1;
                        } else if matches!(
                            voxel,
                            VOXEL_TITANIUM | VOXEL_SOLAR_GOLD | VOXEL_VOID_CRYSTAL
                        ) {
                            ore_count += 1;
                        }

                        let voxel_pos = coord.voxel_coord(lx, ly, lz).center_world_pos();
                        let dist_station = distance_3d(voxel_pos, HOME_STATION_POS);
                        assert!(
                            dist_station > HOME_STATION_SAFE_RADIUS,
                            "voxel {:?} in chunk {:?} at {:?} is inside safe radius ({:.2} <= {:.2})",
                            voxel,
                            coord,
                            voxel_pos,
                            dist_station,
                            HOME_STATION_SAFE_RADIUS
                        );
                        let dist_spawn = distance_3d(voxel_pos, DEFAULT_SHIP_SPAWN_POS);
                        assert!(
                            dist_spawn > 2.0,
                            "voxel {:?} in chunk {:?} at {:?} is too close to DEFAULT_SHIP_SPAWN_POS ({:.2})",
                            voxel,
                            coord,
                            voxel_pos,
                            dist_spawn
                        );
                    }
                }
            }

            assert!(solid_count > 0, "chunk {:?} must have solid voxels", coord);
            assert!(ore_count > 0, "chunk {:?} must have ore voxels", coord);
            assert!(crust_count > 0, "chunk {:?} must have crust voxels", coord);
        }
    }

    #[test]
    fn test_npc_drone_patrol_octree_sync_and_trade_rejection() {
        let mut sim = setup_asteroid_sim();
        let drone1 = Guid::new(EntityType::Npc, sim.node_id, 1);
        let drone2 = Guid::new(EntityType::Npc, sim.node_id, 2);

        let initial_positions: Vec<(Guid, (f32, f32, f32))> = [drone1, drone2]
            .into_iter()
            .map(|guid| {
                let ent = sim
                    .entity_store
                    .get_by_guid(guid)
                    .expect("patrol drone spawned");
                assert!(!is_in_safe_zone(ent.position));
                assert_eq!(extract_shininess_tier(ent.state_flags), 10);
                assert_eq!(
                    ent.get_component::<HealthComponent>().unwrap().current_hp,
                    100.0
                );
                assert!(ent.get_component::<DronePatrolState>().is_some());
                (guid, ent.position)
            })
            .collect();

        // Advance simulation ticks so both drones orbit along their paths
        for _ in 0..10 {
            sim.step_tick();
        }

        let octree_entries = sim.octree.query_radius_with_pos(HOME_STATION_POS, 100.0);
        for (guid, initial_pos) in initial_positions {
            let ent = sim.entity_store.get_by_guid(guid).unwrap();
            assert_ne!(
                ent.position, initial_pos,
                "drone {:?} must orbit over ticks",
                guid
            );
            assert!(
                !is_in_safe_zone(ent.position),
                "drone {:?} must remain outside 15.0m safe zone",
                guid
            );
            let octree_pos = octree_entries
                .iter()
                .find(|(g, _)| *g == guid)
                .map(|(_, pos)| *pos)
                .expect("drone present in octree");
            assert_eq!(
                octree_pos, ent.position,
                "octree position for {:?} must match entity_store",
                guid
            );
        }

        // Player initiating trade (sub_opcode = 4) against an NPC drone must be rejected
        let player = Guid::new(EntityType::Player, 1, 701);
        sim.spawn_entity(
            player,
            sim.entity_store.get_by_guid(drone1).unwrap().position,
        );
        let trade_req = serde_json::to_vec(&PlayerTradeStartRequest {
            target_guid: drone1.raw(),
        })
        .unwrap();
        let err = sim
            .packets
            .dispatch(
                ASTEROID_MODULE_ID,
                4,
                player,
                &trade_req,
                &mut sim.entity_store,
            )
            .unwrap_err();
        assert!(matches!(err, PacketHandlerError::Custom(_)));
    }

    #[test]
    fn test_npc_drone_destruction_transfers_loot_and_respawns_outside_safe_zone() {
        let mut sim = setup_asteroid_sim();
        let drone_guid = Guid::new(EntityType::Npc, sim.node_id, 1);

        // Advance 2 ticks so drone moves away from angle == 0.0
        sim.step_tick();
        sim.step_tick();
        let moved_pos = sim.entity_store.get_by_guid(drone_guid).unwrap().position;
        assert_ne!(
            sim.entity_store
                .get_by_guid(drone_guid)
                .unwrap()
                .get_component::<DronePatrolState>()
                .unwrap()
                .angle,
            0.0
        );

        let attacker = Guid::new(EntityType::Player, 1, 801);
        let attacker_pos = (moved_pos.0 + 5.0, moved_pos.1, moved_pos.2);
        assert!(!is_in_safe_zone(attacker_pos));
        sim.spawn_entity(attacker, attacker_pos);

        // Destroy the 100 HP drone using 4 authoritative sub_opcode=2 PvP laser dispatches (25 dmg each)
        let fire_req = serde_json::to_vec(&FirePvpLaserRequest {
            target_guid: drone_guid.raw(),
        })
        .unwrap();
        let mut last_resp_bytes = Vec::new();
        for shot in 0..4 {
            if shot > 0 {
                std::thread::sleep(Duration::from_millis(PVP_LASER_COOLDOWN_MS + 20));
            }
            last_resp_bytes = sim
                .packets
                .dispatch(
                    ASTEROID_MODULE_ID,
                    2,
                    attacker,
                    &fire_req,
                    &mut sim.entity_store,
                )
                .expect("laser shot succeeds")
                .expect("response present");
        }
        let final_resp: AsteroidSectorStateResponse =
            serde_json::from_slice(&last_resp_bytes).unwrap();

        // Attacker received 2x Titanium Ore + 1x Solar Gold and +1 kill
        assert_eq!(final_resp.station_vault.as_ref().unwrap().kills, 1);
        assert_eq!(final_resp.shininess_tier, 10);
        let a_ent = sim.entity_store.get_by_guid(attacker).unwrap();
        let a_inv = a_ent.get_component::<InventoryComponent>().unwrap();
        assert!(a_inv.has_item_stack(&ItemStack::new(
            ITEM_TITANIUM_ORE_ID,
            "Titanium Ore",
            2,
            2.0
        )));
        assert!(a_inv.has_item_stack(&ItemStack::new(ITEM_SOLAR_GOLD_ID, "Solar Gold", 1, 3.0)));

        // Drone respawned at its patrol anchor outside 15.0m with 100 HP, angle 0.0, restocked ore
        let expected_respawn = (
            ROGUE_DRONE_SPAWNS[0].0 + 4.5,
            ROGUE_DRONE_SPAWNS[0].1,
            ROGUE_DRONE_SPAWNS[0].2,
        );
        {
            let d_ent = sim.entity_store.get_by_guid(drone_guid).unwrap();
            assert_eq!(d_ent.position, expected_respawn);
            assert!(!is_in_safe_zone(d_ent.position));
            assert_eq!(
                d_ent.get_component::<HealthComponent>().unwrap().current_hp,
                100.0
            );
            assert_eq!(
                d_ent.get_component::<DronePatrolState>().unwrap().angle,
                0.0
            );
            assert_eq!(extract_shininess_tier(d_ent.state_flags), 10);
            let d_inv = d_ent.get_component::<InventoryComponent>().unwrap();
            assert!(d_inv.has_item_stack(&ItemStack::new(
                ITEM_TITANIUM_ORE_ID,
                "Titanium Ore",
                2,
                2.0
            )));
            assert!(d_inv.has_item_stack(&ItemStack::new(
                ITEM_SOLAR_GOLD_ID,
                "Solar Gold",
                1,
                3.0
            )));
        }

        // Next tick flushes pending_octree_relocations cleanly and advances patrol by 1 tick
        sim.step_tick();
        let post_tick_pos = sim.entity_store.get_by_guid(drone_guid).unwrap().position;
        assert!(!is_in_safe_zone(post_tick_pos));
        let octree_entries = sim.octree.query_radius_with_pos(HOME_STATION_POS, 100.0);
        let drone_octree_matches: Vec<(Guid, (f32, f32, f32))> = octree_entries
            .into_iter()
            .filter(|(g, _)| *g == drone_guid)
            .collect();
        assert_eq!(drone_octree_matches.len(), 1);
        assert_eq!(drone_octree_matches[0].1, post_tick_pos);
    }
}
