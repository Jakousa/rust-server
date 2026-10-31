//! Authoritative Cancellable Timed Action Primitives.
//!
//! Provides a genre-agnostic state machine for entity tasks that require elapsed time
//! to complete (e.g., cast bars, weapon reload, lockpicking, warp-drive spooling),
//! with deterministic interruption guards and lifecycle event notifications.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::guid::Guid;
use crate::plugin::{DomainEvent, EventQueue, TickContext, TickSystem};
use crate::spatial::{
    gather_sphere_candidates, partition_candidates_by_node, SpatialTargetCandidate, Vec3,
};

/// Flags defining which environmental changes abort an active action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionInterruptFlags {
    pub cancel_on_movement: bool,
    pub cancel_on_damage: bool,
    pub cancel_on_stun: bool,
    pub cancel_on_target_lost: bool,
}

impl Default for ActionInterruptFlags {
    fn default() -> Self {
        Self {
            cancel_on_movement: true,
            cancel_on_damage: false,
            cancel_on_stun: true,
            cancel_on_target_lost: true,
        }
    }
}

/// Reason explaining why an active action was aborted before completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterruptReason {
    Movement,
    Damage,
    CrowdControl,
    TargetLost,
    Replaced,
    Manual,
}

/// Policy for handling a new action when an entity is already performing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePolicy {
    /// Abort the currently active action and start the new one immediately.
    Replace,
    /// Reject the new action if an action is currently in progress.
    RejectIfBusy,
}

/// Represents an active or pending timed action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedAction {
    pub action_id: u32,
    pub duration_sec: f32,
    pub elapsed_sec: f32,
    pub interrupt_flags: ActionInterruptFlags,
    pub target_guid: Option<Guid>,
    pub extra_data: Option<serde_json::Value>,
}

impl TimedAction {
    pub fn new(
        action_id: u32,
        duration_sec: f32,
        interrupt_flags: ActionInterruptFlags,
        target_guid: Option<Guid>,
    ) -> Self {
        Self {
            action_id,
            duration_sec: duration_sec.max(0.0),
            elapsed_sec: 0.0,
            interrupt_flags,
            target_guid,
            extra_data: None,
        }
    }

    pub fn with_extra_data(mut self, data: serde_json::Value) -> Self {
        self.extra_data = Some(data);
        self
    }

    /// Returns the progress ratio from 0.0 to 1.0.
    pub fn progress(&self) -> f32 {
        if self.duration_sec <= 0.0 {
            1.0
        } else {
            (self.elapsed_sec / self.duration_sec).clamp(0.0, 1.0)
        }
    }

    /// Returns `true` if the action has completed its elapsed duration.
    pub fn is_finished(&self) -> bool {
        self.elapsed_sec >= self.duration_sec
    }
}

/// Event dispatched when a timed action begins execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionStartedEvent {
    pub entity: Guid,
    pub action_id: u32,
    pub duration_sec: f32,
    pub target_guid: Option<Guid>,
}
impl DomainEvent for ActionStartedEvent {}

/// Event dispatched during action execution conveying elapsed progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionProgressEvent {
    pub entity: Guid,
    pub action_id: u32,
    pub elapsed_sec: f32,
    pub duration_sec: f32,
}
impl DomainEvent for ActionProgressEvent {}

/// Event dispatched when a timed action successfully completes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionCompletedEvent {
    pub entity: Guid,
    pub action_id: u32,
    pub target_guid: Option<Guid>,
    pub extra_data: Option<serde_json::Value>,
}
impl DomainEvent for ActionCompletedEvent {}

/// Event dispatched when an active action is aborted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionInterruptedEvent {
    pub entity: Guid,
    pub action_id: u32,
    pub reason: InterruptReason,
}
impl DomainEvent for ActionInterruptedEvent {}

/// Entity component managing an entity's current timed action.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionQueue {
    pub current: Option<TimedAction>,
}

impl ActionQueue {
    pub fn new() -> Self {
        Self { current: None }
    }

