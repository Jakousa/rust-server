//! Standalone Modular Finite State Machine (FSM) AI Brain Plugin.
//!
//! Evaluates behavioral states (`Idle`, `Patrol`, `Chase`, `Attack`, `Evade`),
//! driving state transitions based on sensory threat detection, attack range,
//! cooldown timers, and leashing evasion.

use serde::{Deserialize, Serialize};

use mmo_core::{GameplayPlugin, Guid, PluginContext, TickContext, TickSystem};

use crate::combat::{DamageEvent, HealthComponent};
use crate::navigation::{LeashComponent, NavAgentComponent, WaypointComponent};
use crate::threat::ThreatTableComponent;

/// Concrete states for the AI behavior state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FsmState {
    /// Resting at current position for a given timer before resuming patrol or wander.
    Idle { timer_secs: f32 },
    /// Patrolling between designated waypoints.
    Patrol,
    /// Moving towards an active aggro target to enter combat range.
    Chase { target: Guid },
    /// In combat range of a target, performing periodic attacks based on cooldowns.
    Attack { target: Guid, cooldown_timer: f32 },
    /// Resetting back to spawn origin after leashing out or losing all targets.
    Evade { return_position: (f32, f32, f32) },
}

/// Dynamic component defining an entity's AI brain parameters and current state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsmBrainComponent {
    pub current_state: FsmState,
    pub attack_range: f32,
    pub attack_cooldown_secs: f32,
    pub idle_duration_secs: f32,
    pub auto_attack_damage: f32,
}

impl FsmBrainComponent {
    pub fn new(
        initial_state: FsmState,
        attack_range: f32,
        attack_cooldown_secs: f32,
        idle_duration_secs: f32,
        auto_attack_damage: f32,
    ) -> Self {
        Self {
            current_state: initial_state,
            attack_range,
            attack_cooldown_secs,
            idle_duration_secs,
            auto_attack_damage,
        }
    }

    pub fn default_monster() -> Self {
        Self::new(FsmState::Patrol, 2.5, 1.5, 2.0, 15.0)
    }
}

/// Batch tick system that evaluates FSM states and executes transitions.
#[derive(Default)]
pub struct FsmBrainSystem;

impl TickSystem for FsmBrainSystem {
    fn name(&self) -> &'static str {
        "FsmBrainSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        // Collect snapshots of target positions to query during Chase/Attack without borrowing conflicts
        let mut entity_positions: std::collections::HashMap<Guid, (f32, f32, f32)> =
            std::collections::HashMap::new();
        for (_, ent) in ctx.entities.iter() {
            entity_positions.insert(ent.guid, ent.position);
        }

        let mut damage_events_to_emit: Vec<DamageEvent> = Vec::new();

