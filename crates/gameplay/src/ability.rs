//! Pluggable Ability and Cooldown Gameplay Systems.
//!
//! Provides modular ability handling, cooldown tracking, resource validation,
//! and integration with core [`mmo_core::ActionQueue`] and [`mmo_core::AttributeSet`] primitives.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use mmo_core::{
    ActionInterruptFlags, ActionQueue, AttributeSet, DomainEvent, EntityStore, EventQueue,
    GameplayPlugin, Guid, PluginContext, QueuePolicy, TickContext, TickSystem, TimedAction,
};

/// Component tracking ability cooldowns and the shared Global Cooldown (GCD).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CooldownTracker {
    pub ability_cooldowns: HashMap<u32, f32>,
    pub gcd_remaining_sec: f32,
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if a specific ability or the global cooldown is currently active.
    pub fn is_on_cooldown(&self, ability_id: u32) -> bool {
        if self.gcd_remaining_sec > 1e-4 {
            return true;
        }
        self.ability_cooldowns
            .get(&ability_id)
            .copied()
            .unwrap_or(0.0)
            > 1e-4
    }

    /// Triggers cooldowns for the ability and sets the shared GCD timer.
    pub fn start_cooldown(&mut self, ability_id: u32, duration_sec: f32, gcd_sec: f32) {
        if duration_sec > 0.0 {
            self.ability_cooldowns.insert(ability_id, duration_sec);
        }
        if gcd_sec > 0.0 {
            self.gcd_remaining_sec = self.gcd_remaining_sec.max(gcd_sec);
        }
    }

    /// Advances cooldown timers by `dt_sec`, removing expired entries.
    pub fn tick(&mut self, dt_sec: f32) {
        if self.gcd_remaining_sec > 0.0 {
            self.gcd_remaining_sec = (self.gcd_remaining_sec - dt_sec).max(0.0);
        }

        self.ability_cooldowns.retain(|_, remaining| {
            *remaining -= dt_sec;
            *remaining > 0.0
        });
    }
}

/// Definition of a pluggable ability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbilityDefinition {
    pub id: u32,
    pub name: String,
    pub cast_time_sec: f32,
    pub cooldown_sec: f32,
    pub gcd_sec: f32,
    pub range: f32,
    pub resource_cost: Option<(String, f32)>,
    pub interrupt_flags: ActionInterruptFlags,
}

/// Event dispatched when an ability completes its execution or triggers instantly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbilityExecutedEvent {
    pub entity: Guid,
    pub ability_id: u32,
    pub target: Option<Guid>,
}
impl DomainEvent for AbilityExecutedEvent {}

/// Executes an ability immediately (if instant) or initiates a [`TimedAction`] in [`ActionQueue`].
pub fn execute_or_start_ability(
    entity_guid: Guid,
    ability: &AbilityDefinition,
    target_guid: Option<Guid>,
    entities: &mut EntityStore,
    events: &mut EventQueue,
) -> Result<bool, &'static str> {
    let entity = entities
        .get_mut_by_guid(entity_guid)
        .ok_or("Entity not found in store")?;

    // 1. Check cooldowns
    if let Some(cd) = entity.get_component::<CooldownTracker>() {
        if cd.is_on_cooldown(ability.id) {
            return Err("Ability is currently on cooldown");
        }
    }

    // 2. Check and deduct resource cost
    if let Some((ref resource_name, cost)) = ability.resource_cost {
        if let Some(attrs) = entity.get_component_mut::<AttributeSet>() {
            let current = attrs
                .get_current(resource_name)
                .ok_or("Required resource attribute not found on entity")?;
            if current < cost {
                return Err("Insufficient resource to activate ability");
            }
            attrs.modify_current(resource_name, -cost)?;
        }
    }

    // 3. Trigger cooldowns
    if let Some(cd) = entity.get_component_mut::<CooldownTracker>() {
        cd.start_cooldown(ability.id, ability.cooldown_sec, ability.gcd_sec);
    }

    // 4. Handle cast time vs instant execution
    if ability.cast_time_sec > 0.0 {
        let timed_action = TimedAction::new(
            ability.id,
            ability.cast_time_sec,
            ability.interrupt_flags,
            target_guid,
        );

        let queue = entity
            .get_component_mut::<ActionQueue>()
            .ok_or("Entity does not have an ActionQueue component")?;
        queue.start_action(entity_guid, timed_action, QueuePolicy::RejectIfBusy, events)?;
        Ok(false) // Cast in progress
    } else {
        // Instant execution
        events.push(AbilityExecutedEvent {
            entity: entity_guid,
            ability_id: ability.id,
            target: target_guid,
        });
        Ok(true) // Instant completion
    }
}

/// Batch tick system advancing entity cooldowns.
#[derive(Default)]
pub struct AbilityTickSystem;

