//! Standalone Combat and Stats Gameplay Plugin.
//!
//! Provides health/mana pools, armor mitigation, batch tick health regeneration,
//! death state tracking, and damage event processing.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use mmo_core::combat::{CombatEffectOutcome, CombatResolver};
use mmo_core::entity::state_flags;
use mmo_core::plugin::EntityIncapacitatedEvent;
use mmo_core::{
    canonical_id_for_name, ActionQueue, AttributeId, AttributeSet, DomainEvent, Entity,
    EntityStore, EventQueue, GameplayPlugin, Guid, InterruptReason, PluginContext, TickContext,
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

/// Batch tick system that drains queued DamageEvents, applies damage, and emits EntityDiedEvents.
#[derive(Default)]
pub struct DamageProcessingSystem;

impl TickSystem for DamageProcessingSystem {
    fn name(&self) -> &'static str {
        "DamageProcessingSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        let _ = process_damage_events(ctx.events, ctx.entities);
    }
}

/// RPG combat resolver implementing armor mitigation curves and vitality calculations.
#[derive(Debug, Default, Clone, Copy)]
pub struct RpgCombatResolver;

impl CombatResolver for RpgCombatResolver {
    fn resolve_combat_effect(
        &self,
        target_ent: &mut Entity,
        target_guid: Guid,
        caster_guid: Option<Guid>,
        damage: u32,
        healing: u32,
        event_queue: &mut EventQueue,
    ) -> Result<CombatEffectOutcome, &'static str> {
        if target_ent.is_incapacitated() {
            return Err("Target entity is dead");
        }

        let mut current_health = 0u32;
        let mut effective_damage = 0.0f32;
        let mut effective_healing = 0.0f32;
        let mut died = false;

        if damage > 0 {
            // Read armor / mitigation from AttributeSet, StatsComponent, or properties
            let armor = if let Some(stats) = target_ent.get_component::<StatsComponent>() {
                stats.armor as f32
            } else if let Some(attrs) = target_ent.components.get_component::<AttributeSet>() {
                attrs
                    .get_current_by_id(AttributeId::MITIGATION)
                    .or_else(|| attrs.get_current("armor"))
                    .unwrap_or(0.0)
            } else {
                target_ent
                    .properties
                    .get("armor")
                    .and_then(|v| v.as_f64().map(|f| f as f32))
                    .unwrap_or(0.0)
            };

            let mitigation_factor = if armor > 0.0 {
                armor / (armor + 100.0)
            } else {
                0.0
            };
            effective_damage = (damage as f32) * (1.0 - mitigation_factor.clamp(0.0, 0.9));

            if let Some(health_comp) = target_ent.get_component_mut::<HealthComponent>() {
                if health_comp.take_damage(effective_damage) {
                    died = true;
                }
                current_health = health_comp.current_hp.round().max(0.0) as u32;
            } else if let Some(attrs) = target_ent.components.get_component_mut::<AttributeSet>() {
                let res = attrs
                    .modify_current("health", -effective_damage)
                    .or_else(|_| {
                        if let Some(name) = attrs
                            .attributes
                            .keys()
                            .find(|k| {
                                canonical_id_for_name(k) == Some(AttributeId::PRIMARY_VITALITY)
                            })
                            .cloned()
                        {
                            attrs.modify_current(&name, -effective_damage)
                        } else {
                            Err("no health attribute")
                        }
                    });

                if let Ok(new_hp) = res {
                    current_health = new_hp.round().max(0.0) as u32;
                    if new_hp <= 0.0 || current_health == 0 {
                        died = true;
                    }
                }
            } else if let Some(v) = target_ent.properties.get("health").and_then(|v| v.as_i64()) {
                let new_hp = (v as f32 - effective_damage).max(0.0);
                current_health = new_hp.round() as u32;
                target_ent.properties.set("health", new_hp as i64);
                if new_hp <= 0.0 || current_health == 0 {
                    died = true;
                }
            }

            if died {
                target_ent.state_flags |= state_flags::INCAPACITATED;
                event_queue.push(EntityIncapacitatedEvent {
                    entity: target_guid,
                    instigator: caster_guid,
                });
            }

            // Interrupt active action if configured with cancel_on_damage
            if let Some(queue) = target_ent.components.get_component_mut::<ActionQueue>() {
                let should_interrupt = queue
                    .current()
                    .map(|a| a.interrupt_flags.cancel_on_damage)
                    .unwrap_or(false);
                if should_interrupt {
                    queue.interrupt(target_guid, InterruptReason::Damage, event_queue);
                }
            }
        } else if healing > 0 {
            effective_healing = healing as f32;
            if let Some(health_comp) = target_ent.get_component_mut::<HealthComponent>() {
                health_comp.heal(effective_healing);
                current_health = health_comp.current_hp.round().max(0.0) as u32;
            } else if let Some(attrs) = target_ent.components.get_component_mut::<AttributeSet>() {
                let res = attrs
                    .modify_current("health", effective_healing)
                    .or_else(|_| {
                        if let Some(name) = attrs
                            .attributes
                            .keys()
                            .find(|k| {
                                canonical_id_for_name(k) == Some(AttributeId::PRIMARY_VITALITY)
                            })
                            .cloned()
                        {
                            attrs.modify_current(&name, effective_healing)
                        } else {
                            Err("no health attribute")
                        }
                    });

                if let Ok(new_hp) = res {
                    current_health = new_hp.round() as u32;
                }
            } else if let Some(v) = target_ent.properties.get("health").and_then(|v| v.as_i64()) {
                let max_hp = target_ent
                    .properties
                    .get("max_health")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(100);
                let new_hp = (v + healing as i64).min(max_hp);
                current_health = new_hp as u32;
                target_ent.properties.set("health", new_hp);
            }
        } else if let Some(health_comp) = target_ent.get_component::<HealthComponent>() {
            current_health = health_comp.current_hp.round().max(0.0) as u32;
        } else if let Some(attrs) = target_ent.components.get_component::<AttributeSet>() {
            current_health = attrs
                .get_current_by_id(AttributeId::PRIMARY_VITALITY)
                .or_else(|| attrs.get_current("health"))
                .unwrap_or(0.0)
                .round() as u32;
        } else if let Some(v) = target_ent.properties.get("health").and_then(|v| v.as_i64()) {
            current_health = v as u32;
        }