        for (_key, entity) in ctx.entities.iter_mut() {
            let entity_guid = entity.guid;
            let entity_pos = entity.position;

            // Check if entity is dead -> dead entities do not execute AI
            let is_dead = entity
                .get_component::<HealthComponent>()
                .map(|h| h.is_dead)
                .unwrap_or(false);
            if is_dead {
                continue;
            }

            // Read-only state extraction to prevent simultaneous mutable borrowing
            let is_leashed_out = entity
                .get_component::<LeashComponent>()
                .map(|l| l.is_leashed_out)
                .unwrap_or(false);
            let spawn_origin = entity
                .get_component::<LeashComponent>()
                .map(|l| l.spawn_origin)
                .unwrap_or(entity_pos);
            let has_waypoints = entity.get_component::<WaypointComponent>().is_some();
            let has_arrived = entity
                .get_component::<NavAgentComponent>()
                .map(|a| a.has_arrived)
                .unwrap_or(false);

            let current_threat_target = entity
                .get_component::<ThreatTableComponent>()
                .and_then(|t| t.current_target);

            let mut set_nav_target: Option<Option<(f32, f32, f32)>> = None;
            let mut clear_leash_out = false;
            let mut pause_waypoints: Option<bool> = None;

            if let Some(brain) = entity.get_component_mut::<FsmBrainComponent>() {
                // Global transition: If leashed out and not already evading -> transition to Evade
                if is_leashed_out && !matches!(brain.current_state, FsmState::Evade { .. }) {
                    brain.current_state = FsmState::Evade {
                        return_position: spawn_origin,
                    };
                    set_nav_target = Some(Some(spawn_origin));
                    pause_waypoints = Some(true);
                }

                // Execute state-specific logic
                match &mut brain.current_state {
                    FsmState::Evade { return_position } => {
                        pause_waypoints = Some(true);
                        let return_pos = *return_position;
                        let dx = return_pos.0 - entity_pos.0;
                        let dy = return_pos.1 - entity_pos.1;
                        let dz = return_pos.2 - entity_pos.2;
                        let dist_to_spawn = (dx * dx + dy * dy + dz * dz).sqrt();

                        if has_arrived || dist_to_spawn <= 1.0 {
                            // Arrived back home: clear leash, enter Idle
                            clear_leash_out = true;
                            brain.current_state = FsmState::Idle {
                                timer_secs: brain.idle_duration_secs,
                            };
                        } else {
                            set_nav_target = Some(Some(return_pos));
                        }
                    }

                    FsmState::Idle { timer_secs } => {
                        if let Some(target) = current_threat_target {
                            brain.current_state = FsmState::Chase { target };
                            pause_waypoints = Some(true);
                        } else {
                            *timer_secs -= ctx.dt_sec;
                            if *timer_secs <= 0.0 {
                                if has_waypoints {
                                    brain.current_state = FsmState::Patrol;
                                    pause_waypoints = Some(false);
                                } else {
                                    *timer_secs = brain.idle_duration_secs;
                                }
                            }
                        }
                    }

                    FsmState::Patrol => {
                        pause_waypoints = Some(false);
                        if let Some(target) = current_threat_target {
                            brain.current_state = FsmState::Chase { target };
                            pause_waypoints = Some(true);
                        }
                    }

                    FsmState::Chase { target } => {
                        pause_waypoints = Some(true);
                        let target_guid = *target;
                        if current_threat_target != Some(target_guid) {
                            brain.current_state = FsmState::Evade {
                                return_position: spawn_origin,
                            };
                            set_nav_target = Some(Some(spawn_origin));
                        } else if let Some(&target_pos) = entity_positions.get(&target_guid) {
                            let dx = target_pos.0 - entity_pos.0;
                            let dy = target_pos.1 - entity_pos.1;
                            let dz = target_pos.2 - entity_pos.2;
                            let dist = (dx * dx + dy * dy + dz * dz).sqrt();

                            if dist <= brain.attack_range {
                                brain.current_state = FsmState::Attack {
                                    target: target_guid,
                                    cooldown_timer: 0.0,
                                };
                                set_nav_target = Some(None); // Stop movement
                            } else {
                                set_nav_target = Some(Some(target_pos));
                            }
                        } else {
                            brain.current_state = FsmState::Evade {
                                return_position: spawn_origin,
                            };
                            set_nav_target = Some(Some(spawn_origin));
                        }
                    }

                    FsmState::Attack {
                        target,
                        cooldown_timer,
                    } => {
                        pause_waypoints = Some(true);
                        let target_guid = *target;
                        if current_threat_target != Some(target_guid) {
                            brain.current_state = FsmState::Evade {
                                return_position: spawn_origin,
                            };
                            set_nav_target = Some(Some(spawn_origin));
                        } else if let Some(&target_pos) = entity_positions.get(&target_guid) {
                            let dx = target_pos.0 - entity_pos.0;
                            let dy = target_pos.1 - entity_pos.1;
                            let dz = target_pos.2 - entity_pos.2;
                            let dist = (dx * dx + dy * dy + dz * dz).sqrt();

                            if dist > brain.attack_range * 1.5 {
                                brain.current_state = FsmState::Chase { target: target_guid };
                            } else {
                                set_nav_target = Some(None); // Remain stationary while in melee attack range
                                *cooldown_timer -= ctx.dt_sec;
                                if *cooldown_timer <= 0.0 {
                                    *cooldown_timer = brain.attack_cooldown_secs;
                                    damage_events_to_emit.push(DamageEvent {
                                        source: entity_guid,
                                        target: target_guid,
                                        raw_damage: brain.auto_attack_damage,
                                    });
                                }
                            }
                        } else {
                            brain.current_state = FsmState::Evade {
                                return_position: spawn_origin,
                            };
                            set_nav_target = Some(Some(spawn_origin));
                        }
                    }
                }
            }

            // Apply deferred component updates without conflicting borrows
            if clear_leash_out {
                if let Some(leash) = entity.get_component_mut::<LeashComponent>() {
                    leash.is_leashed_out = false;
                }
            }

            if let Some(pause) = pause_waypoints {
                if let Some(wp) = entity.get_component_mut::<WaypointComponent>() {
                    wp.is_paused = pause;
                }
            }

            if let Some(target_pos_opt) = set_nav_target {
                if let Some(agent) = entity.get_component_mut::<NavAgentComponent>() {
                    agent.target_position = target_pos_opt;
                    agent.has_arrived = false;
                }
            }
        }

