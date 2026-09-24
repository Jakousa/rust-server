//! Simulation node tick loop and authoritative movement processing.
//!
//! Drives the fixed-frequency (20 Hz / 50ms) simulation game loop, entity iteration,
//! hook dispatches, and authoritative client movement validation.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::MissedTickBehavior;

use mmo_core::{
    BoundingBox3D, ComponentCodecRegistry, EntityStore, Event, EventQueue, GameplayPlugin, Guid,
    HookDispatcher, ModularPacketRouter, Octree, PluginManager, TickContext, TickSystem,
};

use crate::aoi::{AoiBroadcastJob, AoiManager, ObserverConfig, ObserverState};
use crate::kinematics::KinematicValidator;
use crate::network::{GhostBroadcaster, GhostStore, NodeServiceImpl};

/// Authoritative simulation node driving world ticks, spatial partitioning,
/// dynamic entity stores, and movement validation.
pub struct TickSimulation {
    pub node_id: u16,
    pub entity_store: EntityStore,
    pub octree: Octree,
    pub hooks: HookDispatcher,
    pub kinematics: KinematicValidator,
    pub tick_counter: u64,
    pub ghost_store: GhostStore,
    pub codecs: ComponentCodecRegistry,
    pub event_queue: EventQueue,
    pub packets: ModularPacketRouter,
    pub tick_systems: Vec<Box<dyn TickSystem>>,
    pub plugin_manager: PluginManager,
    pub aoi: AoiManager,
    pub pending_broadcast_jobs: Vec<AoiBroadcastJob>,
}

impl TickSimulation {
    /// Simulation tick rate: 20 Hz (20 ticks per second).
    pub const TICK_RATE_HZ: u32 = 20;
    /// Tick duration interval: 50 milliseconds.
    pub const TICK_INTERVAL: Duration = Duration::from_millis(50);
    /// Tick delta time in seconds: 0.05s.
    pub const TICK_DT_SEC: f32 = 0.05;

    /// Creates a new `TickSimulation` instance with default kinematic validation.
    pub fn new(node_id: u16, bounds: BoundingBox3D) -> Self {
        Self::with_kinematics(node_id, bounds, KinematicValidator::default())
    }

    /// Creates a new `TickSimulation` instance with a custom `KinematicValidator`.
    pub fn with_kinematics(
        node_id: u16,
        bounds: BoundingBox3D,
        kinematics: KinematicValidator,
    ) -> Self {
        Self {
            node_id,
            entity_store: EntityStore::new(),
            octree: Octree::new(bounds),
            hooks: HookDispatcher::new(),
            kinematics,
            tick_counter: 0,
            ghost_store: GhostStore::new(),
            codecs: ComponentCodecRegistry::new(),
            event_queue: EventQueue::new(),
            packets: ModularPacketRouter::new(),
            tick_systems: Vec::new(),
            plugin_manager: PluginManager::new(),
            aoi: AoiManager::new(),
            pending_broadcast_jobs: Vec::new(),
        }
    }

    /// Returns an immutable reference to the ghost store.
    pub fn ghost_store(&self) -> &GhostStore {
        &self.ghost_store
    }

    /// Returns a mutable reference to the ghost store.
    pub fn ghost_store_mut(&mut self) -> &mut GhostStore {
        &mut self.ghost_store
    }