    /// Returns `true` if an action is currently active.
    pub fn is_busy(&self) -> bool {
        self.current.is_some()
    }

    /// Returns a reference to the active action if present.
    pub fn current(&self) -> Option<&TimedAction> {
        self.current.as_ref()
    }

    /// Returns a mutable reference to the active action if present.
    pub fn current_mut(&mut self) -> Option<&mut TimedAction> {
        self.current.as_mut()
    }

    /// Enqueues or starts an action under the specified [`QueuePolicy`].
    pub fn start_action(
        &mut self,
        entity: Guid,
        action: TimedAction,
        policy: QueuePolicy,
        events: &mut EventQueue,
    ) -> Result<(), &'static str> {
        if self.is_busy() {
            match policy {
                QueuePolicy::RejectIfBusy => return Err("Entity is already performing an action"),
                QueuePolicy::Replace => {
                    self.interrupt(entity, InterruptReason::Replaced, events);
                }
            }
        }

        events.push(ActionStartedEvent {
            entity,
            action_id: action.action_id,
            duration_sec: action.duration_sec,
            target_guid: action.target_guid,
        });

        // If the action is instantaneous (0.0s), complete it immediately
        if action.is_finished() {
            events.push(ActionCompletedEvent {
                entity,
                action_id: action.action_id,
                target_guid: action.target_guid,
                extra_data: action.extra_data,
            });
            self.current = None;
        } else {
            self.current = Some(action);
        }

        Ok(())
    }

    /// Interrupts the active action if one exists, emitting an [`ActionInterruptedEvent`].
    pub fn interrupt(
        &mut self,
        entity: Guid,
        reason: InterruptReason,
        events: &mut EventQueue,
    ) -> Option<TimedAction> {
        if let Some(aborted) = self.current.take() {
            events.push(ActionInterruptedEvent {
                entity,
                action_id: aborted.action_id,
                reason,
            });
            Some(aborted)
        } else {
            None
        }
    }
}

/// Batch system executed during the simulation tick to advance active actions.
pub struct ActionTickSystem;