        // Push combat damage events emitted by attack states
        for dmg in damage_events_to_emit {
            ctx.events.push(dmg);
        }
    }
}

/// Standalone plugin registering FSM AI brain components and systems.
pub struct MonsterAiPlugin;

impl GameplayPlugin for MonsterAiPlugin {
    fn id(&self) -> &'static str {
        "monster_ai"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<FsmBrainComponent>("fsm_brain");
        ctx.tick_systems.push(Box::new(FsmBrainSystem));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{BoundingBox3D, EntityStore, EntityType, EventQueue, Octree};

    #[test]
    fn test_fsm_brain_transitions_and_attacks() {
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();

        let monster_guid = Guid::new(EntityType::Npc, 1, 50);
        let monster_key = store.spawn(monster_guid, (0.0, 0.0, 0.0));
        let player_guid = Guid::new(EntityType::Player, 1, 60);
        store.spawn(player_guid, (5.0, 0.0, 0.0));

        let brain = FsmBrainComponent::new(
            FsmState::Idle { timer_secs: 1.0 },
            2.0, // attack range 2.0m
            1.0, // attack cooldown 1.0s
            1.0,
            25.0, // damage 25
        );
        let threat_table = ThreatTableComponent::default_combat();
        let leash = LeashComponent::new((0.0, 0.0, 0.0), 15.0);
        let nav = NavAgentComponent::new(10.0, 0.5);

        store.get_mut(monster_key).unwrap().add_component(brain);
        store
            .get_mut(monster_key)
            .unwrap()
            .add_component(threat_table.clone());
        store.get_mut(monster_key).unwrap().add_component(leash);
        store.get_mut(monster_key).unwrap().add_component(nav);
        store
            .get_mut(monster_key)
            .unwrap()
            .add_component(HealthComponent::new(100.0, 0.0));

        let mut brain_system = FsmBrainSystem;

        // 1. Initial tick with no threat -> stays in Idle
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 1,
            };
            brain_system.update(&mut ctx);
        }

        let monster_ent = store.get(monster_key).unwrap();
        assert!(matches!(
            monster_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
            FsmState::Idle { .. }
        ));

        // 2. Add threat from player -> FSM should transition from Idle to Chase
        store
            .get_mut(monster_key)
            .unwrap()
            .get_component_mut::<ThreatTableComponent>()
            .unwrap()
            .add_threat(player_guid, 50.0);

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 2,
            };
            brain_system.update(&mut ctx);
        }

        let monster_ent = store.get(monster_key).unwrap();
        assert_eq!(
            monster_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
            FsmState::Chase { target: player_guid }
        );

        // 3. Move monster to (4.0, 0.0, 0.0) so distance to player (5.0, 0.0, 0.0) is 1.0m (within 2.0m attack range)
        store.get_mut(monster_key).unwrap().position = (4.0, 0.0, 0.0);

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 3,
            };
            brain_system.update(&mut ctx);
        }

        // Should transition to Attack state
        let monster_ent = store.get(monster_key).unwrap();
        assert!(matches!(
            monster_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
            FsmState::Attack { target, .. } if target == player_guid
        ));

        // 4. Tick while in Attack state -> cooldown_timer is 0.0, so attack fires
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 4,
            };
            brain_system.update(&mut ctx);
        }

        let damage_events = events.drain::<DamageEvent>();
        assert_eq!(damage_events.len(), 1);
        assert_eq!(damage_events[0].source, monster_guid);
        assert_eq!(damage_events[0].target, player_guid);
        assert_eq!(damage_events[0].raw_damage, 25.0);

        // 5. Trigger leash out -> should transition to Evade
        store
            .get_mut(monster_key)
            .unwrap()
            .get_component_mut::<LeashComponent>()
            .unwrap()
            .is_leashed_out = true;

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 5,
            };
            brain_system.update(&mut ctx);
        }

        let monster_ent = store.get(monster_key).unwrap();
        assert!(matches!(
            monster_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
            FsmState::Evade { .. }
        ));
    }
}