        Ok(CombatEffectOutcome {
            current_health,
            effective_damage,
            effective_healing,
            died,
        })
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

        // 2. Register batch tick systems
        ctx.tick_systems.push(Box::new(DamageProcessingSystem));
        ctx.tick_systems.push(Box::new(HealthRegenSystem));

        // 3. Register RPG combat resolver
        *ctx.combat_resolver = Some(Arc::new(RpgCombatResolver));
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
        let bonus_damage = entities
            .get_by_guid(dmg.source)
            .and_then(|attacker| attacker.get_property_f64("bonus_damage"))
            .unwrap_or(0.0) as f32;
        let raw_damage = dmg.raw_damage + bonus_damage;

        let target_entity = match entities.get_mut_by_guid(dmg.target) {
            Some(e) => e,
            None => continue,
        };

        let mut effective_damage =
            if let Some(stats) = target_entity.get_component::<StatsComponent>() {
                stats.mitigate_damage(raw_damage)
            } else {
                raw_damage
            };

        if let Some(mult) = target_entity.get_property_f64("damage_taken_multiplier") {
            effective_damage *= mult as f32;
        }
        if let Some(reduction) = target_entity.get_property_f64("damage_reduction") {
            effective_damage = (effective_damage - reduction as f32).max(0.0);
        }

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

        // 1. Interrupt active action if configured with cancel_on_damage
        if let Some(queue) = target_entity.get_component_mut::<mmo_core::ActionQueue>() {
            let should_interrupt = queue
                .current()
                .map(|a| a.interrupt_flags.cancel_on_damage)
                .unwrap_or(false);
            if should_interrupt {
                queue.interrupt(dmg.target, mmo_core::InterruptReason::Damage, event_queue);
            }
        }

