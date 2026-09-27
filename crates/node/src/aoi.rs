//! Area-of-Interest (AoI) observation and delta broadcasting system.
//!
//! Tracks player observation spheres in the Sparse Octree, computes dynamic
//! entity visibility deltas (`entered`, `updated`, `exited`), performs
//! dynamic density-based radius throttling, and produces binary broadcast packets.

use std::collections::{HashMap, HashSet, VecDeque};

use mmo_core::guid::Guid;
use mmo_core::octree::calculate_throttled_aoi;
use mmo_core::{EntityStore, Octree};
use mmo_gameplay::combat::HealthComponent;
use mmo_schema::client::{
    delta_flags, ClientPacket, EntityDeltaBatchPacket, EntityDeltaState, EntityDespawnPacket,
    EntityStatePacket, Vec3,
};
use mmo_schema::quantize::is_teleport_delta;

/// Configuration parameters for an Area-of-Interest observer.
#[derive(Debug, Clone, PartialEq)]
pub struct ObserverConfig {
    /// Base visibility radius in meters. Default is 50.0m.
    pub base_radius: f32,
    /// Maximum entities an observer can observe before dynamic density throttling kicks in. Default is 200.
    pub max_entities: usize,
    /// Minimum movement distance squared (in meters^2) to trigger a delta state packet. Default is 0.0001 (0.01m).
    pub position_epsilon_sq: f32,
    /// Number of ticks between full refresh broadcasts even if stationary. Default is 20 (1 second at 20 Hz).
    pub full_refresh_interval_ticks: u64,
    /// Whether to include the observer's own entity in AoI packets. Default is false.
    pub include_self: bool,
    /// Whether to compress entity updates into batched delta state packets. Default is false.
    pub enable_delta_compression: bool,
    /// Maximum number of historical tick snapshots retained for delta differencing. Default is 60 (3 seconds at 20 Hz).
    pub history_depth_ticks: usize,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            base_radius: 50.0,
            max_entities: 200,
            position_epsilon_sq: 0.0001,
            full_refresh_interval_ticks: 20,
            include_self: false,
            enable_delta_compression: false,
            history_depth_ticks: 60,
        }
    }
}

/// Cached state of an entity observed by a player in a previous tick.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedEntityState {
    pub position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub yaw: f32,
    pub health: u32,
    pub max_health: u32,
    pub state_flags: u32,
    pub last_sent_tick: u64,
}

/// Historical snapshot of all visible entity states sent to an observer at a specific tick.
#[derive(Debug, Clone, PartialEq)]
pub struct ObserverTickSnapshot {
    pub tick: u64,
    pub states: HashMap<Guid, CachedEntityState>,
}

/// Baseline tracking state for delta compression.
#[derive(Debug, Clone)]
pub struct BaselineTracker {
    /// Confirmed baseline tick acknowledged by the client.
    pub confirmed_baseline_tick: Option<u64>,
    /// Bounded ring buffer of recent tick snapshots sent to the client.
    pub history: VecDeque<ObserverTickSnapshot>,
    /// Maximum history depth before oldest snapshots are dropped.
    pub max_history_depth: usize,
}

impl BaselineTracker {
    pub fn new(max_depth: usize) -> Self {
        let capacity = max_depth.clamp(10, 120);
        Self {
            confirmed_baseline_tick: None,
            history: VecDeque::with_capacity(capacity),
            max_history_depth: capacity,
        }
    }

    /// Records a new snapshot for `tick`. If history exceeds `max_history_depth`, drops the oldest.
    pub fn record_snapshot(&mut self, tick: u64, states: HashMap<Guid, CachedEntityState>) {
        if self.history.len() >= self.max_history_depth {
            self.history.pop_front();
        }
        self.history
            .push_back(ObserverTickSnapshot { tick, states });
    }

    /// Finds the snapshot for `tick`, if present.
    pub fn get_snapshot(&self, tick: u64) -> Option<&ObserverTickSnapshot> {
        self.history.iter().find(|s| s.tick == tick)
    }

