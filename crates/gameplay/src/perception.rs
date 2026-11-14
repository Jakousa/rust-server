//! Standalone Sensory Perception & Spatial Scanning Gameplay Plugin.
//!
//! Provides sight perception scanning using spatial octree queries,
//! faction hostility filtering, and deferred `HostileSpottedEvent` emissions.

use serde::{Deserialize, Serialize};

use mmo_core::{
    DomainEvent, EntityKey, EntityType, GameplayPlugin, Guid, PluginContext, TickContext,
    TickSystem,
};

use crate::combat::HealthComponent;

/// Dynamic component representing an entity's sensory perception and faction affiliation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerceptionComponent {
    pub sight_radius: f32,
    pub faction: u32,
    pub hostile_factions: Vec<u32>,
    pub scan_interval_secs: f32,
    pub scan_timer_secs: f32,
    pub spotted_targets: Vec<Guid>,
}

impl PerceptionComponent {
    pub fn new(sight_radius: f32, faction: u32, hostile_factions: Vec<u32>) -> Self {
        Self {
            sight_radius,
            faction,
            hostile_factions,
            scan_interval_secs: 0.2, // Scan 5 times per second (every 4 ticks)
            scan_timer_secs: 0.0,
            spotted_targets: Vec::new(),
        }
    }

    pub fn with_scan_interval(mut self, interval_secs: f32) -> Self {
        self.scan_interval_secs = interval_secs;
        self
    }
}

/// Domain event emitted when an observing entity detects a hostile entity within sensory range.
#[derive(Debug, Clone, PartialEq)]
pub struct HostileSpottedEvent {
    pub observer: Guid,
    pub spotted: Guid,
    pub distance: f32,
}

impl DomainEvent for HostileSpottedEvent {}

#[derive(Clone, Copy)]
struct PendingScan {
    key: EntityKey,
    guid: Guid,
    pos: (f32, f32, f32),
    sight_radius: f32,
}

/// Batch tick system querying the spatial octree for nearby entities and checking faction hostility.
#[derive(Default)]
pub struct PerceptionScanSystem {
    scratch_scans: Vec<PendingScan>,
    scratch_candidates: Vec<Guid>,
    scratch_events: Vec<(EntityKey, HostileSpottedEvent)>,
    scratch_valid_targets: Vec<Guid>,
}

impl PerceptionScanSystem {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TickSystem for PerceptionScanSystem {
    fn name(&self) -> &'static str {
        "PerceptionScanSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        self.scratch_scans.clear();
        self.scratch_events.clear();

        // Pass 1: Collect observers ready to scan without cloning hostile_factions
        for (key, entity) in ctx.entities.iter_mut() {
            let entity_guid = entity.guid;
            let entity_pos = entity.position;

            if let Some(perception) = entity.get_component_mut::<PerceptionComponent>() {
                perception.scan_timer_secs -= ctx.dt_sec;
                if perception.scan_timer_secs <= 0.0 {
                    perception.scan_timer_secs = perception.scan_interval_secs;
                    self.scratch_scans.push(PendingScan {
                        key,
                        guid: entity_guid,
                        pos: entity_pos,
                        sight_radius: perception.sight_radius,
                    });
                }
            }
        }

        // Pass 2: Perform spatial queries and faction filtering using reusable scratch buffers
        for &PendingScan {
            key: obs_key,
            guid: observer_guid,
            pos: observer_pos,
            sight_radius,
        } in &self.scratch_scans
        {
            let Some(observer_ent) = ctx.entities.get(obs_key) else {
                continue;
            };
            let Some(obs_perception) = observer_ent.get_component::<PerceptionComponent>() else {
                continue;
            };
            let hostile_factions = &obs_perception.hostile_factions;

            self.scratch_candidates.clear();
            ctx.octree
                .query_radius_into(observer_pos, sight_radius, &mut self.scratch_candidates);

            for &candidate_guid in &self.scratch_candidates {
                if candidate_guid == observer_guid {
                    continue;
                }

                // Check candidate faction and living state
                if let Some(candidate) = ctx.entities.get_by_guid(candidate_guid) {
                    let is_dead = candidate
                        .get_component::<HealthComponent>()
                        .map(|h| h.is_dead)
                        .unwrap_or(false);

                    if is_dead {
                        continue;
                    }

                    // Check faction:
                    // If candidate has PerceptionComponent, match against its faction.
                    // Otherwise, if candidate is a Player and player faction is hostile, flag it.
                    let candidate_faction = candidate
                        .get_component::<PerceptionComponent>()
                        .map(|p| p.faction)
                        .unwrap_or_else(|| {
                            if candidate_guid.entity_type() == EntityType::Player {
                                1 // Default player faction ID
                            } else {
                                0
                            }
                        });

                    if hostile_factions.contains(&candidate_faction) {
                        let dx = candidate.position.0 - observer_pos.0;
                        let dy = candidate.position.1 - observer_pos.1;
                        let dz = candidate.position.2 - observer_pos.2;
                        let distance = (dx * dx + dy * dy + dz * dz).sqrt();

                        self.scratch_events.push((
                            obs_key,
                            HostileSpottedEvent {
                                observer: observer_guid,
                                spotted: candidate_guid,
                                distance,
                            },
                        ));
                    }
                }
            }
        }

