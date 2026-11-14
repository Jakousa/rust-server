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
    ActionQueue, AttributeId, AttributeSet, DomainEvent, Entity, EntityStore, EventQueue,
    GameplayPlugin, Guid, InterruptReason, PluginContext, TickContext, TickSystem,
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
        let safe_max = if max_hp.is_finite() && max_hp > 0.0 {
            max_hp
        } else {
            100.0
        };
        let safe_regen = if regen_per_sec.is_finite() {
            regen_per_sec
        } else {
            0.0
        };
        Self {
            current_hp: safe_max,
            max_hp: safe_max,
            is_dead: false,
            regen_per_sec: safe_regen,
        }
    }

    /// Applies raw damage to health. Returns `true` if damage resulted in death.
    pub fn take_damage(&mut self, amount: f32) -> bool {
        if self.is_dead || !amount.is_finite() || amount <= 0.0 {
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
        if !self.is_dead && amount.is_finite() && amount > 0.0 {
            self.current_hp = (self.current_hp + amount).min(self.max_hp);
        }
    }

    /// Resurrects a dead entity with the specified initial HP.
    pub fn respawn(&mut self, initial_hp: f32) {
        self.is_dead = false;
        let safe_max = if self.max_hp.is_finite() && self.max_hp > 0.0 {
            self.max_hp
        } else {
            100.0
        };
        let hp = if initial_hp.is_finite() {
            initial_hp
        } else {
            safe_max
        };
        self.current_hp = hp.clamp(1.0, safe_max);
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

    /// Computes mitigated damage applying a capped 90% armor curve:
    /// `raw * (1 - clamp(armor / (armor + 100), 0.0, 0.9))`.
    pub fn mitigate_damage(&self, raw_damage: f32) -> f32 {
        if !raw_damage.is_finite() || raw_damage <= 0.0 {
            return 0.0;
        }
        let armor = self.armor as f32;
        let mitigation_factor = if armor > 0.0 {
            (armor / (armor + 100.0)).clamp(0.0, 0.9)
        } else {
            0.0
        };
        raw_damage * (1.0 - mitigation_factor)
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

/// Batch tick system that processes queued `DamageEvent`s, applies damage, and cleans up
/// processed events at the end of the tick so other systems in the same tick can inspect them.
#[derive(Default)]
pub struct DamageProcessingSystem {
    re_pushed_damage_events: Vec<DamageEvent>,
}

impl TickSystem for DamageProcessingSystem {
    fn name(&self) -> &'static str {
        "DamageProcessingSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        if !self.re_pushed_damage_events.is_empty() {
            strip_re_pushed_damage_events(ctx.events, &mut self.re_pushed_damage_events);
        }
        let (_, processed_damage) = process_damage_events_inner(ctx.events, ctx.entities, true);
        self.re_pushed_damage_events = processed_damage;
    }

    fn end_of_tick(&mut self, ctx: &mut TickContext) {
        if !self.re_pushed_damage_events.is_empty() {
            strip_re_pushed_damage_events(ctx.events, &mut self.re_pushed_damage_events);
        }
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
        if target_ent.is_incapacitated()
            || target_ent
                .get_component::<HealthComponent>()
                .is_some_and(|h| h.is_dead)
        {
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

            let armor = if armor.is_finite() && armor > 0.0 {
                armor
            } else {
                0.0
            };

            let mitigation_factor = if armor > 0.0 {
                armor / (armor + 100.0)
            } else {
                0.0
            };
            effective_damage = (damage as f32) * (1.0 - mitigation_factor.clamp(0.0, 0.9));
            if !effective_damage.is_finite() || effective_damage <= 0.0 {
                effective_damage = 0.0;
            }

            if let Some(health_comp) = target_ent.get_component_mut::<HealthComponent>() {
                if health_comp.take_damage(effective_damage) {
                    died = true;
                }
                current_health = health_comp.current_hp.round().max(0.0) as u32;
            } else if let Some(attrs) = target_ent.components.get_component_mut::<AttributeSet>() {
                let res = attrs
                    .modify_current("health", -effective_damage)
                    .or_else(|_| {
                        attrs.modify_current_by_id(AttributeId::PRIMARY_VITALITY, -effective_damage)
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
                if let Some(health_comp) = target_ent.get_component_mut::<HealthComponent>() {
                    health_comp.is_dead = true;
                    health_comp.current_hp = 0.0;
                }
                target_ent.state_flags |= state_flags::INCAPACITATED;
                event_queue.push(EntityIncapacitatedEvent {
                    entity: target_guid,
                    instigator: caster_guid,
                });
                event_queue.push(EntityDiedEvent {
                    entity: target_guid,
                    killer: caster_guid,
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
                        attrs.modify_current_by_id(AttributeId::PRIMARY_VITALITY, effective_healing)
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
        ctx.tick_systems
            .push(Box::new(DamageProcessingSystem::default()));
        ctx.tick_systems.push(Box::new(HealthRegenSystem));

        // 3. Register RPG combat resolver
        *ctx.combat_resolver = Some(Arc::new(RpgCombatResolver));
    }
}

fn strip_re_pushed_damage_events(event_queue: &mut EventQueue, re_pushed: &mut Vec<DamageEvent>) {
    let mut queued = event_queue.drain::<DamageEvent>();
    for prev in re_pushed.drain(..) {
        if let Some(pos) = queued.iter().position(|e| e == &prev) {
            queued.remove(pos);
        }
    }
    for remaining in queued {
        event_queue.push(remaining);
    }
}

/// Processes all queued `DamageEvent`s against entities, updating health and emitting
/// `EntityDiedEvent` and `EntityIncapacitatedEvent`.
pub fn process_damage_events(
    event_queue: &mut EventQueue,
    entities: &mut EntityStore,
) -> Vec<EntityDiedEvent> {
    process_damage_events_inner(event_queue, entities, false).0
}

fn process_damage_events_inner(
    event_queue: &mut EventQueue,
    entities: &mut EntityStore,
    repush_damage_events: bool,
) -> (Vec<EntityDiedEvent>, Vec<DamageEvent>) {
    let damage_events = event_queue.drain::<DamageEvent>();
    let mut died_events = Vec::new();
    let mut incapacitated_events = Vec::new();

    for dmg in &damage_events {
        let bonus_damage = entities
            .get_by_guid(dmg.source)
            .and_then(|attacker| attacker.get_property_f64("bonus_damage"))
            .unwrap_or(0.0) as f32;
        let raw_damage = dmg.raw_damage + bonus_damage;
        if !raw_damage.is_finite() || raw_damage <= 0.0 {
            continue;
        }

        let target_entity = match entities.get_mut_by_guid(dmg.target) {
            Some(e) => e,
            None => continue,
        };

        if target_entity.is_incapacitated()
            || target_entity
                .get_component::<HealthComponent>()
                .is_some_and(|h| h.is_dead)
        {
            continue;
        }

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
        if !effective_damage.is_finite() || effective_damage <= 0.0 {
            continue;
        }

        let has_health_comp = target_entity.get_component::<HealthComponent>().is_some();
        let mut died = false;
        if let Some(health) = target_entity.get_component_mut::<HealthComponent>() {
            died = health.take_damage(effective_damage);
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
            if let Ok(new_hp) = attrs.modify_current("health", -effective_damage) {
                if !has_health_comp && new_hp <= 0.0 {
                    died = true;
                }
            }
        }

        if died {
            target_entity.state_flags |= state_flags::INCAPACITATED;
            died_events.push(EntityDiedEvent {
                entity: dmg.target,
                killer: Some(dmg.source),
            });
            incapacitated_events.push(EntityIncapacitatedEvent {
                entity: dmg.target,
                instigator: Some(dmg.source),
            });
        }
    }

    for death in &died_events {
        event_queue.push(death.clone());
    }
    for incap in incapacitated_events {
        event_queue.push(incap);
    }

    if repush_damage_events {
        for dmg in &damage_events {
            event_queue.push(dmg.clone());
        }
    }

    (died_events, damage_events)
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

        let mut sys = DamageProcessingSystem::default();
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

    #[test]
    fn test_non_finite_damage_heal_respawn_hardening() {
        // 1. Initializer sanitizes NaN max_hp and regen
        let mut health = HealthComponent::new(f32::NAN, f32::NAN);
        assert_eq!(health.max_hp, 100.0);
        assert_eq!(health.current_hp, 100.0);
        assert_eq!(health.regen_per_sec, 0.0);
        assert!(!health.is_dead);

        // 2. Reject NaN, Infinity, -Infinity, and negative damage
        assert!(!health.take_damage(f32::NAN));
        assert_eq!(health.current_hp, 100.0);
        assert!(!health.is_dead);

        assert!(!health.take_damage(f32::INFINITY));
        assert_eq!(health.current_hp, 100.0);

        assert!(!health.take_damage(f32::NEG_INFINITY));
        assert_eq!(health.current_hp, 100.0);

        assert!(!health.take_damage(-25.0));
        assert_eq!(health.current_hp, 100.0);

        // 3. Reject NaN heal
        health.current_hp = 50.0;
        health.heal(f32::NAN);
        assert_eq!(health.current_hp, 50.0);
        health.heal(f32::INFINITY);
        assert_eq!(health.current_hp, 50.0);

        // 4. Respawn with NaN initial HP safely sets finite health
        health.is_dead = true;
        health.respawn(f32::NAN);
        assert!(!health.is_dead);
        assert_eq!(health.current_hp, 100.0);

        // 5. Normal lethal damage still kills entity after rejected NaN attempts
        assert!(health.take_damage(100.0));
        assert_eq!(health.current_hp, 0.0);
        assert!(health.is_dead);

        // 6. DamageEvent with NaN raw_damage is safely ignored by process_damage_events
        let mut store = EntityStore::new();
        let mut queue = EventQueue::new();
        let att = Guid::new(EntityType::Player, 1, 101);
        let def = Guid::new(EntityType::Player, 1, 102);

        store.spawn(att, (0.0, 0.0, 0.0));
        let def_key = store.spawn(def, (0.0, 0.0, 0.0));
        store
            .get_mut(def_key)
            .unwrap()
            .add_component(HealthComponent::new(100.0, 0.0));

        queue.push(DamageEvent {
            source: att,
            target: def,
            raw_damage: f32::NAN,
        });

        let died = process_damage_events(&mut queue, &mut store);
        assert!(died.is_empty());
        let def_comp = store
            .get(def_key)
            .unwrap()
            .get_component::<HealthComponent>()
            .unwrap();
        assert_eq!(def_comp.current_hp, 100.0);
        assert!(!def_comp.is_dead);
    }

    #[test]
    fn test_armor_mitigation_90_percent_cap_and_non_finite_guard() {
        let high_armor = StatsComponent::new(10, 100_000);
        // 100_000 / 100_100 = 0.999... clamped to 0.90 -> 10% damage passes through
        let mitigated = high_armor.mitigate_damage(100.0);
        assert!((mitigated - 10.0).abs() < 1e-4);

        // Non-finite and non-positive damage must return 0.0
        assert_eq!(high_armor.mitigate_damage(f32::NAN), 0.0);
        assert_eq!(high_armor.mitigate_damage(f32::INFINITY), 0.0);
        assert_eq!(high_armor.mitigate_damage(f32::NEG_INFINITY), 0.0);
        assert_eq!(high_armor.mitigate_damage(-10.0), 0.0);
        assert_eq!(high_armor.mitigate_damage(0.0), 0.0);

        // RpgCombatResolver must apply the exact same 90% armor cap
        let target_guid = Guid::new(EntityType::Npc, 1, 1);
        let mut target = Entity::new(target_guid, (0.0, 0.0, 0.0));
        target.add_component(HealthComponent::new(100.0, 0.0));
        target.add_component(StatsComponent::new(0, 100_000));
        let mut events = EventQueue::new();
        let outcome = RpgCombatResolver
            .resolve_combat_effect(&mut target, target_guid, None, 100, 0, &mut events)
            .unwrap();
        assert!((outcome.effective_damage - 10.0).abs() < 1e-4);
        assert_eq!(outcome.current_health, 90);
    }

    #[test]
    fn test_combat_resolver_and_damage_events_sync_death_and_incapacitated_state() {
        let attacker = Guid::new(EntityType::Player, 1, 10);

        // 1. Kill via RpgCombatResolver -> sets both is_dead & INCAPACITATED and emits both events
        let target_a_guid = Guid::new(EntityType::Npc, 1, 11);
        let mut target_a = Entity::new(target_a_guid, (0.0, 0.0, 0.0));
        target_a.add_component(HealthComponent::new(50.0, 0.0));
        let mut events_a = EventQueue::new();

        let outcome = RpgCombatResolver
            .resolve_combat_effect(
                &mut target_a,
                target_a_guid,
                Some(attacker),
                60,
                0,
                &mut events_a,
            )
            .unwrap();
        assert!(outcome.died);
        assert!(target_a.get_component::<HealthComponent>().unwrap().is_dead);
        assert!(target_a.is_incapacitated());

        let died_a = events_a.drain::<EntityDiedEvent>();
        let incap_a = events_a.drain::<EntityIncapacitatedEvent>();
        assert_eq!(died_a.len(), 1);
        assert_eq!(died_a[0].entity, target_a_guid);
        assert_eq!(died_a[0].killer, Some(attacker));
        assert_eq!(incap_a.len(), 1);
        assert_eq!(incap_a[0].entity, target_a_guid);
        assert_eq!(incap_a[0].instigator, Some(attacker));

        // Already dead target rejected
        assert_eq!(
            RpgCombatResolver.resolve_combat_effect(
                &mut target_a,
                target_a_guid,
                Some(attacker),
                10,
                0,
                &mut events_a,
            ),
            Err("Target entity is dead")
        );

        // 2. Kill via process_damage_events -> sets both is_dead & INCAPACITATED and emits both events
        let mut store = EntityStore::new();
        let target_b_guid = Guid::new(EntityType::Npc, 1, 12);
        let b_key = store.spawn(target_b_guid, (0.0, 0.0, 0.0));
        store
            .get_mut(b_key)
            .unwrap()
            .add_component(HealthComponent::new(50.0, 0.0));

        let mut events_b = EventQueue::new();
        events_b.push(DamageEvent {
            source: attacker,
            target: target_b_guid,
            raw_damage: 60.0,
        });
        let deaths = process_damage_events(&mut events_b, &mut store);
        assert_eq!(deaths.len(), 1);

        let ent_b = store.get(b_key).unwrap();
        assert!(ent_b.get_component::<HealthComponent>().unwrap().is_dead);
        assert!(ent_b.is_incapacitated());

        let died_b = events_b.drain::<EntityDiedEvent>();
        let incap_b = events_b.drain::<EntityIncapacitatedEvent>();
        assert_eq!(died_b.len(), 1);
        assert_eq!(died_b[0].entity, target_b_guid);
        assert_eq!(died_b[0].killer, Some(attacker));
        assert_eq!(incap_b.len(), 1);
        assert_eq!(incap_b[0].entity, target_b_guid);
        assert_eq!(incap_b[0].instigator, Some(attacker));
    }

    #[test]
    fn test_damage_processing_system_preserves_damage_event_in_tick_and_cleans_at_end_of_tick() {
        let mut store = EntityStore::new();
        let mut queue = EventQueue::new();
        let mut octree = mmo_core::Octree::new(mmo_core::BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));

        let attacker = Guid::new(EntityType::Player, 1, 701);
        let victim = Guid::new(EntityType::Npc, 1, 702);
        store.spawn(attacker, (0.0, 0.0, 0.0));
        let v_key = store.spawn(victim, (1.0, 0.0, 0.0));
        store
            .get_mut(v_key)
            .unwrap()
            .add_component(HealthComponent::new(100.0, 0.0));

        let tick1_dmg = DamageEvent {
            source: attacker,
            target: victim,
            raw_damage: 25.0,
        };
        queue.push(tick1_dmg.clone());

        let mut sys = DamageProcessingSystem::default();
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut queue,
                dt_sec: 0.05,
                tick: 1,
                collision_world: None,
            };
            sys.update(&mut ctx);
        }

        // Damage was applied, AND DamageEvent remains queued during the tick for ThreatSystem
        assert_eq!(
            store
                .get(v_key)
                .unwrap()
                .get_component::<HealthComponent>()
                .unwrap()
                .current_hp,
            75.0
        );

        // Simulate a downstream AI system emitting a NEW DamageEvent later in the same tick
        let next_tick_dmg = DamageEvent {
            source: victim,
            target: attacker,
            raw_damage: 10.0,
        };
        queue.push(next_tick_dmg.clone());

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut queue,
                dt_sec: 0.05,
                tick: 1,
                collision_world: None,
            };
            sys.end_of_tick(&mut ctx);
        }

        // Processed tick1_dmg was cleaned up; next_tick_dmg emitted later in the tick remains
        let remaining = queue.drain::<DamageEvent>();
        assert_eq!(remaining, vec![next_tick_dmg]);
    }
}