    /// Processes an incoming ACK from the client.
    ///
    /// Returns `true` if valid and applied, or `false` if rejected (e.g. future tick spoofing).
    pub fn process_ack(&mut self, observed_tick: u64, _ack_mask: u32, current_tick: u64) -> bool {
        if observed_tick > current_tick {
            // Anti-cheat: reject future-tick spoofing attempt that would purge baseline history
            return false;
        }

        let new_base = match self.confirmed_baseline_tick {
            Some(curr) => curr.max(observed_tick),
            None => observed_tick,
        };
        self.confirmed_baseline_tick = Some(new_base);

        // Prune history older than the confirmed baseline tick
        while let Some(front) = self.history.front() {
            if front.tick < new_base {
                self.history.pop_front();
            } else {
                break;
            }
        }
        true
    }
}

/// State tracking for an active player session observer.
#[derive(Debug, Clone)]
pub struct ObserverState {
    pub session_id: u64,
    pub player_guid: Guid,
    pub config: ObserverConfig,
    pub effective_radius: f32,
    pub observed_entities: HashMap<Guid, CachedEntityState>,
    pub baseline_tracker: BaselineTracker,
}

impl ObserverState {
    /// Creates a new `ObserverState` initialized with the given configuration.
    pub fn new(session_id: u64, player_guid: Guid, config: ObserverConfig) -> Self {
        let base_radius = config.base_radius;
        let depth = config.history_depth_ticks;
        Self {
            session_id,
            player_guid,
            config,
            effective_radius: base_radius,
            observed_entities: HashMap::new(),
            baseline_tracker: BaselineTracker::new(depth),
        }
    }
}

/// Encapsulates a batch of outgoing client packets for a specific player session.
#[derive(Debug, Clone, PartialEq)]
pub struct AoiBroadcastJob {
    pub session_id: u64,
    pub player_guid: Guid,
    pub packets: Vec<ClientPacket>,
}

/// Manager responsible for tracking player AoI observers, executing zero-allocation
/// spatial queries, diffing visible entity sets, and emitting broadcast jobs.
pub struct AoiManager {
    observers: HashMap<u64, ObserverState>,
    guid_to_session: HashMap<Guid, u64>,
    // Reusable scratch buffers to eliminate per-tick heap allocations
    scratch_query: Vec<(Guid, (f32, f32, f32), f32)>,
    scratch_visible_guids: HashSet<Guid>,
    scratch_session_ids: Vec<u64>,
    scratch_exited_guids: Vec<Guid>,
}

impl Default for AoiManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AoiManager {
    /// Creates a new `AoiManager` with pre-allocated scratch buffers.
    pub fn new() -> Self {
        Self {
            observers: HashMap::new(),
            guid_to_session: HashMap::new(),
            scratch_query: Vec::with_capacity(512),
            scratch_visible_guids: HashSet::with_capacity(512),
            scratch_session_ids: Vec::with_capacity(64),
            scratch_exited_guids: Vec::with_capacity(128),
        }
    }

    /// Registers a new player session as an AoI observer.
    pub fn register_observer(
        &mut self,
        session_id: u64,
        player_guid: Guid,
        config: ObserverConfig,
    ) {
        let observer = ObserverState::new(session_id, player_guid, config);
        self.observers.insert(session_id, observer);
        self.guid_to_session.insert(player_guid, session_id);
    }

    /// Unregisters an observer by session ID.
    pub fn unregister_observer(&mut self, session_id: u64) -> Option<ObserverState> {
        if let Some(obs) = self.observers.remove(&session_id) {
            self.guid_to_session.remove(&obs.player_guid);
            Some(obs)
        } else {
            None
        }
    }

    /// Unregisters an observer by player GUID.
    pub fn unregister_by_guid(&mut self, player_guid: Guid) -> Option<ObserverState> {
        if let Some(session_id) = self.guid_to_session.remove(&player_guid) {
            self.observers.remove(&session_id)
        } else {
            None
        }
    }

    /// Retrieves an observer by session ID.
    pub fn get_observer(&self, session_id: u64) -> Option<&ObserverState> {
        self.observers.get(&session_id)
    }

    /// Retrieves an observer by player GUID.
    pub fn get_observer_by_guid(&self, player_guid: Guid) -> Option<&ObserverState> {
        let session_id = self.guid_to_session.get(&player_guid)?;
        self.observers.get(session_id)
    }

