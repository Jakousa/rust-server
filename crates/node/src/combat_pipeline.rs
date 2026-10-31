//! Unified combat effect and damage mitigation pipeline for local and remote abilities.

pub use mmo_core::combat::{CombatEffectOutcome, CombatResolver};
use mmo_core::entity::state_flags;
use mmo_core::plugin::EntityIncapacitatedEvent;
use mmo_core::{ActionQueue, AttributeId, AttributeSet, Entity, EventQueue, Guid, InterruptReason};

/// Default unopinionated combat resolver applying direct primary vitality changes without genre curves.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultCombatResolver;

impl CombatResolver for DefaultCombatResolver {
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
        let effective_damage = damage as f32;
        let mut effective_healing = 0.0f32;
        let mut died = false;

        if damage > 0 {
            if let Some(attrs) = target_ent.components.get_component_mut::<AttributeSet>() {
                let res = attrs
                    .modify_current("vitality", -effective_damage)
                    .or_else(|_| attrs.modify_current("health", -effective_damage));

                if let Ok(new_hp) = res {
                    current_health = new_hp.round().max(0.0) as u32;
                    if new_hp <= 0.0 || current_health == 0 {
                        died = true;
                        target_ent.state_flags |= state_flags::INCAPACITATED;
                    }
                }
            } else if let Some(v) = target_ent
                .properties
                .get("vitality")
                .or_else(|| target_ent.properties.get("health"))
                .and_then(|v| v.as_i64())
            {
                let new_hp = (v as f32 - effective_damage).max(0.0);
                current_health = new_hp.round() as u32;
                if target_ent.properties.get("vitality").is_some() {
                    target_ent.properties.set("vitality", new_hp as i64);
                } else {
                    target_ent.properties.set("health", new_hp as i64);
                }
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
                    .modify_current("vitality", effective_healing)
                    .or_else(|_| attrs.modify_current("health", effective_healing));

                if let Ok(new_hp) = res {
                    current_health = new_hp.round() as u32;
                }
            } else if let Some(v) = target_ent
                .properties
                .get("vitality")
                .or_else(|| target_ent.properties.get("health"))
                .and_then(|v| v.as_i64())
            {
                let max_hp = target_ent
                    .properties
                    .get("max_vitality")
                    .or_else(|| target_ent.properties.get("max_health"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(100);
                let new_hp = (v + healing as i64).min(max_hp);
                current_health = new_hp as u32;
                if target_ent.properties.get("vitality").is_some() {
                    target_ent.properties.set("vitality", new_hp);
                } else {
                    target_ent.properties.set("health", new_hp);
                }
            }
        } else if let Some(attrs) = target_ent.components.get_component::<AttributeSet>() {
            current_health = attrs
                .get_current_by_id(AttributeId::PRIMARY_VITALITY)
                .or_else(|| attrs.get_current("vitality"))
                .or_else(|| attrs.get_current("health"))
                .unwrap_or(0.0)
                .round() as u32;
        } else if let Some(v) = target_ent
            .properties
            .get("vitality")
            .or_else(|| target_ent.properties.get("health"))
            .and_then(|v| v.as_i64())
        {
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

/// Applies combat effect using a specific [`CombatResolver`].
pub fn apply_combat_effect_with_resolver(
    resolver: &dyn CombatResolver,
    target_ent: &mut Entity,
    target_guid: Guid,
    caster_guid: Option<Guid>,
    damage: u32,
    healing: u32,
    event_queue: &mut EventQueue,
) -> Result<CombatEffectOutcome, &'static str> {
    resolver.resolve_combat_effect(
        target_ent,
        target_guid,
        caster_guid,
        damage,
        healing,
        event_queue,
    )
}

/// Authoritatively applies damage or healing to a target entity via [`DefaultCombatResolver`].
pub fn apply_combat_effect(
    target_ent: &mut Entity,
    target_guid: Guid,
    caster_guid: Option<Guid>,
    damage: u32,
    healing: u32,
    event_queue: &mut EventQueue,
) -> Result<CombatEffectOutcome, &'static str> {
    DefaultCombatResolver.resolve_combat_effect(
        target_ent,
        target_guid,
        caster_guid,
        damage,
        healing,
        event_queue,
    )
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

        // In DefaultCombatResolver, unmitigated direct damage = 30
        assert_eq!(outcome.effective_damage, 30.0);
        assert_eq!(outcome.current_health, 70);
        assert!(!outcome.died);

        // Action queue should have been interrupted
        let q = entity.get_component::<ActionQueue>().unwrap();
        assert!(q.current().is_none());

        // Attributes should be synced
        let a = entity.get_component::<AttributeSet>().unwrap();
        assert_eq!(a.get_current("health"), Some(70.0));
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