impl TickSystem for ActionTickSystem {
    fn name(&self) -> &'static str {
        "ActionTickSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        enum ActionDecision {
            Interrupt(u32, InterruptReason),
            Progress,
        }

        let mut decisions: Vec<(Guid, ActionDecision)> = Vec::new();

        // Pass 1: Immutable inspect pass determining whether actions are interrupted or advanced
        for (_key, entity) in ctx.entities.iter() {
            if let Some(queue) = entity.get_component::<ActionQueue>() {
                if let Some(action) = queue.current() {
                    let mut interrupt_reason = None;

                    // 1. cancel_on_movement: non-zero velocity
                    if action.interrupt_flags.cancel_on_movement {
                        let (vx, vy, vz) = entity.velocity;
                        if vx.abs() > 1e-4 || vy.abs() > 1e-4 || vz.abs() > 1e-4 {
                            interrupt_reason = Some(InterruptReason::Movement);
                        }
                    }

                    // 2. cancel_on_stun: entity is incapacitated
                    if interrupt_reason.is_none()
                        && action.interrupt_flags.cancel_on_stun
                        && entity.is_incapacitated()
                    {
                        interrupt_reason = Some(InterruptReason::CrowdControl);
                    }

                    // 3. cancel_on_target_lost: target is missing or incapacitated
                    if interrupt_reason.is_none() && action.interrupt_flags.cancel_on_target_lost {
                        if let Some(tgt_guid) = action.target_guid {
                            match ctx.entities.get_by_guid(tgt_guid) {
                                None => {
                                    interrupt_reason = Some(InterruptReason::TargetLost);
                                }
                                Some(tgt_ent) => {
                                    if tgt_ent.is_incapacitated() {
                                        interrupt_reason = Some(InterruptReason::TargetLost);
                                    }
                                }
                            }
                        }
                    }

                    if let Some(reason) = interrupt_reason {
                        decisions.push((
                            entity.guid,
                            ActionDecision::Interrupt(action.action_id, reason),
                        ));
                    } else {
                        decisions.push((entity.guid, ActionDecision::Progress));
                    }
                }
            }
        }

        // Pass 2: Mutable update pass applying decisions
        let mut completed = Vec::new();
        let mut interrupted = Vec::new();
        let mut progressed = Vec::new();

        for (entity_guid, decision) in decisions {
            if let Some(entity) = ctx.entities.get_mut_by_guid(entity_guid) {
                if let Some(queue) = entity.get_component_mut::<ActionQueue>() {
                    match decision {
                        ActionDecision::Interrupt(action_id, reason) => {
                            queue.current = None;
                            interrupted.push((entity_guid, action_id, reason));
                        }
                        ActionDecision::Progress => {
                            if let Some(ref mut action) = queue.current {
                                action.elapsed_sec += ctx.dt_sec;
                                if action.is_finished() {
                                    let finished_action = queue.current.take().unwrap();
                                    completed.push((entity_guid, finished_action));
                                } else {
                                    progressed.push(ActionProgressEvent {
                                        entity: entity_guid,
                                        action_id: action.action_id,
                                        elapsed_sec: action.elapsed_sec,
                                        duration_sec: action.duration_sec,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        for (entity, action_id, reason) in interrupted {
            ctx.events.push(ActionInterruptedEvent {
                entity,
                action_id,
                reason,
            });
        }

        for ev in progressed {
            ctx.events.push(ev);
        }

        // Emit ActionCompletedEvent for completed actions
        for (entity, action) in completed {
            ctx.events.push(ActionCompletedEvent {
                entity,
                action_id: action.action_id,
                target_guid: action.target_guid,
                extra_data: action.extra_data,
            });
        }
    }
}

/// Component tracking action cooldowns and the shared Global Cooldown (GCD).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CooldownTracker {
    pub ability_cooldowns: HashMap<u32, f32>,
    pub gcd_remaining_sec: f32,
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if a specific action or the global cooldown is currently active.
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

    /// Triggers cooldowns for the action and sets the shared GCD timer.
    pub fn start_cooldown(&mut self, ability_id: u32, duration_sec: f32, gcd_sec: f32) {
        if duration_sec > 0.0 {
            self.ability_cooldowns.insert(ability_id, duration_sec);
        }
        if gcd_sec > 0.0 {
            self.gcd_remaining_sec = self.gcd_remaining_sec.max(gcd_sec);
        }
    }

    /// Resets the cooldown for a specific action (e.g. on compensation refund).
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

/// Definition of a pluggable timed action or ability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub id: u32,
    pub name: String,
    pub cast_time_sec: f32,
    pub cooldown_sec: f32,
    pub gcd_sec: f32,
    pub range: f32,
    pub resource_cost: Option<(String, f32)>,
    pub interrupt_flags: ActionInterruptFlags,
}

/// Backward compatibility alias for `ActionDefinition`.
pub use ActionDefinition as AbilityDefinition;

/// Batch tick system advancing entity action cooldowns.
#[derive(Default)]
pub struct ActionCooldownSystem;

impl TickSystem for ActionCooldownSystem {
    fn name(&self) -> &'static str {
        "ActionCooldownSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        for (_key, entity) in ctx.entities.iter_mut() {
            if let Some(cd) = entity.get_component_mut::<CooldownTracker>() {
                cd.tick(ctx.dt_sec);
            }
        }
    }
}

/// Backward compatibility alias for `ActionCooldownSystem`.
pub use ActionCooldownSystem as AbilityTickSystem;

/// Result of resolving candidate targets for a boundary-crossing AoE action.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryAoeTargeting {
    pub local_targets: Vec<SpatialTargetCandidate>,
    pub remote_batches: BTreeMap<u32, Vec<SpatialTargetCandidate>>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;
    use crate::octree::Octree;

    #[test]
    fn test_instant_action_completes_immediately() {
        let mut queue = ActionQueue::new();
        let mut events = EventQueue::new();
        let entity = Guid::new(EntityType::Player, 1, 100);

        let action = TimedAction::new(101, 0.0, ActionInterruptFlags::default(), None);
        let res = queue.start_action(entity, action, QueuePolicy::Replace, &mut events);
        assert!(res.is_ok());
        assert!(!queue.is_busy());

        let started = events.drain::<ActionStartedEvent>();
        let completed = events.drain::<ActionCompletedEvent>();
        assert_eq!(started.len(), 1);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].action_id, 101);
    }

    #[test]
    fn test_timed_action_progress_and_completion_via_tick_system() {
        let mut entities = crate::entity::EntityStore::new();
        let bounds =
            crate::octree::BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);
        let mut events = EventQueue::new();
        let guid = Guid::new(EntityType::Player, 1, 200);

        let key = entities.spawn(guid, (0.0, 0.0, 0.0));
        let mut queue = ActionQueue::new();
        let action = TimedAction::new(
            42,
            0.10, // 100ms
            ActionInterruptFlags::default(),
            None,
        );
        queue
            .start_action(guid, action, QueuePolicy::RejectIfBusy, &mut events)
            .unwrap();
        entities.get_mut(key).unwrap().add_component(queue);

        // Tick 1: dt = 0.05s (50ms) -> Action should be halfway done
        let mut tick_system = ActionTickSystem;
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.05,
                tick: 1,
                collision_world: None,
            };
            tick_system.update(&mut ctx);
        }

        let completed = events.drain::<ActionCompletedEvent>();
        assert_eq!(completed.len(), 0);

        let ent = entities.get_by_guid(guid).unwrap();
        let q = ent.get_component::<ActionQueue>().unwrap();
        assert!(q.is_busy());
        assert!((q.current().unwrap().progress() - 0.5).abs() < 1e-4);

        // Tick 2: dt = 0.05s (50ms) -> Action should complete
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.05,
                tick: 2,
                collision_world: None,
            };
            tick_system.update(&mut ctx);
        }

        let completed = events.drain::<ActionCompletedEvent>();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].action_id, 42);

        let ent = entities.get_by_guid(guid).unwrap();
        let q = ent.get_component::<ActionQueue>().unwrap();
        assert!(!q.is_busy());
    }

    #[test]
    fn test_action_interruption_by_movement_and_damage() {
        let mut queue = ActionQueue::new();
        let mut events = EventQueue::new();
        let entity = Guid::new(EntityType::Player, 1, 300);

        let action = TimedAction::new(777, 2.0, ActionInterruptFlags::default(), None);
        queue
            .start_action(entity, action, QueuePolicy::RejectIfBusy, &mut events)
            .unwrap();
        assert!(queue.is_busy());

        // Interrupt with Movement
        let aborted = queue.interrupt(entity, InterruptReason::Movement, &mut events);
        assert!(aborted.is_some());
        assert!(!queue.is_busy());

        let interrupted = events.drain::<ActionInterruptedEvent>();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].action_id, 777);
        assert_eq!(interrupted[0].reason, InterruptReason::Movement);
    }

    #[test]
    fn test_queue_policy_reject_and_replace() {
        let mut queue = ActionQueue::new();
        let mut events = EventQueue::new();
        let entity = Guid::new(EntityType::Player, 1, 400);

        let action1 = TimedAction::new(1, 3.0, ActionInterruptFlags::default(), None);
        queue
            .start_action(entity, action1, QueuePolicy::RejectIfBusy, &mut events)
            .unwrap();

        // Reject if busy
        let action2 = TimedAction::new(2, 3.0, ActionInterruptFlags::default(), None);
        let res = queue.start_action(
            entity,
            action2.clone(),
            QueuePolicy::RejectIfBusy,
            &mut events,
        );
        assert!(res.is_err());
        assert_eq!(queue.current().unwrap().action_id, 1);

        // Replace policy
        let res_replace = queue.start_action(entity, action2, QueuePolicy::Replace, &mut events);
        assert!(res_replace.is_ok());
        assert_eq!(queue.current().unwrap().action_id, 2);

        let interrupted = events.drain::<ActionInterruptedEvent>();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].action_id, 1);
        assert_eq!(interrupted[0].reason, InterruptReason::Replaced);
    }

    #[test]
    fn test_action_tick_system_progress_and_interrupt_conditions() {
        use crate::entity::{state_flags, EntityStore};

        let bounds =
            crate::octree::BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);
        let mut events = EventQueue::new();
        let mut entities = EntityStore::new();

        let caster_guid = Guid::new(EntityType::Player, 1, 500);
        let target_guid = Guid::new(EntityType::Player, 1, 501);

        let caster_key = entities.spawn(caster_guid, (0.0, 0.0, 0.0));
        let target_key = entities.spawn(target_guid, (10.0, 0.0, 0.0));

        let mut queue = ActionQueue::new();
        let action = TimedAction::new(99, 1.0, ActionInterruptFlags::default(), Some(target_guid));
        queue
            .start_action(caster_guid, action, QueuePolicy::RejectIfBusy, &mut events)
            .unwrap();
        entities.get_mut(caster_key).unwrap().add_component(queue);

        let mut tick_system = ActionTickSystem;

        // Tick 1: dt = 0.2s -> Emits ActionProgressEvent
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.2,
                tick: 1,
                collision_world: None,
            };
            tick_system.update(&mut ctx);
        }
        let progress = events.drain::<ActionProgressEvent>();
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].action_id, 99);
        assert!((progress[0].elapsed_sec - 0.2).abs() < 1e-4);

        // Tick 2: Caster moves (velocity != 0) -> ActionInterruptedEvent(Movement)
        entities.get_mut(caster_key).unwrap().velocity = (1.0, 0.0, 0.0);
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.2,
                tick: 2,
                collision_world: None,
            };
            tick_system.update(&mut ctx);
        }
        let interrupted = events.drain::<ActionInterruptedEvent>();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].reason, InterruptReason::Movement);

        // Reset velocity, start new action with target
        entities.get_mut(caster_key).unwrap().velocity = (0.0, 0.0, 0.0);
        let action2 =
            TimedAction::new(100, 1.0, ActionInterruptFlags::default(), Some(target_guid));
        entities
            .get_mut(caster_key)
            .unwrap()
            .get_component_mut::<ActionQueue>()
            .unwrap()
            .start_action(caster_guid, action2, QueuePolicy::Replace, &mut events)
            .unwrap();

        // Target becomes incapacitated -> ActionInterruptedEvent(TargetLost)
        entities.get_mut(target_key).unwrap().state_flags |= state_flags::INCAPACITATED;
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.2,
                tick: 3,
                collision_world: None,
            };
            tick_system.update(&mut ctx);
        }
        let interrupted2 = events.drain::<ActionInterruptedEvent>();
        assert_eq!(interrupted2.len(), 1);
        assert_eq!(interrupted2[0].reason, InterruptReason::TargetLost);

        // Start action 3, caster becomes incapacitated -> ActionInterruptedEvent(CrowdControl)
        let action3 = TimedAction::new(101, 1.0, ActionInterruptFlags::default(), None);
        entities
            .get_mut(caster_key)
            .unwrap()
            .get_component_mut::<ActionQueue>()
            .unwrap()
            .start_action(caster_guid, action3, QueuePolicy::Replace, &mut events)
            .unwrap();
        entities.get_mut(caster_key).unwrap().state_flags |= state_flags::INCAPACITATED;
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.2,
                tick: 4,
                collision_world: None,
            };
            tick_system.update(&mut ctx);
        }
        let interrupted3 = events.drain::<ActionInterruptedEvent>();
        assert_eq!(interrupted3.len(), 1);
        assert_eq!(interrupted3[0].reason, InterruptReason::CrowdControl);
    }
}
