//! Decoupled MMO Gameplay Systems & Plugins.
//!
//! Exposes standalone gameplay modules implementing [`mmo_core::GameplayPlugin`]:
//! - [`inventory`]: Container bags, equipment slots, encumbrance hooks, and item codecs.
//! - [`combat`]: Health status, armor mitigation, batch tick regeneration, and damage event processing.
//! - [`trade`]: Atomic two-player trade transactions with mutual confirmation and distance guards.

pub mod ability;
pub mod ai_brain;
pub mod combat;
pub mod guild;
pub mod inventory;
pub mod loot;
pub mod navigation;
pub mod navmesh;
pub mod party;
pub mod perception;
pub mod quest;
pub mod scripting;
pub mod spawner;
pub mod threat;
pub mod trade;
pub mod village;

pub use ability::{
    execute_or_start_ability, execute_or_start_ability_with_collision,
    validate_ability_line_of_sight, AbilityDefinition, AbilityExecutedEvent, AbilityPlugin,
    AbilityTickSystem, CooldownTracker,
};
pub use ai_brain::{FsmBrainComponent, FsmBrainSystem, FsmState, MonsterAiPlugin};
pub use combat::{
    process_damage_events, CombatPlugin, DamageEvent, EntityDiedEvent, HealthComponent,
    HealthRegenSystem, StatsComponent,
};
pub use guild::{
    GuildManager, GuildMemberComponent, GuildPlugin, GuildRank, GuildState, GuildVault,
};
pub use inventory::{
    check_encumbrance, EquipSlot, EquipmentComponent, InventoryComponent, InventoryPlugin,
    ItemStack,
};
pub use loot::{LootContainerComponent, LootEntry, LootPlugin, LootTable};
pub use navigation::{
    LeashComponent, NavAgentComponent, NavSteeringSystem, NpcMovementPlugin, WaypointComponent,
    WaypointLoopMode,
};
pub use navmesh::{NavMeshPlugin, NavMeshPolygon, NavMeshWorld};
pub use party::{PartyManager, PartyMemberComponent, PartyPlugin, PartyState};
pub use perception::{
    HostileSpottedEvent, PerceptionComponent, PerceptionPlugin, PerceptionScanSystem,
};
pub use quest::{QuestLogComponent, QuestObjective, QuestPlugin, QuestRecord, QuestStatus};
pub use scripting::{
    register_host_api, validate_memory_bounds, HostContext, ScriptHookHandler, ScriptHotReloader,
    ScriptTickSystem, WasmRuntime, WasmScriptEngine, WasmScriptPlugin,
};
pub use spawner::{
    SpawnCommandBuffer, SpawnEntityCommand, SpawnTemplate, SpawnerComponent, SpawnerPlugin,
    SpawnerSystem,
};
pub use threat::{ThreatEntry, ThreatPlugin, ThreatSystem, ThreatTableComponent};
pub use trade::{TradeCompletedEvent, TradeManager, TradeOffer, TradePlugin, TradeSession};
pub use village::{
    bootstrap_village_world, build_state_response, build_trade_session_view, BarterRecipe,
    InspectRequest, NpcBarterRequest, PlayerStarterKitHook, PlayerTradeOfferRequest,
    PlayerTradeSessionRequest, PlayerTradeStartRequest, TradeSessionView, VillagePlugin,
    VillageTradePacketHandler, VillageTradeStateResponse, VillagerMerchantComponent,
};
