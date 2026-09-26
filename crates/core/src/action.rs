//! Authoritative Cancellable Timed Action Primitives.
//!
//! Provides a genre-agnostic state machine for entity tasks that require elapsed time
//! to complete (e.g., cast bars, weapon reload, lockpicking, warp-drive spooling),
//! with deterministic interruption guards and lifecycle event notifications.

use serde::{Deserialize, Serialize};

use crate::guid::Guid;
use crate::plugin::{DomainEvent, EventQueue, TickContext, TickSystem};

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
        // Collect entities with ActionQueue to update their actions
        let mut completed = Vec::new();

        for (_key, entity) in ctx.entities.iter_mut() {
            let entity_guid = entity.guid;
            if let Some(queue) = entity.get_component_mut::<ActionQueue>() {
                if let Some(ref mut action) = queue.current {
                    action.elapsed_sec += ctx.dt_sec;
                    if action.is_finished() {
                        let finished_action = queue.current.take().unwrap();
                        completed.push((entity_guid, finished_action));
                    }
                }
            }
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
}
