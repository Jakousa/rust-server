//! Pluggable Ability and Cooldown Gameplay Systems.
//!
//! Provides modular ability handling, cooldown tracking, resource validation,
//! and integration with core [`mmo_core::ActionQueue`] and [`mmo_core::AttributeSet`] primitives.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use mmo_core::collision::{CollisionWorld, SpatialQueryFilter};
use mmo_core::spatial::{
    gather_cone_candidates, gather_sphere_candidates, partition_candidates_by_node,
    SpatialTargetCandidate, Vec3,
};
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

    /// Resets the cooldown for a specific ability (e.g. on compensation refund).
    pub fn reset_cooldown(&mut self, ability_id: u32) {
        self.ability_cooldowns.remove(&ability_id);
    }

    /// Resets the global cooldown timer.
    pub fn reset_gcd(&mut self) {
        self.gcd_remaining_sec = 0.0;
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

/// Validates server-authoritative line of sight between caster and target.
///
/// Returns `Ok(())` if the trajectory is unobstructed.
/// Returns `Err` if coordinates are non-finite or if solid collision geometry blocks line of sight.
pub fn validate_ability_line_of_sight(
    caster_pos: Vec3,
    target_pos: Vec3,
    collision_world: &dyn CollisionWorld,
    caster_guid: Option<Guid>,
) -> Result<(), &'static str> {
    if !caster_pos.x.is_finite()
        || !caster_pos.y.is_finite()
        || !caster_pos.z.is_finite()
        || !target_pos.x.is_finite()
        || !target_pos.y.is_finite()
        || !target_pos.z.is_finite()
    {
        return Err("Non-finite coordinates provided for line of sight");
    }

    let dir = Vec3::new(
        target_pos.x - caster_pos.x,
        target_pos.y - caster_pos.y,
        target_pos.z - caster_pos.z,
    );
    let dist = dir.length();
    if dist < 1e-4 {
        return Ok(());
    }

    let norm_dir = match dir.normalized() {
        Some(d) => d,
        None => return Ok(()),
    };

    // Small clearance margin (0.1m) at target destination
    let max_check_dist = (dist - 0.1).max(0.0);
    if max_check_dist > 0.0 {
        let filter = SpatialQueryFilter::ignoring(caster_guid.unwrap_or(Guid(0)));
        if let Some(hit) = collision_world.raycast(caster_pos, norm_dir, max_check_dist, filter) {
            if hit.distance < max_check_dist {
                return Err("Line of sight blocked by obstacle");
            }
        }
    }

    Ok(())
}

/// Executes an ability immediately or initiates a [`TimedAction`] with authoritative line of sight validation.
pub fn execute_or_start_ability_with_collision(
    entity_guid: Guid,
    ability: &AbilityDefinition,
    target_guid: Option<Guid>,
    entities: &mut EntityStore,
    events: &mut EventQueue,
    collision_world: &dyn CollisionWorld,
) -> Result<bool, &'static str> {
    if let Some(target) = target_guid {
        let caster_pos = entities
            .get_by_guid(entity_guid)
            .map(|e| Vec3::new(e.position.0, e.position.1, e.position.2))
            .ok_or("Caster entity not found in store")?;
        let target_pos = entities
            .get_by_guid(target)
            .map(|e| Vec3::new(e.position.0, e.position.1, e.position.2))
            .ok_or("Target entity not found in store")?;

        validate_ability_line_of_sight(caster_pos, target_pos, collision_world, Some(entity_guid))?;
    }

    execute_or_start_ability(entity_guid, ability, target_guid, entities, events)
}

/// Result of resolving candidate targets for a boundary-crossing AoE ability.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryAoeTargeting {
    pub local_targets: Vec<SpatialTargetCandidate>,
    pub remote_batches: std::collections::BTreeMap<u32, Vec<SpatialTargetCandidate>>,
}

/// Gathers candidates within a radial sphere and partitions them by node.
pub fn resolve_boundary_sphere_aoe(
    candidates: &[SpatialTargetCandidate],
    center: Vec3,
    radius: f32,
    max_targets: usize,
    require_alive: bool,
    local_node_id: u32,
) -> BoundaryAoeTargeting {
    let gathered = gather_sphere_candidates(candidates, center, radius, max_targets, require_alive);
    let (local_targets, remote_batches) = partition_candidates_by_node(&gathered, local_node_id);
    BoundaryAoeTargeting {
        local_targets,
        remote_batches,
    }
}