    /// Retrieves the session ID mapped to a player GUID.
    pub fn get_session_by_guid(&self, player_guid: Guid) -> Option<u64> {
        self.guid_to_session.get(&player_guid).copied()
    }

    /// Returns the number of registered observers.
    pub fn observer_count(&self) -> usize {
        self.observers.len()
    }

    /// Processes an incoming client ACK for an observer session, advancing the confirmed baseline.
    ///
    /// Validates `observed_tick <= current_tick` to protect against future-tick spoofing DoS attacks.
    pub fn process_ack(
        &mut self,
        session_id: u64,
        observed_tick: u64,
        ack_mask: u32,
        current_tick: u64,
    ) -> bool {
        if let Some(obs) = self.observers.get_mut(&session_id) {
            obs.baseline_tracker
                .process_ack(observed_tick, ack_mask, current_tick)
        } else {
            false
        }
    }

    /// Evaluates AoI visibility for all observers and generates broadcast jobs.
    pub fn update_observers(
        &mut self,
        entity_store: &EntityStore,
        octree: &Octree,
        current_tick: u64,
        timestamp_ms: u64,
    ) -> Vec<AoiBroadcastJob> {
        let mut jobs = Vec::new();

        self.scratch_session_ids.clear();
        self.scratch_session_ids
            .extend(self.observers.keys().copied());

        for i in 0..self.scratch_session_ids.len() {
            let session_id = self.scratch_session_ids[i];
            let observer = match self.observers.get_mut(&session_id) {
                Some(obs) => obs,
                None => continue,
            };

            // 1. Get observer position
            let observer_pos = match entity_store.get_by_guid(observer.player_guid) {
                Some(ent) => ent.position,
                None => continue,
            };

            // 2. Perform zero-allocation query into scratch buffer
            self.scratch_query.clear();
            octree.query_radius_with_dist_sq_into(
                observer_pos,
                observer.effective_radius,
                &mut self.scratch_query,
            );

            // Filter out self if include_self is false
            if !observer.config.include_self {
                self.scratch_query
                    .retain(|&(g, _, _)| g != observer.player_guid);
            }

            // 3. Dynamic density throttling
            let total_found = self.scratch_query.len();
            let max_entities = observer.config.max_entities;
            if total_found > max_entities {
                // select_nth_unstable_by guarantees elements before `max_entities` are <= element at `max_entities`
                // in O(N) average time rather than O(N log N) full sort
                self.scratch_query
                    .select_nth_unstable_by(max_entities, |a, b| {
                        a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal)
                    });
                self.scratch_query.truncate(max_entities);

                // Calculate throttled radius
                let throttled =
                    calculate_throttled_aoi(observer.config.base_radius, total_found, max_entities);
                observer.effective_radius = throttled;
            } else if observer.effective_radius < observer.config.base_radius {
                // Restore base radius if density subsides
                observer.effective_radius = observer.config.base_radius;
            }

            // 4. Populate current visible set
            self.scratch_visible_guids.clear();
            for &(g, _, _) in &self.scratch_query {
                self.scratch_visible_guids.insert(g);
            }

            let mut packets = Vec::new();

            // 5. Detect exits (was in observed_entities, but not in current visible set)
            self.scratch_exited_guids.clear();
            for &observed_guid in observer.observed_entities.keys() {
                if !self.scratch_visible_guids.contains(&observed_guid) {
                    self.scratch_exited_guids.push(observed_guid);
                }
            }

            if observer.config.enable_delta_compression {
                // === DELTA COMPRESSION MODE ===
                let mut despawns = Vec::new();
                for &exited_guid in &self.scratch_exited_guids {
                    observer.observed_entities.remove(&exited_guid);
                    despawns.push(exited_guid.raw());
                }

                let baseline_tick_opt = observer.baseline_tracker.confirmed_baseline_tick;
                let is_baseline_expired = match baseline_tick_opt {
                    Some(t) => current_tick.saturating_sub(t) > 40, // 2s baseline timeout -> fallback
                    None => true,
                };
                let effective_baseline_tick = if is_baseline_expired {
                    0
                } else {
                    baseline_tick_opt.unwrap_or(0)
                };

                let baseline_snapshot = if effective_baseline_tick > 0 {
                    observer
                        .baseline_tracker
                        .get_snapshot(effective_baseline_tick)
                } else {
                    None
                };

                let mut batch_deltas = Vec::new();

                for &(visible_guid, pos, _) in &self.scratch_query {
                    let ent = match entity_store.get_by_guid(visible_guid) {
                        Some(e) => e,
                        None => continue,
                    };

                    let (health, max_health, state_flags) = ent
                        .components
                        .get_component::<HealthComponent>()
                        .map(|h| {
                            (
                                h.current_hp as u32,
                                h.max_hp as u32,
                                if h.is_dead { 1 } else { 0 },
                            )
                        })
                        .unwrap_or((100, 100, 0));

                    let yaw = ent.rotation;
                    let pitch = 0.0;
                    let velocity = (0.0, 0.0, 0.0);

                    let mut delta_entry = None;

                    if let Some(base_state) =
                        baseline_snapshot.and_then(|s| s.states.get(&visible_guid))
                    {
                        let dx = pos.0 - base_state.position.0;
                        let dy = pos.1 - base_state.position.1;
                        let dz = pos.2 - base_state.position.2;
                        let is_teleport = is_teleport_delta(dx, dy, dz);
                        let periodic_refresh = current_tick
                            .saturating_sub(base_state.last_sent_tick)
                            >= observer.config.full_refresh_interval_ticks;

                        if is_teleport || periodic_refresh {
                            let mut field_mask = delta_flags::MASK_KEYFRAME;
                            if velocity != (0.0, 0.0, 0.0) {
                                field_mask |= delta_flags::MASK_VELOCITY;
                            }
                            field_mask |= delta_flags::MASK_YAW;
                            field_mask |= delta_flags::MASK_HEALTH
                                | delta_flags::MASK_MAX_HEALTH
                                | delta_flags::MASK_STATE_FLAGS;

                            delta_entry = Some(EntityDeltaState {
                                entity_id: visible_guid.raw(),
                                field_mask,
                                position: pos,
                                velocity,
                                yaw,
                                pitch,
                                health,
                                max_health,
                                state_flags,
                                action_id: 0,
                            });
                        } else {
                            let mut field_mask = 0u16;
                            let dist_sq = dx * dx + dy * dy + dz * dz;
                            if dist_sq >= observer.config.position_epsilon_sq {
                                if dx.abs() >= 0.0039 {
                                    field_mask |= delta_flags::MASK_DELTA_POS_X;
                                }
                                if dy.abs() >= 0.0039 {
                                    field_mask |= delta_flags::MASK_DELTA_POS_Y;
                                }
                                if dz.abs() >= 0.0039 {
                                    field_mask |= delta_flags::MASK_DELTA_POS_Z;
                                }
                            }
                            if velocity != base_state.velocity {
                                field_mask |= delta_flags::MASK_VELOCITY;
                            }
                            let yaw_diff = (yaw.rem_euclid(2.0 * std::f32::consts::PI)
                                - base_state.yaw.rem_euclid(2.0 * std::f32::consts::PI))
                            .abs();
                            if yaw_diff > 0.02 {
                                field_mask |= delta_flags::MASK_YAW;
                            }
                            if health != base_state.health {
                                field_mask |= delta_flags::MASK_HEALTH;
                            }
                            if max_health != base_state.max_health {
                                field_mask |= delta_flags::MASK_MAX_HEALTH;
                            }
                            if state_flags != base_state.state_flags {
                                field_mask |= delta_flags::MASK_STATE_FLAGS;
                            }

                            if field_mask != 0 {
                                delta_entry = Some(EntityDeltaState {
                                    entity_id: visible_guid.raw(),
                                    field_mask,
                                    position: (dx, dy, dz),
                                    velocity,
                                    yaw,
                                    pitch,
                                    health,
                                    max_health,
                                    state_flags,
                                    action_id: 0,
                                });
                            }
                        }
                    } else {
                        // New entrant or unacknowledged baseline: full keyframe
                        let mut field_mask = delta_flags::MASK_KEYFRAME;
                        if velocity != (0.0, 0.0, 0.0) {
                            field_mask |= delta_flags::MASK_VELOCITY;
                        }
                        field_mask |= delta_flags::MASK_YAW;
                        field_mask |= delta_flags::MASK_HEALTH
                            | delta_flags::MASK_MAX_HEALTH
                            | delta_flags::MASK_STATE_FLAGS;

                        delta_entry = Some(EntityDeltaState {
                            entity_id: visible_guid.raw(),
                            field_mask,
                            position: pos,
                            velocity,
                            yaw,
                            pitch,
                            health,
                            max_health,
                            state_flags,
                            action_id: 0,
                        });
                    }

                    if let Some(entry) = delta_entry {
                        batch_deltas.push(entry);
                        observer.observed_entities.insert(
                            visible_guid,
                            CachedEntityState {
                                position: pos,
                                velocity,
                                yaw,
                                health,
                                max_health,
                                state_flags,
                                last_sent_tick: current_tick,
                            },
                        );
                    }
                }

                if !batch_deltas.is_empty() || !despawns.is_empty() {
                    packets.push(ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
                        server_tick: current_tick,
                        baseline_tick: effective_baseline_tick,
                        despawns,
                        deltas: batch_deltas,
                    }));
                }

