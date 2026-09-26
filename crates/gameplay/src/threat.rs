//! Standalone Threat & Aggro Management Gameplay Plugin.
//!
//! Maintains threat tables for combat entities, calculates target switching thresholds,
//! accumulates threat from damage and sensory spotting, and resets upon leashing or target death.

use serde::{Deserialize, Serialize};

use mmo_core::{EntityDespawnEvent, GameplayPlugin, Guid, PluginContext, TickContext, TickSystem};

use crate::combat::{DamageEvent, HealthComponent};
use crate::navigation::LeashComponent;
use crate::perception::HostileSpottedEvent;

/// Individual target entry in an entity's threat table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreatEntry {
    pub target: Guid,
    pub threat: f32,
}

/// Dynamic component managing threat scores and current aggro target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreatTableComponent {
    pub entries: Vec<ThreatEntry>,
    pub current_target: Option<Guid>,
    pub max_entries: usize,
    pub switch_threshold_pct: f32,
}

impl ThreatTableComponent {
    pub fn new(max_entries: usize, switch_threshold_pct: f32) -> Self {
        Self {
            entries: Vec::with_capacity(max_entries),
            current_target: None,
            max_entries,
            switch_threshold_pct,
        }
    }

    pub fn default_combat() -> Self {
        Self::new(16, 1.10) // 10% threat lead required to pull aggro from current target
    }

    /// Adds or updates threat for a given target. Updates `current_target` if thresholds are exceeded.
    pub fn add_threat(&mut self, target: Guid, amount: f32) {
        if amount <= 0.0 {
            return;
        }

        if let Some(entry) = self.entries.iter_mut().find(|e| e.target == target) {
            entry.threat += amount;
        } else if self.entries.len() < self.max_entries {
            self.entries.push(ThreatEntry { target, threat: amount });
        } else {
            // Table full: replace the entry with the lowest threat if new threat is higher
            if let Some((idx, lowest)) = self
                .entries
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.threat.partial_cmp(&b.1.threat).unwrap())
            {
                if amount > lowest.threat {
                    self.entries[idx] = ThreatEntry { target, threat: amount };
                } else {
                    return;
                }
            }
        }

        self.evaluate_target();
    }

    /// Gets the threat score for a specific target.
    pub fn get_threat(&self, target: Guid) -> f32 {
        self.entries
            .iter()
            .find(|e| e.target == target)
            .map(|e| e.threat)
            .unwrap_or(0.0)
    }

    /// Removes a target from the threat table and re-evaluates the primary target.
    pub fn remove_target(&mut self, target: Guid) {
        self.entries.retain(|e| e.target != target);
        if self.current_target == Some(target) {
            self.current_target = None;
            self.evaluate_target();
        }
    }

    /// Clears all threat entries and drops the active target.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_target = None;
    }

    /// Re-evaluates whether the primary target should switch based on threat thresholds.
    pub fn evaluate_target(&mut self) {
        if self.entries.is_empty() {
            self.current_target = None;
            return;
        }

        let highest_entry = self
            .entries
            .iter()
            .max_by(|a, b| a.threat.partial_cmp(&b.threat).unwrap())
            .unwrap();

        match self.current_target {
            None => {
                self.current_target = Some(highest_entry.target);
            }
            Some(curr) => {
                if curr == highest_entry.target {
                    return;
                }
                let curr_threat = self.get_threat(curr);
                // Switch if highest threat exceeds current threat by switch_threshold_pct (e.g. 110%)
                if highest_entry.threat >= curr_threat * self.switch_threshold_pct {
                    self.current_target = Some(highest_entry.target);
                }
            }
        }
    }
}

/// Batch system processing sensory events and damage into threat tables.
#[derive(Default)]
pub struct ThreatSystem;

impl TickSystem for ThreatSystem {
    fn name(&self) -> &'static str {
        "ThreatSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        // 1. Process HostileSpottedEvents -> generate initial base threat
        let spotted_events = ctx.events.drain::<HostileSpottedEvent>();
        for evt in spotted_events {
            if let Some(entity) = ctx.entities.get_mut_by_guid(evt.observer) {
                if let Some(threat_table) = entity.get_component_mut::<ThreatTableComponent>() {
                    // Initial proximity threat (closer = more threat, baseline 10.0)
                    let base_threat = (10.0 / (1.0 + evt.distance * 0.1)).max(1.0);
                    threat_table.add_threat(evt.spotted, base_threat);
                }
            }
        }

        // 2. Process DamageEvents -> victim adds threat to attacker
        let damage_events = ctx.events.drain::<DamageEvent>();
        for dmg in &damage_events {
            if let Some(victim) = ctx.entities.get_mut_by_guid(dmg.target) {
                if let Some(threat_table) = victim.get_component_mut::<ThreatTableComponent>() {
                    // Threat = raw damage dealt
                    threat_table.add_threat(dmg.source, dmg.raw_damage);
                }
            }
        }
        // Re-push damage events so CombatPlugin or other plugins can also process them
        for dmg in damage_events {
            ctx.events.push(dmg);
        }