        // 2. Synchronize with dynamic AttributeSet if attached
        if let Some(attrs) = target_entity.get_component_mut::<mmo_core::AttributeSet>() {
            let _ = attrs.modify_current("health", -effective_damage);
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
            collision_world: None,
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

    #[test]
    fn test_damage_interrupts_action_and_modifies_attribute_set() {
        let mut store = EntityStore::new();
        let mut queue = EventQueue::new();

        let attacker = Guid::new(EntityType::Player, 1, 10);
        let victim = Guid::new(EntityType::Player, 1, 20);

        let v_key = store.spawn(victim, (0.0, 0.0, 0.0));
        let ent = store.get_mut(v_key).unwrap();

        // 1. Attach AttributeSet with health
        let mut attrs = mmo_core::AttributeSet::new();
        attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);

        // 2. Attach ActionQueue with active action (cancel_on_damage = true)
        let mut action_queue = mmo_core::ActionQueue::new();
        let action = mmo_core::TimedAction::new(
            55,
            3.0,
            mmo_core::ActionInterruptFlags {
                cancel_on_movement: true,
                cancel_on_damage: true,
                cancel_on_stun: true,
                cancel_on_target_lost: true,
            },
            None,
        );
        action_queue
            .start_action(victim, action, mmo_core::QueuePolicy::Replace, &mut queue)
            .unwrap();
        ent.add_component(action_queue);

        // Drain start event
        let _ = queue.drain::<mmo_core::ActionStartedEvent>();

        // 3. Push damage event
        queue.push(DamageEvent {
            source: attacker,
            target: victim,
            raw_damage: 25.0,
        });

        process_damage_events(&mut queue, &mut store);

        // Verify action was aborted with Damage reason
        let ent = store.get_by_guid(victim).unwrap();
        let aq = ent.get_component::<mmo_core::ActionQueue>().unwrap();
        assert!(!aq.is_busy());

        let interrupted = queue.drain::<mmo_core::ActionInterruptedEvent>();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].action_id, 55);
        assert_eq!(interrupted[0].reason, mmo_core::InterruptReason::Damage);

        // Verify AttributeSet health was decremented by 25.0 -> 75.0
        let current_hp = ent
            .get_component::<mmo_core::AttributeSet>()
            .unwrap()
            .get_current("health")
            .unwrap();
        assert_eq!(current_hp, 75.0);
    }

    #[test]
    fn test_combat_with_dynamic_properties() {
        let mut store = EntityStore::new();
        let mut queue = EventQueue::new();

        let attacker = Guid::new(EntityType::Player, 1, 100);
        let defender = Guid::new(EntityType::Player, 1, 200);

        let att_key = store.spawn(attacker, (0.0, 0.0, 0.0));
        let def_key = store.spawn(defender, (0.0, 0.0, 0.0));

        // Attacker has dynamic bonus_damage = 20.0
        store
            .get_mut(att_key)
            .unwrap()
            .set_property("bonus_damage", 20.0);

        // Defender has health 100, damage_taken_multiplier = 0.5 (50% shield), damage_reduction = 5.0
        let def = store.get_mut(def_key).unwrap();
        def.add_component(HealthComponent::new(100.0, 0.0));
        def.set_property("damage_taken_multiplier", 0.5);
        def.set_property("damage_reduction", 5.0);

        // Raw damage = 40.0. Attacker adds +20.0 -> 60.0.
        // Mitigated by multiplier 0.5 -> 30.0.
        // Mitigated by flat reduction 5.0 -> 25.0.
        queue.push(DamageEvent {
            source: attacker,
            target: defender,
            raw_damage: 40.0,
        });

        process_damage_events(&mut queue, &mut store);

        let def_hp = store
            .get(def_key)
            .unwrap()
            .get_component::<HealthComponent>()
            .unwrap()
            .current_hp;
        assert_eq!(def_hp, 75.0); // 100.0 - 25.0 = 75.0
    }

    #[test]
    fn test_damage_processing_system_ticks_and_damages_entity() {
        let mut store = EntityStore::new();
        let mut queue = EventQueue::new();
        let mut octree = mmo_core::Octree::new(mmo_core::BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));

        let attacker = Guid::new(EntityType::Player, 1, 501);
        let defender = Guid::new(EntityType::Player, 1, 502);

        let _att_key = store.spawn(attacker, (0.0, 0.0, 0.0));
        let def_key = store.spawn(defender, (5.0, 0.0, 0.0));

        store
            .get_mut(def_key)
            .unwrap()
            .add_component(HealthComponent::new(100.0, 0.0));

        queue.push(DamageEvent {
            source: attacker,
            target: defender,
            raw_damage: 35.0,
        });

        let mut sys = DamageProcessingSystem;
        let mut ctx = TickContext {
            tick: 1,
            dt_sec: 0.1,
            entities: &mut store,
            events: &mut queue,
            octree: &mut octree,
            collision_world: None,
        };
        sys.update(&mut ctx);

        let hp = store
            .get(def_key)
            .unwrap()
            .get_component::<HealthComponent>()
            .unwrap()
            .current_hp;
        assert_eq!(hp, 65.0);
    }
}