                // Record tick snapshot into history buffer
                observer
                    .baseline_tracker
                    .record_snapshot(current_tick, observer.observed_entities.clone());
            } else {
                // === LEGACY UNCOMPRESSED MODE ===
                for &exited_guid in &self.scratch_exited_guids {
                    observer.observed_entities.remove(&exited_guid);
                    packets.push(ClientPacket::EntityDespawn(EntityDespawnPacket {
                        entity_id: exited_guid.raw(),
                    }));
                }

                for &(visible_guid, pos, _) in &self.scratch_query {
                    let ent = match entity_store.get_by_guid(visible_guid) {
                        Some(e) => e,
                        None => continue,
                    };

                    let (health, max_health, state_flags) = ent
                        .components
                        .get_component::<HealthComponent>()
                        .map(|h| {
                            (
                                h.current_hp as u32,
                                h.max_hp as u32,
                                if h.is_dead { 1 } else { 0 },
                            )
                        })
                        .unwrap_or((100, 100, 0));

                    let yaw = ent.rotation;
                    let pitch = 0.0;
                    let velocity = (0.0, 0.0, 0.0);

                    if let Some(cached) = observer.observed_entities.get_mut(&visible_guid) {
                        let dx = pos.0 - cached.position.0;
                        let dy = pos.1 - cached.position.1;
                        let dz = pos.2 - cached.position.2;
                        let dist_sq = dx * dx + dy * dy + dz * dz;

                        let pos_changed = dist_sq >= observer.config.position_epsilon_sq;
                        let health_changed =
                            cached.health != health || cached.max_health != max_health;
                        let flags_changed = cached.state_flags != state_flags;
                        let periodic_refresh = current_tick.saturating_sub(cached.last_sent_tick)
                            >= observer.config.full_refresh_interval_ticks;

                        if pos_changed || health_changed || flags_changed || periodic_refresh {
                            cached.position = pos;
                            cached.velocity = velocity;
                            cached.yaw = yaw;
                            cached.health = health;
                            cached.max_health = max_health;
                            cached.state_flags = state_flags;
                            cached.last_sent_tick = current_tick;

                            packets.push(ClientPacket::EntityState(EntityStatePacket {
                                entity_id: visible_guid.raw(),
                                position: Vec3::new(pos.0, pos.1, pos.2),
                                velocity: Vec3::new(velocity.0, velocity.1, velocity.2),
                                yaw,
                                pitch,
                                health,
                                max_health,
                                state_flags,
                                timestamp: timestamp_ms,
                            }));
                        }
                    } else {
                        observer.observed_entities.insert(
                            visible_guid,
                            CachedEntityState {
                                position: pos,
                                velocity,
                                yaw,
                                health,
                                max_health,
                                state_flags,
                                last_sent_tick: current_tick,
                            },
                        );

                        packets.push(ClientPacket::EntityState(EntityStatePacket {
                            entity_id: visible_guid.raw(),
                            position: Vec3::new(pos.0, pos.1, pos.2),
                            velocity: Vec3::new(velocity.0, velocity.1, velocity.2),
                            yaw,
                            pitch,
                            health,
                            max_health,
                            state_flags,
                            timestamp: timestamp_ms,
                        }));
                    }
                }
            }

            if !packets.is_empty() {
                jobs.push(AoiBroadcastJob {
                    session_id,
                    player_guid: observer.player_guid,
                    packets,
                });
            }
        }

        jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::BoundingBox3D;

    fn make_guid(id: u64) -> Guid {
        Guid::from_raw(id)
    }

    #[test]
    fn test_observer_registration_and_lookup() {
        let mut aoi = AoiManager::new();
        let guid = make_guid(100);
        let config = ObserverConfig {
            base_radius: 30.0,
            max_entities: 50,
            ..Default::default()
        };

        aoi.register_observer(1, guid, config.clone());
        assert_eq!(aoi.observer_count(), 1);

        let obs = aoi.get_observer(1).expect("observer by session");
        assert_eq!(obs.player_guid, guid);
        assert_eq!(obs.effective_radius, 30.0);

        let obs_by_g = aoi.get_observer_by_guid(guid).expect("observer by guid");
        assert_eq!(obs_by_g.session_id, 1);

        let removed = aoi.unregister_observer(1).expect("unregister");
        assert_eq!(removed.player_guid, guid);
        assert_eq!(aoi.observer_count(), 0);
        assert!(aoi.get_observer(1).is_none());
        assert!(aoi.get_observer_by_guid(guid).is_none());
    }

    #[test]
    fn test_aoi_enter_and_despawn_lifecycle() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);

        let player = make_guid(1);
        let npc = make_guid(2);

        store.spawn(player, (10.0, 10.0, 10.0));
        octree.insert(player, (10.0, 10.0, 10.0));

        let config = ObserverConfig {
            base_radius: 20.0,
            ..Default::default()
        };
        aoi.register_observer(10, player, config);

        // Tick 1: Only player in world, no other entities -> no broadcast jobs
        let jobs = aoi.update_observers(&store, &octree, 1, 1000);
        assert!(jobs.is_empty());

        // Spawn NPC within 20m of player at (15, 10, 10)
        store.spawn(npc, (15.0, 10.0, 10.0));
        octree.insert(npc, (15.0, 10.0, 10.0));

        // Tick 2: NPC enters player's AoI -> EntityState packet
        let jobs = aoi.update_observers(&store, &octree, 2, 1050);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].session_id, 10);
        assert_eq!(jobs[0].packets.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::EntityState(state) => {
                assert_eq!(state.entity_id, npc.raw());
                assert_eq!(state.position.x, 15.0);
            }
            other => panic!("Expected EntityState, got {:?}", other),
        }

        // Tick 3: NPC stationary -> No delta packets
        let jobs = aoi.update_observers(&store, &octree, 3, 1100);
        assert!(jobs.is_empty());

        // Despawn NPC from octree & store
        octree.remove(npc, (15.0, 10.0, 10.0));
        store.despawn_by_guid(npc);

        // Tick 4: NPC missing from AoI -> EntityDespawn packet
        let jobs = aoi.update_observers(&store, &octree, 4, 1150);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].packets.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::EntityDespawn(despawn) => {
                assert_eq!(despawn.entity_id, npc.raw());
            }
            other => panic!("Expected EntityDespawn, got {:?}", other),
        }
    }

    #[test]
    fn test_aoi_movement_delta_and_periodic_refresh() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);

        let player = make_guid(1);
        let target = make_guid(2);

        store.spawn(player, (10.0, 10.0, 10.0));
        octree.insert(player, (10.0, 10.0, 10.0));

        store.spawn(target, (12.0, 10.0, 10.0));
        octree.insert(target, (12.0, 10.0, 10.0));

        let config = ObserverConfig {
            base_radius: 20.0,
            full_refresh_interval_ticks: 5,
            ..Default::default()
        };
        aoi.register_observer(1, player, config);

        // Initial enter
        let jobs = aoi.update_observers(&store, &octree, 1, 1000);
        assert_eq!(jobs.len(), 1);

        // Target moves slightly (within epsilon: 0.001m < 0.01m)
        let ent = store.get_mut_by_guid(target).unwrap();
        ent.position = (12.001, 10.0, 10.0);
        // Not enough delta to trigger update
        let jobs = aoi.update_observers(&store, &octree, 2, 1050);
        assert!(jobs.is_empty());

        // Target moves significantly (0.5m)
        let ent = store.get_mut_by_guid(target).unwrap();
        ent.position = (12.5, 10.0, 10.0);
        octree.remove(target, (12.0, 10.0, 10.0));
        octree.insert(target, (12.5, 10.0, 10.0));

        let jobs = aoi.update_observers(&store, &octree, 3, 1100);
        assert_eq!(jobs.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::EntityState(state) => {
                assert_eq!(state.position.x, 12.5);
            }
            other => panic!("Expected EntityState, got {:?}", other),
        }

        // Ticks 4 and 5 stationary
        assert!(aoi.update_observers(&store, &octree, 4, 1150).is_empty());
        assert!(aoi.update_observers(&store, &octree, 5, 1200).is_empty());

        // Tick 8 (>= tick 3 + 5 refresh interval) -> triggers periodic refresh
        let jobs = aoi.update_observers(&store, &octree, 8, 1350);
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn test_aoi_density_throttling_sort_and_clamp() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);

        let player = make_guid(1);
        store.spawn(player, (50.0, 50.0, 50.0));
        octree.insert(player, (50.0, 50.0, 50.0));

        // Max entities is 5
        let config = ObserverConfig {
            base_radius: 40.0,
            max_entities: 5,
            ..Default::default()
        };
        aoi.register_observer(1, player, config);

        // Spawn 10 entities at varying distances from player
        for i in 2..=11 {
            let guid = make_guid(i);
            let dist = (i - 1) as f32 * 2.0; // 2, 4, 6, 8, 10, 12, 14, 16, 18, 20
            store.spawn(guid, (50.0 + dist, 50.0, 50.0));
            octree.insert(guid, (50.0 + dist, 50.0, 50.0));
        }

        // Update
        let jobs = aoi.update_observers(&store, &octree, 1, 1000);
        assert_eq!(jobs.len(), 1);
        // Observer should have clamped to max_entities (5)
        assert_eq!(jobs[0].packets.len(), 5);

        let obs = aoi.get_observer(1).unwrap();
        // Effective radius throttled below 40.0
        assert!(obs.effective_radius < 40.0);
        assert_eq!(obs.observed_entities.len(), 5);

        // Verify that the 5 closest entities (IDs 2, 3, 4, 5, 6) were the ones observed
        for i in 2..=6 {
            assert!(obs.observed_entities.contains_key(&make_guid(i)));
        }
    }

    #[test]
    fn test_baseline_tracker_ack_advancement_and_pruning() {
        let mut tracker = BaselineTracker::new(10);
        let mut map = HashMap::new();
        map.insert(
            make_guid(1),
            CachedEntityState {
                position: (1.0, 2.0, 3.0),
                velocity: (0.0, 0.0, 0.0),
                yaw: 0.0,
                health: 100,
                max_health: 100,
                state_flags: 0,
                last_sent_tick: 1,
            },
        );

        for tick in 1..=5 {
            tracker.record_snapshot(tick, map.clone());
        }
        assert_eq!(tracker.history.len(), 5);

        // ACK tick 3
        let acked = tracker.process_ack(3, 0, 5);
        assert!(acked);
        assert_eq!(tracker.confirmed_baseline_tick, Some(3));
        // Ticks 1 and 2 pruned, 3, 4, 5 remain
        assert_eq!(tracker.history.len(), 3);
        assert_eq!(tracker.history.front().unwrap().tick, 3);
    }

    #[test]
    fn test_baseline_tracker_anti_cheat_future_tick_rejection() {
        let mut tracker = BaselineTracker::new(10);
        tracker.record_snapshot(1, HashMap::new());
        tracker.record_snapshot(2, HashMap::new());

        // Attempt future tick spoofing (tick 100 > current tick 5)
        let acked = tracker.process_ack(100, 0, 5);
        assert!(!acked); // Rejected!
        assert_eq!(tracker.confirmed_baseline_tick, None);
        assert_eq!(tracker.history.len(), 2); // History preserved!
    }

    #[test]
    fn test_delta_compression_enter_delta_and_teleport() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (1000.0, 1000.0, 1000.0));
        let mut octree = Octree::new(bounds);

        let player = make_guid(1);
        let npc = make_guid(2);

        store.spawn(player, (100.0, 100.0, 100.0));
        octree.insert(player, (100.0, 100.0, 100.0));

        let config = ObserverConfig {
            base_radius: 50.0,
            enable_delta_compression: true,
            ..Default::default()
        };
        aoi.register_observer(10, player, config);

        // Spawn NPC at (105, 100, 100)
        store.spawn(npc, (105.0, 100.0, 100.0));
        octree.insert(npc, (105.0, 100.0, 100.0));

        // Tick 1: NPC enters -> keyframe inside EntityDeltaBatchPacket
        let jobs = aoi.update_observers(&store, &octree, 1, 1000);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].packets.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::EntityDeltaBatch(batch) => {
                assert_eq!(batch.server_tick, 1);
                assert_eq!(batch.baseline_tick, 0); // initial keyframe
                assert_eq!(batch.deltas.len(), 1);
                let d = &batch.deltas[0];
                assert_eq!(d.entity_id, npc.raw());
                assert_ne!(d.field_mask & delta_flags::MASK_KEYFRAME, 0);
                assert_eq!(d.position, (105.0, 100.0, 100.0));
            }
            other => panic!("Expected EntityDeltaBatch, got {:?}", other),
        }

        // Client ACKs tick 1
        assert!(aoi.process_ack(10, 1, 0, 1));

        // Move NPC slightly (+0.5m on X)
        let ent = store.get_mut_by_guid(npc).unwrap();
        ent.position = (105.5, 100.0, 100.0);
        octree.remove(npc, (105.0, 100.0, 100.0));
        octree.insert(npc, (105.5, 100.0, 100.0));

        // Tick 2: NPC delta update
        let jobs = aoi.update_observers(&store, &octree, 2, 1050);
        assert_eq!(jobs.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::EntityDeltaBatch(batch) => {
                assert_eq!(batch.server_tick, 2);
                assert_eq!(batch.baseline_tick, 1);
                assert_eq!(batch.deltas.len(), 1);
                let d = &batch.deltas[0];
                assert_eq!(d.entity_id, npc.raw());
                assert_eq!(d.field_mask & delta_flags::MASK_KEYFRAME, 0); // Not a keyframe!
                assert_ne!(d.field_mask & delta_flags::MASK_DELTA_POS_X, 0);
                assert!((d.position.0 - 0.5).abs() < 0.008);
            }
            other => panic!("Expected EntityDeltaBatch, got {:?}", other),
        }

        // Client ACKs tick 2
        assert!(aoi.process_ack(10, 2, 0, 2));

        // Teleport NPC by 300m (> 255.0m max delta)
        let ent = store.get_mut_by_guid(npc).unwrap();
        ent.position = (105.5 + 300.0, 100.0, 100.0);
        let player_ent = store.get_mut_by_guid(player).unwrap();
        player_ent.position = (400.0, 100.0, 100.0); // Move player along so NPC remains in 50m radius
        octree.remove(npc, (105.5, 100.0, 100.0));
        octree.insert(npc, (405.5, 100.0, 100.0));

        // Tick 3: Teleport should elevate to MASK_KEYFRAME
        let jobs = aoi.update_observers(&store, &octree, 3, 1100);
        assert_eq!(jobs.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::EntityDeltaBatch(batch) => {
                assert_eq!(batch.server_tick, 3);
                assert_eq!(batch.deltas.len(), 1);
                let d = &batch.deltas[0];
                assert_eq!(d.entity_id, npc.raw());
                assert_ne!(d.field_mask & delta_flags::MASK_KEYFRAME, 0); // Forced keyframe on teleport!
                assert_eq!(d.position, (405.5, 100.0, 100.0));
            }
            other => panic!("Expected EntityDeltaBatch, got {:?}", other),
        }
    }
}