impl TickSystem for AbilityTickSystem {
    fn name(&self) -> &'static str {
        "AbilityTickSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        for (_key, entity) in ctx.entities.iter_mut() {
            if let Some(cd) = entity.get_component_mut::<CooldownTracker>() {
                cd.tick(ctx.dt_sec);
            }
        }
    }
}

/// Gameplay plugin registering ability codecs and tick systems.
pub struct AbilityPlugin;

impl GameplayPlugin for AbilityPlugin {
    fn id(&self) -> &'static str {
        "ability"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<CooldownTracker>("cooldown_tracker");
        ctx.tick_systems.push(Box::new(AbilityTickSystem));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_cooldown_tracker_gcd_and_ability_cooldown() {
        let mut tracker = CooldownTracker::new();
        assert!(!tracker.is_on_cooldown(1));

        // Start 5.0s cooldown with 1.5s GCD
        tracker.start_cooldown(1, 5.0, 1.5);
        assert!(tracker.is_on_cooldown(1));
        assert!(tracker.is_on_cooldown(2)); // Locked by GCD

        // Advance 1.0s -> GCD remaining 0.5s, ability remaining 4.0s
        tracker.tick(1.0);
        assert!(tracker.is_on_cooldown(2));

        // Advance 0.6s -> GCD expired!
        tracker.tick(0.6);
        assert!(!tracker.is_on_cooldown(2)); // Ability 2 unlocked
        assert!(tracker.is_on_cooldown(1)); // Ability 1 still on cooldown

        // Advance 3.5s -> Ability 1 expired!
        tracker.tick(3.5);
        assert!(!tracker.is_on_cooldown(1));
    }

    #[test]
    fn test_instant_ability_execution_and_resource_deduction() {
        let mut entities = EntityStore::new();
        let mut events = EventQueue::new();
        let guid = Guid::new(EntityType::Player, 1, 601);

        let key = entities.spawn(guid, (0.0, 0.0, 0.0));
        let ent = entities.get_mut(key).unwrap();

        let mut attrs = AttributeSet::new();
        attrs.add_attribute("energy", 50.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        let fireball = AbilityDefinition {
            id: 10,
            name: "Fire Blast".into(),
            cast_time_sec: 0.0, // Instant
            cooldown_sec: 4.0,
            gcd_sec: 1.5,
            range: 20.0,
            resource_cost: Some(("energy".into(), 20.0)),
            interrupt_flags: ActionInterruptFlags::default(),
        };

        // Execute instant ability
        let res = execute_or_start_ability(guid, &fireball, None, &mut entities, &mut events);
        assert_eq!(res, Ok(true));

        // Energy should be reduced to 30.0
        let ent = entities.get_by_guid(guid).unwrap();
        let current_energy = ent
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("energy")
            .unwrap();
        assert_eq!(current_energy, 30.0);

        // Ability and GCD should now be on cooldown
        let cd = ent.get_component::<CooldownTracker>().unwrap();
        assert!(cd.is_on_cooldown(10));

        // AbilityExecutedEvent should be emitted
        let executed = events.drain::<AbilityExecutedEvent>();
        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].ability_id, 10);
    }

    #[test]
    fn test_casted_ability_enqueues_timed_action() {
        let mut entities = EntityStore::new();
        let mut events = EventQueue::new();
        let guid = Guid::new(EntityType::Player, 1, 602);

        let key = entities.spawn(guid, (0.0, 0.0, 0.0));
        let ent = entities.get_mut(key).unwrap();

        let mut attrs = AttributeSet::new();
        attrs.add_attribute("mana", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());
        ent.add_component(ActionQueue::new());

        let frostbolt = AbilityDefinition {
            id: 20,
            name: "Frostbolt".into(),
            cast_time_sec: 2.0, // 2 second cast
            cooldown_sec: 0.0,
            gcd_sec: 1.5,
            range: 30.0,
            resource_cost: Some(("mana".into(), 25.0)),
            interrupt_flags: ActionInterruptFlags {
                cancel_on_movement: true,
                cancel_on_damage: true,
                cancel_on_stun: true,
                cancel_on_target_lost: true,
            },
        };

        let res = execute_or_start_ability(guid, &frostbolt, None, &mut entities, &mut events);
        assert_eq!(res, Ok(false)); // Cast in progress

        // ActionQueue should now be busy with 2.0s duration
        let ent = entities.get_by_guid(guid).unwrap();
        let q = ent.get_component::<ActionQueue>().unwrap();
        assert!(q.is_busy());
        assert_eq!(q.current().unwrap().action_id, 20);
        assert_eq!(q.current().unwrap().duration_sec, 2.0);

        // ActionStartedEvent should be emitted
        let started = events.drain::<mmo_core::ActionStartedEvent>();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].action_id, 20);
    }
}
