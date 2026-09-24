//! Synchronous Hook Dispatcher and Event Pipeline.
//!
//! This module provides a synchronous event pipeline for gameplay lifecycle events
//! (e.g. `BeforeMove`, `BeforeInteract`), allowing decoupled gameplay modules to inspect,
//! mutate, or veto pending actions via [`EventContext`].

use std::sync::Arc;

use crate::guid::Guid;

/// Gameplay lifecycle events that can be dispatched through the hook pipeline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// Dispatched before an entity moves; handlers can inspect or veto the movement.
    BeforeMove {
        entity: Guid,
        from: (f32, f32, f32),
        to: (f32, f32, f32),
    },
    /// Dispatched after an entity has moved.
    OnMove {
        entity: Guid,
        position: (f32, f32, f32),
    },
    /// Dispatched before an interaction occurs; handlers can inspect or veto the interaction.
    BeforeInteract {
        initiator: Guid,
        target: Guid,
        action_id: u32,
    },
    /// Dispatched after an interaction has taken place.
    OnInteract {
        initiator: Guid,
        target: Guid,
        action_id: u32,
    },
    /// Dispatched when an entity is spawned into the world.
    OnSpawn {
        entity: Guid,
        position: (f32, f32, f32),
    },
    /// Dispatched when an entity is despawned from the world.
    OnDespawn {
        entity: Guid,
    },
    /// Dispatched during the simulation tick for each active entity.
    OnTick {
        entity: Guid,
        tick: u64,
        dt_sec: f32,
    },
}

impl Event {
    /// Returns the primary entity associated with this event.
    ///
    /// For interaction events, this returns the `initiator` GUID.
    pub fn entity(&self) -> Guid {
        match *self {
            Event::BeforeMove { entity, .. } => entity,
            Event::OnMove { entity, .. } => entity,
            Event::BeforeInteract { initiator, .. } => initiator,
            Event::OnInteract { initiator, .. } => initiator,
            Event::OnSpawn { entity, .. } => entity,
            Event::OnDespawn { entity } => entity,
            Event::OnTick { entity, .. } => entity,
        }
    }
}

/// Mutable context passed to hook handlers during event dispatch.
///
/// Handlers can inspect or veto an action by calling [`EventContext::cancel`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EventContext {
    canceled: bool,
    cancel_reason: Option<String>,
}

impl EventContext {
    /// Creates a new un-canceled `EventContext`.
    pub fn new() -> Self {
        Self {
            canceled: false,
            cancel_reason: None,
        }
    }

    /// Cancels / vetos the event with a descriptive reason.
    pub fn cancel(&mut self, reason: impl Into<String>) {
        self.canceled = true;
        self.cancel_reason = Some(reason.into());
    }

    /// Returns `true` if the event has been canceled.
    pub fn is_canceled(&self) -> bool {
        self.canceled
    }

    /// Returns the cancellation reason, if the event was canceled.
    pub fn cancel_reason(&self) -> Option<&str> {
        self.cancel_reason.as_deref()
    }
}

/// Trait for synchronous hook handlers.
///
/// Handlers receive a reference to the dispatched [`Event`] and a mutable reference
/// to the [`EventContext`] where they can call `ctx.cancel(...)` to veto the action.
pub trait HookHandler: Send + Sync {
    /// Handles a dispatched event and optionally modifies the event context.
    fn handle(&self, event: &Event, ctx: &mut EventContext);
}

/// Blanket implementation allowing closures to act as hook handlers.
impl<F> HookHandler for F
where
    F: Fn(&Event, &mut EventContext) + Send + Sync,
{
    fn handle(&self, event: &Event, ctx: &mut EventContext) {
        self(event, ctx);
    }
}

/// Wrapper to allow `Arc<dyn HookHandler>` to be treated as a `HookHandler`.
struct ArcHookHandler(Arc<dyn HookHandler>);

impl HookHandler for ArcHookHandler {
    fn handle(&self, event: &Event, ctx: &mut EventContext) {
        self.0.handle(event, ctx);
    }
}

/// Synchronous hook dispatcher that manages handlers and dispatches events in sequence.
///
/// Handlers are executed in registration order. If any handler cancels the event,
/// subsequent handlers are skipped, and the canceled context is returned to the caller.
#[derive(Default)]
pub struct HookDispatcher {
    handlers: Vec<Box<dyn HookHandler>>,
}

