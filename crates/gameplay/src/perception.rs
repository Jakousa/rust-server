//! Standalone Sensory Perception & Spatial Scanning Gameplay Plugin.
//!
//! Provides sight perception scanning using spatial octree queries,
//! faction hostility filtering, and deferred `HostileSpottedEvent` emissions.

use serde::{Deserialize, Serialize};

use mmo_core::{
    DomainEvent, EntityType, GameplayPlugin, Guid, PluginContext, TickContext, TickSystem,
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

/// Batch tick system querying the spatial octree for nearby entities and checking faction hostility.
#[derive(Default)]
pub struct PerceptionScanSystem;

impl TickSystem for PerceptionScanSystem {
    fn name(&self) -> &'static str {
        "PerceptionScanSystem"
    }

    #[allow(clippy::type_complexity)]
    fn update(&mut self, ctx: &mut TickContext) {
        // Collect observers ready to scan: (observer_guid, observer_pos, sight_radius, hostile_factions)
        let mut scans: Vec<(Guid, (f32, f32, f32), f32, Vec<u32>)> = Vec::new();

        for (_key, entity) in ctx.entities.iter_mut() {
            let entity_guid = entity.guid;
            let entity_pos = entity.position;

            if let Some(perception) = entity.get_component_mut::<PerceptionComponent>() {
                perception.scan_timer_secs -= ctx.dt_sec;
                if perception.scan_timer_secs <= 0.0 {
                    perception.scan_timer_secs = perception.scan_interval_secs;
                    scans.push((
                        entity_guid,
                        entity_pos,
                        perception.sight_radius,
                        perception.hostile_factions.clone(),
                    ));
                }
            }
        }

        // Perform spatial queries and faction filtering
        let mut events_to_emit: Vec<HostileSpottedEvent> = Vec::new();

        for (observer_guid, observer_pos, sight_radius, hostile_factions) in scans {
            let candidates = ctx.octree.query_radius(observer_pos, sight_radius);

            for candidate_guid in candidates {
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

                        events_to_emit.push(HostileSpottedEvent {
                            observer: observer_guid,
                            spotted: candidate_guid,
                            distance,
                        });
                    }
                }
            }
        }

        // Emit events and update spotted_targets
        for evt in events_to_emit {
            if let Some(observer_ent) = ctx.entities.get_mut_by_guid(evt.observer) {
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
        ctx.tick_systems.push(Box::new(PerceptionScanSystem));
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

        let mut scan_system = PerceptionScanSystem;

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 1,
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
    }
}