        // 3. Inspect leash reset and target validity for all entities with ThreatTableComponent
        let despawns = ctx.events.drain::<EntityDespawnEvent>();
        let mut invalid_targets: std::collections::HashSet<Guid> =
            despawns.iter().map(|e| e.guid).collect();

        // Collect unique current targets to evaluate
        let mut targets_to_check = Vec::new();
        for (_key, entity) in ctx.entities.iter() {
            if let Some(tt) = entity.get_component::<ThreatTableComponent>() {
                if let Some(target) = tt.current_target {
                    targets_to_check.push(target);
                }
            }
        }

        // Check health of active targets only (O(K_targets) instead of O(N_world))
        for target in targets_to_check {
            match ctx.entities.get_by_guid(target) {
                Some(target_ent) => {
                    if let Some(health) = target_ent.get_component::<HealthComponent>() {
                        if health.is_dead {
                            invalid_targets.insert(target);
                        }
                    }
                }
                None => {
                    invalid_targets.insert(target);
                }
            }
        }

        for (_key, entity) in ctx.entities.iter_mut() {
            let is_leashed_out = entity
                .get_component::<LeashComponent>()
                .map(|l| l.is_leashed_out)
                .unwrap_or(false);

            if let Some(threat_table) = entity.get_component_mut::<ThreatTableComponent>() {
                if is_leashed_out {
                    // Leash exceeded: wipe threat and drop combat
                    threat_table.clear();
                } else if let Some(target_guid) = threat_table.current_target {
                    if invalid_targets.contains(&target_guid) {
                        threat_table.remove_target(target_guid);
                    }
                }
            }
        }

        // Re-push despawn events so other systems can process them
        for despawn in despawns {
            ctx.events.push(despawn);
        }
    }
}

/// Standalone plugin registering threat components and systems.
pub struct ThreatPlugin;

impl GameplayPlugin for ThreatPlugin {
    fn id(&self) -> &'static str {
        "threat"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<ThreatTableComponent>("threat_table");
        ctx.tick_systems.push(Box::new(ThreatSystem));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{BoundingBox3D, EntityStore, EntityType, EventQueue, Octree};

    #[test]
    fn test_threat_table_accumulation_and_target_switching() {
        let mut table = ThreatTableComponent::default_combat();
        let p1 = Guid::new(EntityType::Player, 1, 100);
        let p2 = Guid::new(EntityType::Player, 1, 101);

        table.add_threat(p1, 50.0);
        assert_eq!(table.current_target, Some(p1));
        assert_eq!(table.get_threat(p1), 50.0);

        // Player 2 adds 52 threat: higher than 50, but < 50 * 1.10 (55.0) -> does NOT switch
        table.add_threat(p2, 52.0);
        assert_eq!(table.current_target, Some(p1));

        // Player 2 adds 4 more threat (total 56.0 > 55.0) -> switches target to p2!
        table.add_threat(p2, 4.0);
        assert_eq!(table.current_target, Some(p2));

        // P2 dies / removed -> falls back to P1
        table.remove_target(p2);
        assert_eq!(table.current_target, Some(p1));

        // Clear drops everything
        table.clear();
        assert_eq!(table.current_target, None);
        assert!(table.entries.is_empty());
    }

    #[test]
    fn test_threat_system_events_and_leash_wipe() {
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();

        let monster_guid = Guid::new(EntityType::Npc, 1, 10);
        let monster_key = store.spawn(monster_guid, (0.0, 0.0, 0.0));
        let mut leash = LeashComponent::new((0.0, 0.0, 0.0), 10.0);
        leash.is_leashed_out = false;

        store.get_mut(monster_key).unwrap().add_component(ThreatTableComponent::default_combat());
        store.get_mut(monster_key).unwrap().add_component(leash);

        let player_guid = Guid::new(EntityType::Player, 1, 20);
        store.spawn(player_guid, (5.0, 0.0, 0.0));

        // Enqueue DamageEvent: player dealt 30 damage to monster
        events.push(DamageEvent {
            source: player_guid,
            target: monster_guid,
            raw_damage: 30.0,
        });

        let mut threat_system = ThreatSystem;

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 1,
            };
            threat_system.update(&mut ctx);
        }

        // Verify monster gained threat from player
        let monster_ent = store.get(monster_key).unwrap();
        let table = monster_ent.get_component::<ThreatTableComponent>().unwrap();
        assert_eq!(table.current_target, Some(player_guid));
        assert_eq!(table.get_threat(player_guid), 30.0);

        // Now simulate leash out
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
                tick: 2,
            };
            threat_system.update(&mut ctx);
        }

        // Threat wiped due to leash out!
        let monster_ent = store.get(monster_key).unwrap();
        let table = monster_ent.get_component::<ThreatTableComponent>().unwrap();
        assert_eq!(table.current_target, None);
        assert!(table.entries.is_empty());
    }
}