impl HookDispatcher {
    /// Creates a new empty `HookDispatcher`.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Registers a boxed hook handler.
    pub fn register_handler(&mut self, handler: Box<dyn HookHandler>) {
        self.handlers.push(handler);
    }

    /// Registers a hook handler wrapped in an `Arc`.
    pub fn register_arc(&mut self, handler: Arc<dyn HookHandler>) {
        self.handlers.push(Box::new(ArcHookHandler(handler)));
    }

    /// Registers a closure as a hook handler.
    pub fn register_fn<F>(&mut self, handler: F)
    where
        F: Fn(&Event, &mut EventContext) + Send + Sync + 'static,
    {
        self.handlers.push(Box::new(handler));
    }

    /// Dispatches an event through all registered handlers in order.
    ///
    /// Execution stops immediately if a handler cancels the event.
    pub fn dispatch(&self, event: &Event) -> EventContext {
        let mut ctx = EventContext::new();
        for handler in &self.handlers {
            handler.handle(event, &mut ctx);
            if ctx.is_canceled() {
                break;
            }
        }
        ctx
    }

    /// Convenience helper to dispatch a `BeforeMove` event.
    pub fn dispatch_before_move(
        &self,
        entity: Guid,
        from: (f32, f32, f32),
        to: (f32, f32, f32),
    ) -> EventContext {
        self.dispatch(&Event::BeforeMove { entity, from, to })
    }

    /// Convenience helper to dispatch an `OnMove` event.
    pub fn dispatch_on_move(&self, entity: Guid, position: (f32, f32, f32)) -> EventContext {
        self.dispatch(&Event::OnMove { entity, position })
    }

    /// Convenience helper to dispatch a `BeforeInteract` event.
    pub fn dispatch_before_interact(
        &self,
        initiator: Guid,
        target: Guid,
        action_id: u32,
    ) -> EventContext {
        self.dispatch(&Event::BeforeInteract {
            initiator,
            target,
            action_id,
        })
    }

    /// Convenience helper to dispatch an `OnInteract` event.
    pub fn dispatch_on_interact(
        &self,
        initiator: Guid,
        target: Guid,
        action_id: u32,
    ) -> EventContext {
        self.dispatch(&Event::OnInteract {
            initiator,
            target,
            action_id,
        })
    }

    /// Convenience helper to dispatch an `OnSpawn` event.
    pub fn dispatch_on_spawn(&self, entity: Guid, position: (f32, f32, f32)) -> EventContext {
        self.dispatch(&Event::OnSpawn { entity, position })
    }

    /// Convenience helper to dispatch an `OnDespawn` event.
    pub fn dispatch_on_despawn(&self, entity: Guid) -> EventContext {
        self.dispatch(&Event::OnDespawn { entity })
    }

    /// Convenience helper to dispatch an `OnTick` event.
    pub fn dispatch_tick(&self, entity: Guid, tick: u64, dt_sec: f32) -> EventContext {
        self.dispatch(&Event::OnTick {
            entity,
            tick,
            dt_sec,
        })
    }

