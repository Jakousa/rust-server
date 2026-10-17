//! Unified combat effect and damage mitigation pipeline for local and remote abilities.

use mmo_core::{ActionQueue, AttributeSet, Entity, EventQueue, Guid, InterruptReason};
use mmo_gameplay::combat::{EntityDiedEvent, StatsComponent};
use mmo_gameplay::HealthComponent;

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
/// 1. Dead target validation.
/// 2. Armor and resistance mitigation via [`StatsComponent::mitigate_damage`].
/// 3. Health adjustment on [`HealthComponent`].
/// 4. Entity death notification via [`EntityDiedEvent`].
/// 5. Damage-triggered action interruption via [`ActionQueue::interrupt`].
/// 6. Attribute synchronization on [`AttributeSet`].
pub fn apply_combat_effect(
    target_ent: &mut Entity,
    target_guid: Guid,
    caster_guid: Option<Guid>,
    damage: u32,
    healing: u32,
    event_queue: &mut EventQueue,
) -> Result<CombatEffectOutcome, &'static str> {
    if let Some(health) = target_ent.get_component::<HealthComponent>() {
        if health.is_dead {
            return Err("Target entity is dead");
        }
    }

    let mut current_health = 0u32;
    let mut effective_damage = 0.0f32;
    let mut effective_healing = 0.0f32;
    let mut died = false;

    if damage > 0 {
        effective_damage = if let Some(stats) = target_ent.get_component::<StatsComponent>() {
            stats.mitigate_damage(damage as f32)
        } else {
            damage as f32
        };

        if let Some(health) = target_ent.get_component_mut::<HealthComponent>() {
            died = health.take_damage(effective_damage);
            current_health = health.current_hp.round() as u32;
        }

        if died {
            event_queue.push(EntityDiedEvent {
                entity: target_guid,
                killer: caster_guid,
            });
        }

        // Interrupt active action if configured with cancel_on_damage
        if let Some(queue) = target_ent.get_component_mut::<ActionQueue>() {
            let should_interrupt = queue
                .current()
                .map(|a| a.interrupt_flags.cancel_on_damage)
                .unwrap_or(false);
            if should_interrupt {
                queue.interrupt(target_guid, InterruptReason::Damage, event_queue);
            }
        }

        if let Some(attrs) = target_ent.get_component_mut::<AttributeSet>() {
            let _ = attrs.modify_current("health", -effective_damage);
        }
    } else if healing > 0 {
        effective_healing = healing as f32;
        if let Some(health) = target_ent.get_component_mut::<HealthComponent>() {
            health.heal(effective_healing);
            current_health = health.current_hp.round() as u32;
        }
        if let Some(attrs) = target_ent.get_component_mut::<AttributeSet>() {
            let _ = attrs.modify_current("health", effective_healing);
        }
    } else if let Some(health) = target_ent.get_component::<HealthComponent>() {
        current_health = health.current_hp.round() as u32;
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
    use mmo_gameplay::combat::StatsComponent;
    use mmo_gameplay::HealthComponent;

    #[test]
    fn test_apply_combat_effect_damage_mitigation_and_interrupt() {
        let guid = Guid::new(EntityType::Player, 1, 10);
        let mut entity = Entity::new(guid, (0.0, 0.0, 0.0));
        entity.add_component(HealthComponent::new(100.0, 0.0));
        let stats = StatsComponent::new(10, 50); // armor 50 -> 33.3% mitigation
        entity.add_component(stats);

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

        let mut attrs = AttributeSet::new();
        attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
        entity.add_component(attrs);

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
        entity.add_component(HealthComponent::new(10.0, 0.0));

        let mut event_queue = EventQueue::new();
        let killer = Guid::new(EntityType::Player, 1, 30);
        let outcome = apply_combat_effect(&mut entity, guid, Some(killer), 50, 0, &mut event_queue)
            .expect("applied effect");

        assert!(outcome.died);
        assert_eq!(outcome.current_health, 0);

        // Verify EntityDiedEvent in event_queue
        let death_events = event_queue.drain::<EntityDiedEvent>();
        assert_eq!(death_events.len(), 1);
        assert_eq!(death_events[0].entity, guid);
        assert_eq!(death_events[0].killer, Some(killer));

        // Subsequent attack on dead entity returns error
        let err = apply_combat_effect(&mut entity, guid, Some(killer), 10, 0, &mut event_queue);
        assert!(err.is_err());
    }
}
