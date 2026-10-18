//! Unified combat effect and damage mitigation pipeline for local and remote abilities.

use mmo_core::entity::state_flags;
use mmo_core::plugin::EntityIncapacitatedEvent;
use mmo_core::{
    canonical_id_for_name, ActionQueue, AttributeId, AttributeSet, Entity, EventQueue, Guid,
    InterruptReason,
};

/// Outcome report of applying combat damage or healing to an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatEffectOutcome {
    /// Resulting health points of the target entity.
    pub current_health: u32,
    /// Damage applied after mitigation.
    pub effective_damage: f32,
    /// Healing applied.
    pub effective_healing: f32,
    /// Whether the target entity transitioned from alive to dead.
    pub died: bool,
}

/// Authoritatively applies damage or healing to a target entity, executing:
/// 1. Dead target validation via [`Entity::is_incapacitated`].
/// 2. Armor and resistance mitigation via [`AttributeSet`] or properties.
/// 3. Health adjustment on [`AttributeSet`] / properties.
/// 4. Entity death notification via [`EntityIncapacitatedEvent`].
/// 5. Damage-triggered action interruption via [`ActionQueue::interrupt`].
pub fn apply_combat_effect(
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
        // Read armor / mitigation from AttributeSet or properties
        let armor = target_ent
            .components
            .get_component::<AttributeSet>()
            .and_then(|a| {
                a.get_current_by_id(AttributeId::MITIGATION)
                    .or_else(|| a.get_current("armor"))
            })
            .or_else(|| {
                target_ent
                    .properties
                    .get("armor")
                    .and_then(|v| v.as_f64().map(|f| f as f32))
            })
            .unwrap_or(0.0);

        let mitigation_factor = if armor > 0.0 {
            armor / (armor + 100.0)
        } else {
            0.0
        };
        effective_damage = (damage as f32) * (1.0 - mitigation_factor.clamp(0.0, 0.9));

        if let Some(attrs) = target_ent.components.get_component_mut::<AttributeSet>() {
            let res = attrs
                .modify_current("health", -effective_damage)
                .or_else(|_| {
                    if let Some(name) = attrs
                        .attributes
                        .keys()
                        .find(|k| canonical_id_for_name(k) == Some(AttributeId::PRIMARY_VITALITY))
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
                    target_ent.state_flags |= state_flags::INCAPACITATED;
                }
            }
        } else if let Some(v) = target_ent.properties.get("health").and_then(|v| v.as_i64()) {
            let new_hp = (v as f32 - effective_damage).max(0.0);
            current_health = new_hp.round() as u32;
            target_ent.properties.set("health", new_hp as i64);
            if new_hp <= 0.0 || current_health == 0 {
                died = true;
                target_ent.state_flags |= state_flags::INCAPACITATED;
            }
        }

        if died {
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
        if let Some(attrs) = target_ent.components.get_component_mut::<AttributeSet>() {
            let res = attrs
                .modify_current("health", effective_healing)
                .or_else(|_| {
                    if let Some(name) = attrs
                        .attributes
                        .keys()
                        .find(|k| canonical_id_for_name(k) == Some(AttributeId::PRIMARY_VITALITY))
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

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{
        ActionInterruptFlags, ActionQueue, AttributeSet, EntityType, QueuePolicy, TimedAction,
    };

    #[test]
    fn test_apply_combat_effect_damage_mitigation_and_interrupt() {
        let guid = Guid::new(EntityType::Player, 1, 10);
        let mut entity = Entity::new(guid, (0.0, 0.0, 0.0));

        let mut attrs = AttributeSet::new();
        attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
        attrs.add_attribute("armor", 50.0, 0.0, 1000.0, 0.0); // 50 / (50 + 100) = 33.3% mitigation
        entity.add_component(attrs);

        let mut queue = ActionQueue::new();
        let action = TimedAction::new(
            1,
            2.0,
            ActionInterruptFlags {
                cancel_on_damage: true,
                ..Default::default()
            },
            None,
        );
        let mut dummy_events = EventQueue::new();
        queue
            .start_action(guid, action, QueuePolicy::Replace, &mut dummy_events)
            .unwrap();
        entity.add_component(queue);

        let mut event_queue = EventQueue::new();
        let outcome = apply_combat_effect(
            &mut entity,
            guid,
            Some(Guid::new(EntityType::Player, 1, 20)),
            30,
            0,
            &mut event_queue,
        )
        .expect("applied effect");

        // 30 damage with 33.3% mitigation = 20 damage
        assert!(outcome.effective_damage < 30.0);
        assert_eq!(outcome.current_health, 80);
        assert!(!outcome.died);

        // Action queue should have been interrupted
        let q = entity.get_component::<ActionQueue>().unwrap();
        assert!(q.current().is_none());

        // Attributes should be synced
        let a = entity.get_component::<AttributeSet>().unwrap();
        assert_eq!(a.get_current("health"), Some(80.0));
    }

    #[test]
    fn test_apply_combat_effect_death_event() {
        let guid = Guid::new(EntityType::Player, 1, 11);
        let mut entity = Entity::new(guid, (0.0, 0.0, 0.0));

        let mut attrs = AttributeSet::new();
        attrs.add_attribute("health", 10.0, 0.0, 10.0, 0.0);
        entity.add_component(attrs);

        let mut event_queue = EventQueue::new();
        let killer = Guid::new(EntityType::Player, 1, 30);
        let outcome = apply_combat_effect(&mut entity, guid, Some(killer), 50, 0, &mut event_queue)
            .expect("applied effect");

        assert!(outcome.died);
        assert_eq!(outcome.current_health, 0);
        assert!(entity.is_incapacitated());

        // Verify EntityIncapacitatedEvent in event_queue
        let death_events = event_queue.drain::<EntityIncapacitatedEvent>();
        assert_eq!(death_events.len(), 1);
        assert_eq!(death_events[0].entity, guid);
        assert_eq!(death_events[0].instigator, Some(killer));

        // Subsequent attack on dead entity returns error
        let err = apply_combat_effect(&mut entity, guid, Some(killer), 10, 0, &mut event_queue);
        assert!(err.is_err());
    }
}