    /// Wraps this simulation instance in a shared asynchronous `RwLock`.
    pub fn into_shared(self) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(self))
    }

    /// Creates a [`NodeServiceImpl`] backed by a shared simulation instance.
    pub fn create_service(shared_sim: Arc<RwLock<Self>>) -> NodeServiceImpl {
        NodeServiceImpl::new(shared_sim)
    }

    /// Creates a [`GhostBroadcaster`] configured for this simulation node.
    pub fn create_ghost_broadcaster(&self, boundary_margin: f32) -> GhostBroadcaster {
        GhostBroadcaster::new(self.node_id as u32, boundary_margin)
    }

    /// Spawns an entity into both `EntityStore` and `Octree`, dispatching `OnSpawn`.
    ///
    /// Returns `true` if the entity was successfully spawned within octree boundaries.
    pub fn spawn_entity(&mut self, guid: Guid, position: (f32, f32, f32)) -> bool {
        if !self.octree.bounds.contains(position) {
            return false;
        }

        self.entity_store.spawn(guid, position);
        self.octree.insert(guid, position);
        self.hooks.dispatch(&Event::OnSpawn {
            entity: guid,
            position,
        });
        true
    }

    /// Despawns an entity by GUID from both `EntityStore` and `Octree`, dispatching `OnDespawn`.
    ///
    /// Returns `true` if the entity existed and was removed.
    pub fn despawn_entity(&mut self, guid: Guid) -> bool {
        if let Some(entity) = self.entity_store.despawn_by_guid(guid) {
            if !self.octree.remove(guid, entity.position) {
                self.octree.remove_by_guid(guid);
            }
            self.hooks.dispatch(&Event::OnDespawn { entity: guid });
            true
        } else {
            false
        }
    }

    /// Executes a single simulation step.
    ///
    /// Registers a gameplay plugin to be initialized in this simulation node.
    pub fn add_plugin(&mut self, plugin: Box<dyn GameplayPlugin>) {
        self.plugin_manager.add_plugin(plugin);
    }

    /// Initializes and builds all registered plugins.
    pub fn init_plugins(&mut self) {
        self.plugin_manager.build_all(
            &mut self.hooks,
            &mut self.codecs,
            &mut self.event_queue,
            &mut self.packets,
            &mut self.tick_systems,
        );
    }

    /// Increments the `tick_counter`, runs batch tick systems, iterates through active entities
    /// in `EntityStore`, executes tick updates, and triggers synchronous hook dispatches (`Event::OnTick`).
    pub fn step(&mut self, dt_sec: f32) {
        self.tick_counter += 1;

        // 1. Run batch tick systems registered by plugins
        let mut tick_ctx = TickContext {
            entities: &mut self.entity_store,
            octree: &mut self.octree,
            events: &mut self.event_queue,
            dt_sec,
            tick: self.tick_counter,
        };
        for system in &mut self.tick_systems {
            system.update(&mut tick_ctx);
        }

        // 2. Snapshot entity GUIDs to avoid borrowing conflicts during hook dispatch
        let guids: Vec<Guid> = self.entity_store.iter().map(|(_, ent)| ent.guid).collect();

        for guid in guids {
            self.hooks.dispatch(&Event::OnTick {
                entity: guid,
                tick: self.tick_counter,
                dt_sec,
            });
        }

        // 3. Update AoI observers and queue broadcast jobs
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut aoi_jobs = self.aoi.update_observers(
            &self.entity_store,
            &self.octree,
            self.tick_counter,
            now_ms,
        );
        self.pending_broadcast_jobs.append(&mut aoi_jobs);
    }

    /// Drains and returns all queued AoI broadcast jobs accumulated during ticks.
    pub fn drain_broadcast_jobs(&mut self) -> Vec<AoiBroadcastJob> {
        std::mem::take(&mut self.pending_broadcast_jobs)
    }

    /// Registers a player session as an AoI observer.
    pub fn register_aoi_observer(
        &mut self,
        session_id: u64,
        player_guid: Guid,
        config: ObserverConfig,
    ) {
        self.aoi.register_observer(session_id, player_guid, config);
    }

    /// Unregisters an AoI observer by session ID.
    pub fn unregister_aoi_observer(&mut self, session_id: u64) -> Option<ObserverState> {
        self.aoi.unregister_observer(session_id)
    }

    /// Convenience helper executing a standard 20 Hz (50ms) simulation tick step.
    pub fn step_tick(&mut self) {
        self.step(Self::TICK_DT_SEC);
    }

    /// Runs a fixed number of 20 Hz simulation ticks asynchronously.
    ///
    /// Each tick executes after a 50ms interval, maintaining a precise 20 Hz rhythm.
    pub async fn run_ticks(&mut self, count: usize) -> u64 {
        let mut interval = tokio::time::interval(Self::TICK_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Burst);

        for _ in 0..count {
            interval.tick().await;
            self.step_tick();
        }

        self.tick_counter
    }

    /// Processes a client-reported movement packet authoritatively.
    ///
    /// Performs kinematic speed and teleport validation, spatial boundary checks,
    /// and dispatches `BeforeMove`. If validation passes, updates position in
    /// `EntityStore` and `Octree`, dispatches `OnMove`, and returns `Ok(new_pos)`.
    ///
    /// If validation fails, returns `Err(snap_back_pos)` indicating the authoritative
    /// snap-back destination the client must be corrected to.
    pub fn process_client_move(
        &mut self,
        guid: Guid,
        new_pos: (f32, f32, f32),
        dt_sec: f32,
    ) -> Result<(f32, f32, f32), (f32, f32, f32)> {
        // 1. Fetch current entity coordinates
        let current_pos = match self.entity_store.get_by_guid(guid) {
            Some(entity) => entity.position,
            None => return Err(new_pos),
        };

        // 2. Kinematic validation (speed caps, teleport threshold, NaN/Inf, dt bounds)
        if let Err(violation) = self.kinematics.validate_move(current_pos, new_pos, dt_sec) {
            return Err(violation.snap_back);
        }

        // 3. Spatial world boundary check
        if !self.octree.bounds.contains(new_pos) {
            return Err(current_pos);
        }

        // 3.5. Component movement checks (encumbrance and death status)
        if let Some(entity) = self.entity_store.get_by_guid(guid) {
            if mmo_gameplay::check_encumbrance(&entity.components) {
                return Err(current_pos);
            }
            if let Some(health) = entity.get_component::<mmo_gameplay::HealthComponent>() {
                if health.is_dead {
                    return Err(current_pos);
                }
            }
        }

        // 4. Synchronous BeforeMove hook dispatch (veto / cancel support)
        let ctx = self.hooks.dispatch(&Event::BeforeMove {
            entity: guid,
            from: current_pos,
            to: new_pos,
        });
        if ctx.is_canceled() {
            return Err(current_pos);
        }

        // 5. Update EntityStore position
        if let Some(entity) = self.entity_store.get_mut_by_guid(guid) {
            entity.position = new_pos;
        }

        // 6. Update Octree spatial index
        if !self.octree.remove(guid, current_pos) {
            self.octree.remove_by_guid(guid);
        }
        self.octree.insert(guid, new_pos);

        // 7. Dispatch OnMove hook
        self.hooks.dispatch(&Event::OnMove {
            entity: guid,
            position: new_pos,
        });

        Ok(new_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    fn test_bounds() -> BoundingBox3D {
        BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0))
    }

    #[test]
    fn test_tick_simulation_initialization() {
        let sim = TickSimulation::new(1, test_bounds());
        assert_eq!(sim.node_id, 1);
        assert_eq!(sim.tick_counter, 0);
        assert_eq!(sim.entity_store.count(), 0);
        assert_eq!(sim.octree.len(), 0);
        assert!(sim.octree.is_empty());
        assert_eq!(sim.ghost_store.len(), 0);
        assert!(sim.ghost_store.is_empty());
    }

    #[test]
    fn test_process_client_move_normal_pass() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 100);
        let start_pos = (0.0, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Move 0.4 units over dt = 0.05s (speed = 8.0 units/s <= max 10.0)
        let target_pos = (0.4, 0.0, 0.0);
        let result = sim.process_client_move(guid, target_pos, 0.05);

        assert_eq!(result, Ok(target_pos));

        // EntityStore updated
        let entity = sim.entity_store.get_by_guid(guid).expect("entity must exist");
        assert_eq!(entity.position, target_pos);

        // Octree updated
        let nearby = sim.octree.query_radius(target_pos, 0.1);
        assert_eq!(nearby, vec![guid]);
    }

    #[test]
    fn test_process_client_move_speed_hack_rejection_and_snapback() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 200);
        let start_pos = (5.0, 5.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Attempt to move 5 units in 0.05s (speed = 100.0 units/s > max 10.0)
        let hack_pos = (10.0, 5.0, 0.0);
        let result = sim.process_client_move(guid, hack_pos, 0.05);

        // Expect rejection with snap-back to start_pos
        assert_eq!(result, Err(start_pos));

        // Authoritative entity store remains unchanged
        let entity = sim.entity_store.get_by_guid(guid).expect("entity must exist");
        assert_eq!(entity.position, start_pos);

        // Octree index still reflects original position
        let old_pos_query = sim.octree.query_radius(start_pos, 0.1);
        assert_eq!(old_pos_query, vec![guid]);

        let hack_pos_query = sim.octree.query_radius(hack_pos, 0.1);
        assert!(hack_pos_query.is_empty());
    }

    #[test]
    fn test_process_client_move_out_of_bounds_snap_back() {
        let bounds = BoundingBox3D::new((-10.0, -10.0, -10.0), (10.0, 10.0, 10.0));
        let mut sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 300);
        let start_pos = (9.8, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Move 0.3 units in 0.05s -> reaches 10.1 (outside bounds [-10, 10])
        let out_pos = (10.1, 0.0, 0.0);
        let result = sim.process_client_move(guid, out_pos, 0.05);

        assert_eq!(result, Err(start_pos));
        let entity = sim.entity_store.get_by_guid(guid).unwrap();
        assert_eq!(entity.position, start_pos);
    }

    #[test]
    fn test_process_client_move_hook_veto_snap_back() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 400);
        let start_pos = (0.0, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Register a hook that vetos any movement (e.g. stunned / rooted debuff)
        sim.hooks.register_fn(|event, ctx| {
            if let Event::BeforeMove { entity, .. } = event {
                ctx.cancel(format!("Entity {entity} is rooted"));
            }
        });

        let target_pos = (0.2, 0.0, 0.0);
        let result = sim.process_client_move(guid, target_pos, 0.05);

        assert_eq!(result, Err(start_pos));
        let entity = sim.entity_store.get_by_guid(guid).unwrap();
        assert_eq!(entity.position, start_pos);
    }

    #[test]
    fn test_movement_hooks_dispatched() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 500);
        let start_pos = (0.0, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        let before_count = Arc::new(AtomicUsize::new(0));
        let on_move_count = Arc::new(AtomicUsize::new(0));

        let b_clone = before_count.clone();
        let m_clone = on_move_count.clone();

        sim.hooks.register_fn(move |event, _| match event {
            Event::BeforeMove { .. } => {
                b_clone.fetch_add(1, Ordering::SeqCst);
            }
            Event::OnMove { .. } => {
                m_clone.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        });

        let target_pos = (0.3, 0.0, 0.0);
        let res = sim.process_client_move(guid, target_pos, 0.05);
        assert!(res.is_ok());

        assert_eq!(before_count.load(Ordering::SeqCst), 1);
        assert_eq!(on_move_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_step_iterates_entities_and_dispatches_tick_hooks() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let g1 = Guid::new(EntityType::Player, 1, 601);
        let g2 = Guid::new(EntityType::Npc, 1, 602);
        sim.spawn_entity(g1, (1.0, 1.0, 1.0));
        sim.spawn_entity(g2, (2.0, 2.0, 2.0));

        let tick_events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let t_clone = tick_events.clone();

        sim.hooks.register_fn(move |event, _| {
            if let Event::OnTick {
                entity,
                tick,
                dt_sec,
            } = event
            {
                t_clone.lock().unwrap().push((*entity, *tick, *dt_sec));
            }
        });

        assert_eq!(sim.tick_counter, 0);

        // Execute step 1 (50ms = 0.05s)
        sim.step(0.05);
        assert_eq!(sim.tick_counter, 1);

        {
            let events = tick_events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(events.contains(&(g1, 1, 0.05)));
            assert!(events.contains(&(g2, 1, 0.05)));
        }

        // Execute step 2
        sim.step(0.05);
        assert_eq!(sim.tick_counter, 2);

        {
            let events = tick_events.lock().unwrap();
            assert_eq!(events.len(), 4);
            assert!(events.contains(&(g1, 2, 0.05)));
            assert!(events.contains(&(g2, 2, 0.05)));
        }
    }

    #[tokio::test]
    async fn test_20hz_timed_tick_execution() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 700);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));

        let start = Instant::now();
        // Run 4 ticks at 20 Hz (50ms each = ~200ms total)
        let total_ticks = sim.run_ticks(4).await;
        let elapsed = start.elapsed();

        assert_eq!(total_ticks, 4);
        assert_eq!(sim.tick_counter, 4);
        // Expect at least 150ms elapsed given 50ms intervals
        assert!(
            elapsed >= Duration::from_millis(150),
            "Expected elapsed >= 150ms for 4 ticks, got {:?}",
            elapsed
        );
    }

    #[test]
    fn test_tick_simulation_aoi_broadcast_integration() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let player = Guid::new(EntityType::Player, 1, 100);
        let npc = Guid::new(EntityType::Npc, 1, 200);

        sim.spawn_entity(player, (10.0, 10.0, 10.0));
        sim.register_aoi_observer(1, player, ObserverConfig::default());

        // Step 1: Only player present, no other entities -> no broadcast jobs
        sim.step_tick();
        assert!(sim.drain_broadcast_jobs().is_empty());

        // Spawn NPC within AoI radius
        sim.spawn_entity(npc, (15.0, 10.0, 10.0));

        // Step 2: NPC enters AoI -> broadcast job generated
        sim.step_tick();
        let jobs = sim.drain_broadcast_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].session_id, 1);
        assert_eq!(jobs[0].player_guid, player);
        assert_eq!(jobs[0].packets.len(), 1);

        // Queue drained
        assert!(sim.drain_broadcast_jobs().is_empty());

        // Step 3: Despawn NPC
        sim.despawn_entity(npc);
        sim.step_tick();
        let jobs = sim.drain_broadcast_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].packets.len(), 1);
        match &jobs[0].packets[0] {
            mmo_schema::client::ClientPacket::EntityDespawn(d) => {
                assert_eq!(d.entity_id, npc.raw());
            }
            other => panic!("Expected EntityDespawn, got {:?}", other),
        }
    }
}