/// Gathers candidates within a cone arc and partitions them by node.
#[allow(clippy::too_many_arguments)]
pub fn resolve_boundary_cone_aoe(
    candidates: &[SpatialTargetCandidate],
    origin: Vec3,
    facing: Vec3,
    max_range: f32,
    half_angle_rad: f32,
    max_targets: usize,
    require_alive: bool,
    local_node_id: u32,
) -> BoundaryAoeTargeting {
    let gathered = gather_cone_candidates(
        candidates,
        origin,
        facing,
        max_range,
        half_angle_rad,
        max_targets,
        require_alive,
    );
    let (local_targets, remote_batches) = partition_candidates_by_node(&gathered, local_node_id);
    BoundaryAoeTargeting {
        local_targets,
        remote_batches,
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

    #[test]
    fn test_cooldown_reset_and_boundary_aoe() {
        let mut tracker = CooldownTracker::new();
        tracker.start_cooldown(42, 10.0, 1.5);
        assert!(tracker.is_on_cooldown(42));

        tracker.reset_gcd();
        // Ability 42 still on cooldown
        assert!(tracker.is_on_cooldown(42));

        tracker.reset_cooldown(42);
        assert!(!tracker.is_on_cooldown(42));

        // Test boundary AoE candidate partitioning
        let c1 = SpatialTargetCandidate::new(
            Guid::new(EntityType::Player, 1, 1),
            1,
            Vec3::new(2.0, 0.0, 0.0),
            true,
            false,
        );
        let c2 = SpatialTargetCandidate::new(
            Guid::new(EntityType::Player, 2, 2),
            2,
            Vec3::new(4.0, 0.0, 0.0),
            true,
            false,
        );

        let targeting = resolve_boundary_sphere_aoe(
            &[c1.clone(), c2.clone()],
            Vec3::new(0.0, 0.0, 0.0),
            10.0,
            5,
            true,
            1,
        );

        assert_eq!(targeting.local_targets.len(), 1);
        assert_eq!(targeting.local_targets[0].guid, c1.guid);
        assert_eq!(targeting.remote_batches.len(), 1);
        assert_eq!(targeting.remote_batches.get(&2).unwrap().len(), 1);
        assert_eq!(targeting.remote_batches.get(&2).unwrap()[0].guid, c2.guid);
    }

    #[test]
    fn test_ability_line_of_sight_blocking_and_success() {
        use mmo_core::collision::AabbCollisionWorld;
        use mmo_core::octree::BoundingBox3D;
        use mmo_core::Entity;

        let mut collision = AabbCollisionWorld::new();
        let wall = BoundingBox3D::new((4.0, -2.0, -2.0), (6.0, 2.0, 2.0));
        let wall_id = Guid(99);
        collision.register_obstacle(wall_id, wall);

        let mut entities = EntityStore::new();
        let mut events = EventQueue::new();

        let caster_guid = Guid(1);
        let mut caster = Entity::new(caster_guid, (0.0, 0.0, 0.0));
        caster.add_component(ActionQueue::new());
        entities.insert_entity(caster);

        let target_guid = Guid(2);
        let target = Entity::new(target_guid, (10.0, 0.0, 0.0));
        entities.insert_entity(target);

        let ability = AbilityDefinition {
            id: 1,
            name: "Fireball".to_string(),
            cast_time_sec: 0.0,
            cooldown_sec: 1.0,
            gcd_sec: 1.0,
            range: 20.0,
            resource_cost: None,
            interrupt_flags: ActionInterruptFlags::default(),
        };

        // 1. Cast directly through the wall should fail due to blocked line of sight
        let result_blocked = execute_or_start_ability_with_collision(
            caster_guid,
            &ability,
            Some(target_guid),
            &mut entities,
            &mut events,
            &collision,
        );
        assert!(result_blocked.is_err());
        assert_eq!(
            result_blocked.unwrap_err(),
            "Line of sight blocked by obstacle"
        );

        // 2. Remove the obstacle wall: cast now succeeds
        collision.unregister_obstacle(wall_id);
        let result_clear = execute_or_start_ability_with_collision(
            caster_guid,
            &ability,
            Some(target_guid),
            &mut entities,
            &mut events,
            &collision,
        );
        assert!(result_clear.is_ok());
        assert!(result_clear.unwrap()); // Instant cast completed
    }

    #[test]
    fn test_ability_los_nan_coordinates_rejected() {
        use mmo_core::collision::EmptyCollisionWorld;
        let collision = EmptyCollisionWorld;
        let res = validate_ability_line_of_sight(
            Vec3::new(f32::NAN, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            &collision,
            None,
        );
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err(),
            "Non-finite coordinates provided for line of sight"
        );
    }
}
