//! Standalone Combat and Stats Gameplay Plugin.
//!
//! Provides health/mana pools, armor mitigation, batch tick health regeneration,
//! death state tracking, and damage event processing.

use serde::{Deserialize, Serialize};

use mmo_core::{
    DomainEvent, EntityStore, EventQueue, GameplayPlugin, Guid, PluginContext, TickContext,
    TickSystem,
};

/// Dynamic component representing an entity's health status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthComponent {
    pub current_hp: f32,
    pub max_hp: f32,
    pub is_dead: bool,
    pub regen_per_sec: f32,
}

impl HealthComponent {
    pub fn new(max_hp: f32, regen_per_sec: f32) -> Self {
        Self {
            current_hp: max_hp,
            max_hp,
            is_dead: false,
            regen_per_sec,
        }
    }

    /// Applies raw damage to health. Returns `true` if damage resulted in death.
    pub fn take_damage(&mut self, amount: f32) -> bool {
        if self.is_dead || amount <= 0.0 {
            return false;
        }

        self.current_hp = (self.current_hp - amount).max(0.0);
        if self.current_hp <= 0.0 {
            self.is_dead = true;
            true
        } else {
            false
        }
    }

    /// Restores health, up to max_hp. Does nothing if dead.
    pub fn heal(&mut self, amount: f32) {
        if !self.is_dead && amount > 0.0 {
            self.current_hp = (self.current_hp + amount).min(self.max_hp);
        }
    }

    /// Resurrects a dead entity with the specified initial HP.
    pub fn respawn(&mut self, initial_hp: f32) {
        self.is_dead = false;
        self.current_hp = initial_hp.clamp(1.0, self.max_hp);
    }
}

/// Dynamic component representing combat attributes and defense.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsComponent {
    pub attack_power: u32,
    pub armor: u32,
}

impl StatsComponent {
    pub fn new(attack_power: u32, armor: u32) -> Self {
        Self {
            attack_power,
            armor,
        }
    }

    /// Computes mitigated damage applying an armor curve: raw * 100 / (100 + armor).
    pub fn mitigate_damage(&self, raw_damage: f32) -> f32 {
        let reduction_factor = 100.0 / (100.0 + self.armor as f32);
        raw_damage * reduction_factor
    }
}

/// Domain event emitted when an entity attacks another.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageEvent {
    pub source: Guid,
    pub target: Guid,
    pub raw_damage: f32,
}

impl DomainEvent for DamageEvent {}

/// Domain event emitted when an entity dies.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDiedEvent {
    pub entity: Guid,
    pub killer: Option<Guid>,
}

impl DomainEvent for EntityDiedEvent {}

/// Batch tick system that regenerates health for living entities once per tick.
#[derive(Default)]
pub struct HealthRegenSystem;

impl TickSystem for HealthRegenSystem {
    fn name(&self) -> &'static str {
        "HealthRegenSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        for (_key, entity) in ctx.entities.iter_mut() {
            if let Some(health) = entity.get_component_mut::<HealthComponent>() {
                if !health.is_dead && health.current_hp < health.max_hp {
                    let regen = health.regen_per_sec * ctx.dt_sec;
                    health.heal(regen);
                }
            }
        }
    }
}

/// Standalone plugin for combat, health, and attribute systems.
pub struct CombatPlugin;

impl GameplayPlugin for CombatPlugin {
    fn id(&self) -> &'static str {
        "combat"
    }

    fn build(&self, ctx: &mut PluginContext) {
        // 1. Register persistence codecs
        ctx.codecs.register::<HealthComponent>("health");
        ctx.codecs.register::<StatsComponent>("stats");

        // 2. Register batch tick system
        ctx.tick_systems.push(Box::new(HealthRegenSystem));
    }
}

/// Processes all queued `DamageEvent`s against entities, updating health and emitting `EntityDiedEvent`.
pub fn process_damage_events(
    event_queue: &mut EventQueue,
    entities: &mut EntityStore,
) -> Vec<EntityDiedEvent> {
    let damage_events = event_queue.drain::<DamageEvent>();
    let mut died_events = Vec::new();

    for dmg in damage_events {
        let target_entity = match entities.get_mut_by_guid(dmg.target) {
            Some(e) => e,
            None => continue,
        };

        let effective_damage = if let Some(stats) = target_entity.get_component::<StatsComponent>()
        {
            stats.mitigate_damage(dmg.raw_damage)
        } else {
            dmg.raw_damage
        };

        if let Some(health) = target_entity.get_component_mut::<HealthComponent>() {
            let died = health.take_damage(effective_damage);
            if died {
                let death_event = EntityDiedEvent {
                    entity: dmg.target,
                    killer: Some(dmg.source),
                };
                died_events.push(death_event);
            }
        }
    }

    for death in &died_events {
        event_queue.push(death.clone());
    }

    died_events
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::EntityType;

    #[test]
    fn test_damage_and_mitigation() {
        let mut health = HealthComponent::new(100.0, 5.0);
        let stats = StatsComponent::new(20, 100); // 100 armor = 50% damage reduction

        let raw = 40.0;
        let mitigated = stats.mitigate_damage(raw);
        assert_eq!(mitigated, 20.0);

        let died = health.take_damage(mitigated);
        assert!(!died);
        assert_eq!(health.current_hp, 80.0);

        // Lethal damage
        let lethal_mitigated = stats.mitigate_damage(200.0); // 100 damage
        let died = health.take_damage(lethal_mitigated);
        assert!(died);
        assert!(health.is_dead);
        assert_eq!(health.current_hp, 0.0);
    }

    #[test]
    fn test_health_regen_system() {
        let mut store = EntityStore::new();
        let g1 = Guid::new(EntityType::Player, 1, 10);
        let key = store.spawn(g1, (0.0, 0.0, 0.0));

        let mut health = HealthComponent::new(100.0, 10.0); // 10 HP/sec
        health.current_hp = 50.0;
        store.get_mut(key).unwrap().add_component(health);

        let mut octree = mmo_core::Octree::new(mmo_core::BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();
        let mut ctx = TickContext {
            entities: &mut store,
            octree: &mut octree,
            events: &mut events,
            dt_sec: 0.5,
            tick: 1,
        };

        let mut regen_system = HealthRegenSystem;
        regen_system.update(&mut ctx); // 0.5s * 10 HP/sec = 5 HP

        let hp = store
            .get(key)
            .unwrap()
            .get_component::<HealthComponent>()
            .unwrap()
            .current_hp;
        assert_eq!(hp, 55.0);
    }

    #[test]
    fn test_damage_event_queue_processing() {
        let mut store = EntityStore::new();
        let mut queue = EventQueue::new();

        let attacker = Guid::new(EntityType::Player, 1, 10);
        let victim = Guid::new(EntityType::Player, 1, 20);

        let v_key = store.spawn(victim, (0.0, 0.0, 0.0));
        store
            .get_mut(v_key)
            .unwrap()
            .add_component(HealthComponent::new(50.0, 0.0));

        queue.push(DamageEvent {
            source: attacker,
            target: victim,
            raw_damage: 60.0,
        });

        let deaths = process_damage_events(&mut queue, &mut store);
        assert_eq!(deaths.len(), 1);
        assert_eq!(deaths[0].entity, victim);
        assert_eq!(deaths[0].killer, Some(attacker));

        let victim_hp = store
            .get(v_key)
            .unwrap()
            .get_component::<HealthComponent>()
            .unwrap();
        assert!(victim_hp.is_dead);
        assert_eq!(victim_hp.current_hp, 0.0);
    }
}