        // Pass 3: Prune stale targets in-place reusing existing spotted_targets allocations
        for &PendingScan {
            key: obs_key,
            pos: observer_pos,
            sight_radius,
            ..
        } in &self.scratch_scans
        {
            self.scratch_valid_targets.clear();
            let sight_radius_sq = sight_radius * sight_radius;
            if let Some(observer_ent) = ctx.entities.get(obs_key) {
                if let Some(perception) = observer_ent.get_component::<PerceptionComponent>() {
                    for &target_guid in &perception.spotted_targets {
                        if let Some(target_ent) = ctx.entities.get_by_guid(target_guid) {
                            let is_dead = target_ent
                                .get_component::<HealthComponent>()
                                .map(|h| h.is_dead)
                                .unwrap_or(false);
                            let dx = target_ent.position.0 - observer_pos.0;
                            let dy = target_ent.position.1 - observer_pos.1;
                            let dz = target_ent.position.2 - observer_pos.2;
                            let dist_sq = dx * dx + dy * dy + dz * dz;
                            if !is_dead && dist_sq <= sight_radius_sq {
                                self.scratch_valid_targets.push(target_guid);
                            }
                        }
                    }
                }
            }

            if let Some(observer_ent) = ctx.entities.get_mut(obs_key) {
                if let Some(perception) = observer_ent.get_component_mut::<PerceptionComponent>() {
                    perception.spotted_targets.clear();
                    perception
                        .spotted_targets
                        .extend_from_slice(&self.scratch_valid_targets);
                }
            }
        }

        // Pass 4: Emit events and update spotted_targets
        for (obs_key, evt) in self.scratch_events.drain(..) {
            if let Some(observer_ent) = ctx.entities.get_mut(obs_key) {
                if let Some(perception) = observer_ent.get_component_mut::<PerceptionComponent>() {
                    if !perception.spotted_targets.contains(&evt.spotted) {
                        perception.spotted_targets.push(evt.spotted);
                    }
                }
            }
            ctx.events.push(evt);
        }
    }
}

/// Standalone plugin registering perception components and scan systems.
pub struct PerceptionPlugin;

impl GameplayPlugin for PerceptionPlugin {
    fn id(&self) -> &'static str {
        "perception"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<PerceptionComponent>("perception");
        ctx.tick_systems.push(Box::new(PerceptionScanSystem::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{BoundingBox3D, EntityStore, EventQueue, Octree};

    #[test]
    fn test_perception_scanning_and_hostile_event_emission() {
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();

        // Monster observer at (0, 0, 0), sight radius 15.0m, hostile to faction 1 (players)
        let monster_guid = Guid::new(EntityType::Npc, 1, 10);
        let monster_key = store.spawn(monster_guid, (0.0, 0.0, 0.0));
        octree.insert(monster_guid, (0.0, 0.0, 0.0));

        let perception = PerceptionComponent::new(15.0, 2, vec![1]); // Monster faction 2, hostile to 1
        store
            .get_mut(monster_key)
            .unwrap()
            .add_component(perception);
        store
            .get_mut(monster_key)
            .unwrap()
            .add_component(HealthComponent::new(100.0, 0.0));

        // Friendly NPC at (5, 0, 0), faction 2 (same as monster) -> should not trigger aggro
        let ally_guid = Guid::new(EntityType::Npc, 1, 11);
        let ally_key = store.spawn(ally_guid, (5.0, 0.0, 0.0));
        octree.insert(ally_guid, (5.0, 0.0, 0.0));
        store
            .get_mut(ally_key)
            .unwrap()
            .add_component(PerceptionComponent::new(10.0, 2, vec![1]));

        // Hostile Player at (8, 0, 0), faction 1 (within 15m radius)
        let player_guid = Guid::new(EntityType::Player, 1, 20);
        let player_key = store.spawn(player_guid, (8.0, 0.0, 0.0));
        octree.insert(player_guid, (8.0, 0.0, 0.0));
        store
            .get_mut(player_key)
            .unwrap()
            .add_component(HealthComponent::new(100.0, 0.0));
        store
            .get_mut(player_key)
            .unwrap()
            .add_component(PerceptionComponent::new(20.0, 1, vec![2]));

        // Distant Player at (30, 0, 0) -> outside 15m radius
        let distant_guid = Guid::new(EntityType::Player, 1, 30);
        store.spawn(distant_guid, (30.0, 0.0, 0.0));
        octree.insert(distant_guid, (30.0, 0.0, 0.0));

        let mut scan_system = PerceptionScanSystem::new();

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 1,
                collision_world: None,
            };
            scan_system.update(&mut ctx);
        }

        // Verify HostileSpottedEvent was emitted for player at (8, 0, 0)
        let spotted_events = events.drain::<HostileSpottedEvent>();
        assert!(!spotted_events.is_empty());

        let monster_spotted: Vec<&HostileSpottedEvent> = spotted_events
            .iter()
            .filter(|e| e.observer == monster_guid)
            .collect();
        assert_eq!(monster_spotted.len(), 1);
        assert_eq!(monster_spotted[0].spotted, player_guid);
        assert!((monster_spotted[0].distance - 8.0).abs() < 1e-4);

        // Verify monster recorded spotted player in component
        let monster_ent = store.get(monster_key).unwrap();
        assert_eq!(
            monster_ent
                .get_component::<PerceptionComponent>()
                .unwrap()
                .spotted_targets,
            vec![player_guid]
        );

        // Kill the player and tick again -> monster should prune dead player from spotted_targets
        store
            .get_mut(player_key)
            .unwrap()
            .get_component_mut::<HealthComponent>()
            .unwrap()
            .is_dead = true;
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 1.0,
                tick: 2,
                collision_world: None,
            };
            scan_system.update(&mut ctx);
        }
        let monster_ent = store.get(monster_key).unwrap();
        assert!(monster_ent
            .get_component::<PerceptionComponent>()
            .unwrap()
            .spotted_targets
            .is_empty());
    }
}