    /// Returns the number of registered handlers.
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// Returns `true` if there are no registered handlers.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Clears all registered handlers.
    pub fn clear(&mut self) {
        self.handlers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    #[test]
    fn test_event_context_defaults_and_cancel() {
        let mut ctx = EventContext::new();
        assert!(!ctx.is_canceled());
        assert_eq!(ctx.cancel_reason(), None);

        ctx.cancel("inventory full");
        assert!(ctx.is_canceled());
        assert_eq!(ctx.cancel_reason(), Some("inventory full"));
    }

    #[test]
    fn test_event_entity_accessor() {
        let p1 = Guid::new(EntityType::Player, 1, 100);
        let p2 = Guid::new(EntityType::Player, 1, 200);

        let before_move = Event::BeforeMove {
            entity: p1,
            from: (0.0, 0.0, 0.0),
            to: (1.0, 1.0, 1.0),
        };
        assert_eq!(before_move.entity(), p1);

        let on_move = Event::OnMove {
            entity: p1,
            position: (1.0, 1.0, 1.0),
        };
        assert_eq!(on_move.entity(), p1);

        let before_interact = Event::BeforeInteract {
            initiator: p1,
            target: p2,
            action_id: 42,
        };
        assert_eq!(before_interact.entity(), p1);

        let on_interact = Event::OnInteract {
            initiator: p1,
            target: p2,
            action_id: 42,
        };
        assert_eq!(on_interact.entity(), p1);

        let on_spawn = Event::OnSpawn {
            entity: p1,
            position: (5.0, 5.0, 5.0),
        };
        assert_eq!(on_spawn.entity(), p1);

        let on_despawn = Event::OnDespawn { entity: p1 };
        assert_eq!(on_despawn.entity(), p1);
    }

    #[test]
    fn test_unvetoed_event_pass() {
        let mut dispatcher = HookDispatcher::new();
        let calls = Arc::new(AtomicU32::new(0));

        let calls_clone = Arc::clone(&calls);
        dispatcher.register_fn(move |_event, _ctx| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
        });

        let p1 = Guid::new(EntityType::Player, 1, 1);
        let ctx = dispatcher.dispatch_before_move(p1, (0.0, 0.0, 0.0), (1.0, 0.0, 0.0));

        assert!(!ctx.is_canceled());
        assert_eq!(ctx.cancel_reason(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_handler_ordering() {
        let mut dispatcher = HookDispatcher::new();
        let execution_order = Arc::new(Mutex::new(Vec::new()));

        for i in 1..=3 {
            let order_clone = Arc::clone(&execution_order);
            dispatcher.register_fn(move |_event, _ctx| {
                order_clone.lock().unwrap().push(i);
            });
        }

        let p1 = Guid::new(EntityType::Player, 1, 1);
        let ctx = dispatcher.dispatch_on_move(p1, (10.0, 20.0, 30.0));

        assert!(!ctx.is_canceled());
        let order = execution_order.lock().unwrap().clone();
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn test_veto_logic_and_short_circuit() {
        let mut dispatcher = HookDispatcher::new();
        let execution_order = Arc::new(Mutex::new(Vec::new()));

        // Handler 1: checks and records
        let order1 = Arc::clone(&execution_order);
        dispatcher.register_fn(move |_event, _ctx| {
            order1.lock().unwrap().push("handler_1");
        });

        // Handler 2: vetos the event
        let order2 = Arc::clone(&execution_order);
        dispatcher.register_fn(move |_event, ctx| {
            order2.lock().unwrap().push("handler_2");
            ctx.cancel("speed cap exceeded");
        });

        // Handler 3: should NOT be called due to short-circuit
        let order3 = Arc::clone(&execution_order);
        dispatcher.register_fn(move |_event, _ctx| {
            order3.lock().unwrap().push("handler_3");
        });

        let p1 = Guid::new(EntityType::Player, 1, 1);
        let ctx = dispatcher.dispatch_before_move(p1, (0.0, 0.0, 0.0), (100.0, 0.0, 0.0));

        assert!(ctx.is_canceled());
        assert_eq!(ctx.cancel_reason(), Some("speed cap exceeded"));

        let order = execution_order.lock().unwrap().clone();
        assert_eq!(order, vec!["handler_1", "handler_2"]);
    }

    #[test]
    fn test_struct_handler_and_arc_registration() {
        struct SpeedValidator {
            max_speed: f32,
        }

        impl HookHandler for SpeedValidator {
            fn handle(&self, event: &Event, ctx: &mut EventContext) {
                if let Event::BeforeMove { from, to, .. } = event {
                    let dx = to.0 - from.0;
                    let dy = to.1 - from.1;
                    let dz = to.2 - from.2;
                    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                    if dist > self.max_speed {
                        ctx.cancel(format!("teleport detected: dist={:.1}", dist));
                    }
                }
            }
        }

        let mut dispatcher = HookDispatcher::new();
        let validator = Arc::new(SpeedValidator { max_speed: 10.0 });
        dispatcher.register_arc(validator);

        let p1 = Guid::new(EntityType::Player, 1, 1);

        // Valid move within speed cap
        let ctx1 = dispatcher.dispatch_before_move(p1, (0.0, 0.0, 0.0), (5.0, 0.0, 0.0));
        assert!(!ctx1.is_canceled());

        // Invalid move exceeding speed cap
        let ctx2 = dispatcher.dispatch_before_move(p1, (0.0, 0.0, 0.0), (50.0, 0.0, 0.0));
        assert!(ctx2.is_canceled());
        assert_eq!(ctx2.cancel_reason(), Some("teleport detected: dist=50.0"));
    }

    #[test]
    fn test_all_convenience_dispatchers() {
        let mut dispatcher = HookDispatcher::new();
        let received_events = Arc::new(Mutex::new(Vec::new()));

        let rec = Arc::clone(&received_events);
        dispatcher.register_fn(move |event, _ctx| {
            rec.lock().unwrap().push(event.clone());
        });

        let p1 = Guid::new(EntityType::Player, 1, 10);
        let npc = Guid::new(EntityType::Npc, 1, 20);

        dispatcher.dispatch_before_move(p1, (0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
        dispatcher.dispatch_on_move(p1, (1.0, 1.0, 1.0));
        dispatcher.dispatch_before_interact(p1, npc, 101);
        dispatcher.dispatch_on_interact(p1, npc, 101);
        dispatcher.dispatch_on_spawn(p1, (0.0, 0.0, 0.0));
        dispatcher.dispatch_on_despawn(p1);

        let events = received_events.lock().unwrap().clone();
        assert_eq!(events.len(), 6);
        assert_eq!(
            events[0],
            Event::BeforeMove {
                entity: p1,
                from: (0.0, 0.0, 0.0),
                to: (1.0, 1.0, 1.0),
            }
        );
        assert_eq!(
            events[1],
            Event::OnMove {
                entity: p1,
                position: (1.0, 1.0, 1.0),
            }
        );
        assert_eq!(
            events[2],
            Event::BeforeInteract {
                initiator: p1,
                target: npc,
                action_id: 101,
            }
        );
        assert_eq!(
            events[3],
            Event::OnInteract {
                initiator: p1,
                target: npc,
                action_id: 101,
            }
        );
        assert_eq!(
            events[4],
            Event::OnSpawn {
                entity: p1,
                position: (0.0, 0.0, 0.0),
            }
        );
        assert_eq!(events[5], Event::OnDespawn { entity: p1 });
    }

    #[test]
    fn test_box_dyn_hook_handler_registration() {
        struct InterceptInteract;

        impl HookHandler for InterceptInteract {
            fn handle(&self, event: &Event, ctx: &mut EventContext) {
                if let Event::BeforeInteract { action_id, .. } = event {
                    if *action_id == 999 {
                        ctx.cancel("forbidden action");
                    }
                }
            }
        }

        let mut dispatcher = HookDispatcher::new();
        dispatcher.register_handler(Box::new(InterceptInteract));

        let p1 = Guid::new(EntityType::Player, 1, 1);
        let p2 = Guid::new(EntityType::Player, 1, 2);

        let ctx_allowed = dispatcher.dispatch_before_interact(p1, p2, 1);
        assert!(!ctx_allowed.is_canceled());

        let ctx_denied = dispatcher.dispatch_before_interact(p1, p2, 999);
        assert!(ctx_denied.is_canceled());
        assert_eq!(ctx_denied.cancel_reason(), Some("forbidden action"));
    }

    #[test]
    fn test_dispatcher_management() {
        let mut dispatcher = HookDispatcher::new();
        assert!(dispatcher.is_empty());
        assert_eq!(dispatcher.handler_count(), 0);

        dispatcher.register_fn(|_, _| {});
        assert!(!dispatcher.is_empty());
        assert_eq!(dispatcher.handler_count(), 1);

        dispatcher.clear();
        assert!(dispatcher.is_empty());
        assert_eq!(dispatcher.handler_count(), 0);
    }

    #[test]
    fn test_multiple_sequential_dispatches_isolated_contexts() {
        let mut dispatcher = HookDispatcher::new();
        dispatcher.register_fn(|event, ctx| {
            if let Event::BeforeInteract { action_id, .. } = event {
                if *action_id == 100 {
                    ctx.cancel("vetoed");
                }
            }
        });

        let p1 = Guid::new(EntityType::Player, 1, 1);
        let p2 = Guid::new(EntityType::Player, 1, 2);

        let ctx1 = dispatcher.dispatch_before_interact(p1, p2, 100);
        assert!(ctx1.is_canceled());

        let ctx2 = dispatcher.dispatch_before_interact(p1, p2, 200);
        assert!(!ctx2.is_canceled());
        assert_eq!(ctx2.cancel_reason(), None);
    }

    #[test]
    fn test_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HookDispatcher>();
        assert_send_sync::<Event>();
        assert_send_sync::<EventContext>();
    }
}
