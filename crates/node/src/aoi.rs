//! Area-of-Interest (AoI) observation and delta broadcasting system.
//!
//! Tracks player observation spheres in the Sparse Octree, computes dynamic
//! entity visibility deltas (`entered`, `updated`, `exited`), performs
//! dynamic density-based radius throttling, and produces binary broadcast packets.

use std::collections::{HashMap, HashSet, VecDeque};

use mmo_core::guid::Guid;
use mmo_core::octree::calculate_throttled_aoi;
use mmo_core::world::SectorCoord3D;
use mmo_core::{EntityStore, Octree};
use mmo_gameplay::combat::HealthComponent;
use mmo_schema::client::{
    delta_flags, ClientPacket, EntityDeltaBatchPacket, EntityDeltaState, EntityDespawnPacket,
    EntityStatePacket, ScalarAttributeDelta, Vec3, WorldObjectDeltaPacket,
    WorldObjectDespawnPacket, WorldObjectSpawnPacket,
};
use mmo_schema::quantize::is_teleport_delta;

use crate::sector::SectorManager;

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
    /// Whether to enable directional view frustum culling. Default is false.
    pub enable_frustum_culling: bool,
    /// Horizontal field of view in degrees. Default is 90.0 degrees.
    pub fov_degrees: f32,
    /// Immediate awareness sphere radius in meters. Entities within this 3D radius
    /// are always visible regardless of facing direction. Default is 5.0m.
    pub near_clip_radius: f32,
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
            enable_frustum_culling: false,
            fov_degrees: 90.0,
            near_clip_radius: 5.0,
        }
    }
}

/// Evaluates whether `target_pos` lies within the observer's view frustum or 3D awareness sphere.
///
/// Returns `true` if:
/// 1. The 3D distance between `observer_pos` and `target_pos` is <= `near_clip_radius` (awareness bubble).
/// 2. OR the entity lies within `fov_degrees` of the observer's forward heading vector `(-sin(yaw), 0, cos(yaw))`.
pub fn is_in_view_frustum(
    observer_pos: (f32, f32, f32),
    observer_yaw: f32,
    target_pos: (f32, f32, f32),
    fov_degrees: f32,
    near_clip_radius: f32,
) -> bool {
    let dx = target_pos.0 - observer_pos.0;
    let dy = target_pos.1 - observer_pos.1;
    let dz = target_pos.2 - observer_pos.2;

    let dist_3d_sq = dx * dx + dy * dy + dz * dz;
    let near_clip_sq = near_clip_radius * near_clip_radius;
    if dist_3d_sq <= near_clip_sq {
        return true;
    }

    let dist_horiz_sq = dx * dx + dz * dz;
    if dist_horiz_sq <= 1e-6 {
        // Vertically above/below observer outside near clip radius -> culled
        return false;
    }

    let dist_horiz = dist_horiz_sq.sqrt();
    let fwd_x = -observer_yaw.sin();
    let fwd_z = observer_yaw.cos();

    // Dot product between normalized horizontal direction and forward vector
    let cos_angle = (dx * fwd_x + dz * fwd_z) / dist_horiz;
    let half_fov_rad = (fov_degrees * 0.5).to_radians();
    let min_cos = half_fov_rad.cos();

    cos_angle >= min_cos
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
    pub attributes: Vec<ScalarAttributeDelta>,
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

/// Maps a container owner to its compact wire discriminant.
fn owner_type_code(owner: &mmo_core::container::ContainerOwner) -> u8 {
    use mmo_core::container::ContainerOwner;
    match owner {
        ContainerOwner::Character(_) => 0,
        ContainerOwner::Guild(_) => 1,
        ContainerOwner::WorldEntity(_) => 2,
        ContainerOwner::System(_) => 3,
    }
}

/// Builds a full-state spawn packet for a world object.
pub(crate) fn build_spawn_packet(obj: &mmo_core::world::WorldObject) -> WorldObjectSpawnPacket {
    WorldObjectSpawnPacket {
        object_id: obj.object_id.0,
        prefab_id: obj.prefab_id,
        pos: Vec3 {
            x: obj.transform.position.0,
            y: obj.transform.position.1,
            z: obj.transform.position.2,
        },
        rot: obj.transform.rotation,
        scale: Vec3 {
            x: obj.transform.scale.0,
            y: obj.transform.scale.1,
            z: obj.transform.scale.2,
        },
        bounds_min: Vec3 {
            x: obj.bounds.min.0,
            y: obj.bounds.min.1,
            z: obj.bounds.min.2,
        },
        bounds_max: Vec3 {
            x: obj.bounds.max.0,
            y: obj.bounds.max.1,
            z: obj.bounds.max.2,
        },
        state_flags: obj.state_flags,
        owner_type: owner_type_code(&obj.owner),
        owner_id: obj.owner.owner_id(),
        health: obj.health.map(|h| (h.current, h.max)),
        properties: obj.properties.clone(),
        revision: obj.dirty_revision,
    }
}

/// Maximum number of queued world objects streamed to an observer per simulation tick.
pub const MAX_PACED_WORLD_OBJECTS_PER_TICK: usize = 10;

/// State tracking for an active player session observer.
#[derive(Debug, Clone)]
pub struct ObserverState {
    pub session_id: u64,
    pub player_guid: Guid,
    pub config: ObserverConfig,
    pub effective_radius: f32,
    pub observed_entities: HashMap<Guid, CachedEntityState>,
    pub baseline_tracker: BaselineTracker,
    pub observed_sectors: HashSet<SectorCoord3D>,
    pub known_world_objects: HashMap<Guid, u64>,
    pub pending_world_objects_egress: VecDeque<(Guid, SectorCoord3D, f32)>,
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
            observed_sectors: HashSet::new(),
            known_world_objects: HashMap::new(),
            pending_world_objects_egress: VecDeque::new(),
        }
    }

    /// Prunes all pending egress world objects associated with the given sector.
    pub fn prune_sector_from_egress(&mut self, sector: SectorCoord3D) {
        self.pending_world_objects_egress
            .retain(|(_, s, _)| *s != sector);
    }
}

/// Computes all discrete 3D spatial sectors whose bounding boxes intersect the observer's sphere.
pub fn compute_visible_sectors(center: (f32, f32, f32), radius: f32) -> HashSet<SectorCoord3D> {
    if radius <= 0.0 {
        return HashSet::new();
    }
    let min_pos = (center.0 - radius, center.1 - radius, center.2 - radius);
    let max_pos = (center.0 + radius, center.1 + radius, center.2 + radius);

    let min_sec = SectorCoord3D::from_world_pos(min_pos);
    let max_sec = SectorCoord3D::from_world_pos(max_pos);

    let mut visible = HashSet::new();
    for sx in min_sec.x..=max_sec.x {
        for sy in min_sec.y..=max_sec.y {
            for sz in min_sec.z..=max_sec.z {
                let sector = SectorCoord3D::new(sx, sy, sz);
                if sector.bounds().intersects_sphere(center, radius) {
                    visible.insert(sector);
                }
            }
        }
    }
    visible
}

/// Extracts generic scalar attributes and state flags for an entity.
///
/// Follows the engine's unopinionated prime directive: reads generic properties without hardcoding
/// specific genre gameplay logic into spatial replication.
fn extract_entity_attributes(ent: &mmo_core::Entity) -> (u32, u32, u32, Vec<ScalarAttributeDelta>) {
    let mut attrs = Vec::new();

    // Check generic dynamic properties first
    let mut health = ent
        .properties
        .get("health")
        .and_then(|v| v.as_i64())
        .map(|v| v as u32);
    let mut max_health = ent
        .properties
        .get("max_health")
        .and_then(|v| v.as_i64())
        .map(|v| v as u32);
    let state_flags = ent
        .properties
        .get("state_flags")
        .and_then(|v| v.as_i64())
        .map(|v| v as u32)
        .unwrap_or(0);

    // Fallback to combat HealthComponent if attached to entity
    if health.is_none() || max_health.is_none() {
        if let Some(h) = ent.components.get_component::<HealthComponent>() {
            health = Some(h.current_hp as u32);
            max_health = Some(h.max_hp as u32);
        }
    }

    let h_val = health.unwrap_or(100);
    let mh_val = max_health.unwrap_or(100);

    // Generic scalar attribute mapping: 1 for health, 2 for max_health
    attrs.push(ScalarAttributeDelta {
        attribute_id: 1,
        value: h_val,
    });
    attrs.push(ScalarAttributeDelta {
        attribute_id: 2,
        value: mh_val,
    });

    (h_val, mh_val, state_flags, attrs)
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

    /// Registers a new player session as an AoI observer, evicting any previous session for this player.
    pub fn register_observer(
        &mut self,
        session_id: u64,
        player_guid: Guid,
        config: ObserverConfig,
    ) {
        if let Some(old_session_id) = self.guid_to_session.get(&player_guid).copied() {
            if old_session_id != session_id {
                self.observers.remove(&old_session_id);
            }
        }
        if let Some(old_obs) = self.observers.remove(&session_id) {
            if old_obs.player_guid != player_guid {
                self.guid_to_session.remove(&old_obs.player_guid);
            }
        }
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

            // 1. Get observer position and orientation
            let (observer_pos, observer_yaw) = match entity_store.get_by_guid(observer.player_guid)
            {
                Some(ent) => (ent.position, ent.rotation),
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

            // 2.5 Frustum Culling (if enabled)
            if observer.config.enable_frustum_culling {
                let fov = observer.config.fov_degrees;
                let near_r = observer.config.near_clip_radius;
                self.scratch_query.retain(|&(_, pos, _)| {
                    is_in_view_frustum(observer_pos, observer_yaw, pos, fov, near_r)
                });
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

                    let (health, max_health, state_flags, attrs) = extract_entity_attributes(ent);

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
                            if !attrs.is_empty() {
                                field_mask |= delta_flags::MASK_ATTRIBUTES;
                            }

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
                                attributes: attrs.clone(),
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
                            let diff =
                                (yaw - base_state.yaw).rem_euclid(2.0 * std::f32::consts::PI);
                            let shortest_yaw_diff = diff.min(2.0 * std::f32::consts::PI - diff);
                            if shortest_yaw_diff > 0.02 {
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
                            if attrs != base_state.attributes {
                                field_mask |= delta_flags::MASK_ATTRIBUTES;
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
                                    attributes: if field_mask & delta_flags::MASK_ATTRIBUTES != 0 {
                                        attrs.clone()
                                    } else {
                                        Vec::new()
                                    },
                                });
                            }
                        }
                    } else {
                        // Unacknowledged baseline or new entrant. Rate-limit keyframe fallback to avoid network death spiral!
                        let last_sent_opt = observer.observed_entities.get(&visible_guid);
                        let is_new_entrant = last_sent_opt.is_none();
                        let should_emit_keyframe = if is_new_entrant {
                            true
                        } else {
                            let last_sent = last_sent_opt.unwrap();
                            let dx = pos.0 - last_sent.position.0;
                            let dy = pos.1 - last_sent.position.1;
                            let dz = pos.2 - last_sent.position.2;
                            let dist_sq = dx * dx + dy * dy + dz * dz;
                            let moved = dist_sq >= observer.config.position_epsilon_sq;
                            let attr_changed = health != last_sent.health
                                || max_health != last_sent.max_health
                                || state_flags != last_sent.state_flags
                                || attrs != last_sent.attributes;
                            let refresh_due = current_tick.saturating_sub(last_sent.last_sent_tick)
                                >= observer.config.full_refresh_interval_ticks;
                            moved || attr_changed || refresh_due
                        };

                        if should_emit_keyframe {
                            let mut field_mask = delta_flags::MASK_KEYFRAME;
                            if velocity != (0.0, 0.0, 0.0) {
                                field_mask |= delta_flags::MASK_VELOCITY;
                            }
                            field_mask |= delta_flags::MASK_YAW;
                            field_mask |= delta_flags::MASK_HEALTH
                                | delta_flags::MASK_MAX_HEALTH
                                | delta_flags::MASK_STATE_FLAGS;
                            if !attrs.is_empty() {
                                field_mask |= delta_flags::MASK_ATTRIBUTES;
                            }

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
                                attributes: attrs.clone(),
                            });
                        }
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
                                attributes: attrs,
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

                    let (health, max_health, state_flags, attrs) = extract_entity_attributes(ent);

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
                        let attrs_changed = cached.attributes != attrs;
                        let periodic_refresh = current_tick.saturating_sub(cached.last_sent_tick)
                            >= observer.config.full_refresh_interval_ticks;

                        if pos_changed
                            || health_changed
                            || flags_changed
                            || attrs_changed
                            || periodic_refresh
                        {
                            cached.position = pos;
                            cached.velocity = velocity;
                            cached.yaw = yaw;
                            cached.health = health;
                            cached.max_health = max_health;
                            cached.state_flags = state_flags;
                            cached.last_sent_tick = current_tick;
                            cached.attributes = attrs;

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
                                attributes: attrs,
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

    /// Evaluates spatial sector discovery, egress queue pruning, paced object streaming,
    /// and delta updates for all observers.
    pub fn update_world_object_observers(
        &mut self,
        entity_store: &EntityStore,
        sector_manager: &SectorManager,
    ) -> Vec<AoiBroadcastJob> {
        let mut jobs = Vec::new();

        self.scratch_session_ids.clear();
        self.scratch_session_ids
            .extend(self.observers.keys().copied());

        for session_id in &self.scratch_session_ids {
            let observer = match self.observers.get_mut(session_id) {
                Some(obs) => obs,
                None => continue,
            };

            let observer_pos = match entity_store.get_by_guid(observer.player_guid) {
                Some(ent) => ent.position,
                None => continue,
            };

            let current_visible_sectors =
                compute_visible_sectors(observer_pos, observer.effective_radius);
            let mut packets = Vec::new();

            // 1. Sector exits: prune egress queue and despawn objects
            let mut exited_sectors = Vec::new();
            for sector in &observer.observed_sectors {
                if !current_visible_sectors.contains(sector) {
                    exited_sectors.push(*sector);
                }
            }

            for exited_sector in exited_sectors {
                // Prune any unstreamed objects from this exited sector
                observer.prune_sector_from_egress(exited_sector);
                if let Some(sector_map) = sector_manager.query_sector(exited_sector) {
                    for &obj_guid in sector_map.keys() {
                        if observer.known_world_objects.remove(&obj_guid).is_some() {
                            packets.push(ClientPacket::WorldObjectDespawn(
                                WorldObjectDespawnPacket {
                                    object_id: obj_guid.0,
                                },
                            ));
                        }
                    }
                }
            }

            // 2. Sector entries: discover new sectors and enqueue resident objects
            for sector in &current_visible_sectors {
                if !observer.observed_sectors.contains(sector) {
                    if let Some(sector_map) = sector_manager.query_sector(*sector) {
                        for obj in sector_map.values() {
                            if !observer.known_world_objects.contains_key(&obj.object_id)
                                && !observer
                                    .pending_world_objects_egress
                                    .iter()
                                    .any(|(g, _, _)| *g == obj.object_id)
                            {
                                let dx = obj.transform.position.0 - observer_pos.0;
                                let dy = obj.transform.position.1 - observer_pos.1;
                                let dz = obj.transform.position.2 - observer_pos.2;
                                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                                observer.pending_world_objects_egress.push_back((
                                    obj.object_id,
                                    *sector,
                                    dist,
                                ));
                            }
                        }
                    }
                }
            }

            observer.observed_sectors = current_visible_sectors;

            // 3. Sort pending egress queue by distance ascending
            let mut egress_vec: Vec<(Guid, SectorCoord3D, f32)> =
                observer.pending_world_objects_egress.drain(..).collect();
            egress_vec.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

            // 4. Paced streaming: deliver up to MAX_PACED_WORLD_OBJECTS_PER_TICK
            let to_stream = egress_vec.len().min(MAX_PACED_WORLD_OBJECTS_PER_TICK);
            for (obj_id, _, _) in egress_vec.drain(..to_stream) {
                if let Some(obj) = sector_manager.get_object(obj_id) {
                    packets.push(ClientPacket::WorldObjectSpawn(build_spawn_packet(obj)));
                    observer
                        .known_world_objects
                        .insert(obj_id, obj.dirty_revision);
                }
            }
            observer.pending_world_objects_egress = egress_vec.into();

            // 5. Delta updates for active known objects
            let mut despawned_guids = Vec::new();
            for (&obj_id, last_rev) in observer.known_world_objects.iter_mut() {
                if let Some(obj) = sector_manager.get_object(obj_id) {
                    if obj.dirty_revision > *last_rev {
                        *last_rev = obj.dirty_revision;
                        packets.push(ClientPacket::WorldObjectDelta(WorldObjectDeltaPacket {
                            object_id: obj.object_id.0,
                            revision: obj.dirty_revision,
                            state_flags: Some(obj.state_flags),
                            health: obj.health.map(|h| (h.current, h.max)),
                            properties: obj.properties.clone(),
                        }));
                    }
                } else {
                    despawned_guids.push(obj_id);
                }
            }

            for despawned in despawned_guids {
                observer.known_world_objects.remove(&despawned);
                packets.push(ClientPacket::WorldObjectDespawn(WorldObjectDespawnPacket {
                    object_id: despawned.0,
                }));
            }

            if !packets.is_empty() {
                jobs.push(AoiBroadcastJob {
                    session_id: *session_id,
                    player_guid: observer.player_guid,
                    packets,
                });
            }
        }

        jobs
    }

    /// Handles a resync request for a specific world object, emitting a full spawn packet if found.
    pub fn handle_resync_request(
        &mut self,
        session_id: u64,
        object_id: u64,
        sector_manager: &SectorManager,
    ) -> Option<ClientPacket> {
        let observer = self.observers.get_mut(&session_id)?;
        let obj_guid = Guid(object_id);
        let obj = sector_manager.get_object(obj_guid)?;

        observer
            .known_world_objects
            .insert(obj_guid, obj.dirty_revision);
        Some(ClientPacket::WorldObjectSpawn(build_spawn_packet(obj)))
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
                attributes: Vec::new(),
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

    #[test]
    fn test_world_object_aoi_discovery_paced_streaming_and_pruning() {
        use mmo_core::container::ContainerOwner;
        use mmo_core::world::{Transform3D, WorldObject};

        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let mut sector_mgr = SectorManager::new();

        let player = make_guid(1);
        store.spawn(player, (0.0, 0.0, 0.0));

        let config = ObserverConfig {
            base_radius: 50.0,
            ..Default::default()
        };
        aoi.register_observer(100, player, config);

        // Place 15 objects in sector (0, 0, 0)
        for i in 1..=15 {
            let obj = WorldObject::new(
                Guid(i),
                100,
                Transform3D::new(
                    (i as f32 * 2.0, 0.0, 0.0),
                    (0.0, 0.0, 0.0, 1.0),
                    (1.0, 1.0, 1.0),
                ),
                BoundingBox3D::new(
                    (i as f32 * 2.0 - 0.5, -0.5, -0.5),
                    (i as f32 * 2.0 + 0.5, 0.5, 0.5),
                ),
                ContainerOwner::Character(1),
            );
            sector_mgr.insert_object(obj);
        }

        // Tick 1: Paced streaming should stream only first 10 objects (closest first)
        let jobs = aoi.update_world_object_observers(&store, &sector_mgr);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].packets.len(), 10);
        for (idx, pkt) in jobs[0].packets.iter().enumerate() {
            match pkt {
                ClientPacket::WorldObjectSpawn(spawn) => {
                    assert_eq!(spawn.object_id, (idx + 1) as u64);
                }
                other => panic!("Expected WorldObjectSpawn, got {:?}", other),
            }
        }

        // Tick 2: Streams remaining 5 objects
        let jobs2 = aoi.update_world_object_observers(&store, &sector_mgr);
        assert_eq!(jobs2.len(), 1);
        assert_eq!(jobs2[0].packets.len(), 5);
        for (idx, pkt) in jobs2[0].packets.iter().enumerate() {
            match pkt {
                ClientPacket::WorldObjectSpawn(spawn) => {
                    assert_eq!(spawn.object_id, (idx + 11) as u64);
                }
                other => panic!("Expected WorldObjectSpawn, got {:?}", other),
            }
        }

        // Tick 3: No new objects or changes -> 0 packets
        let jobs3 = aoi.update_world_object_observers(&store, &sector_mgr);
        assert_eq!(jobs3.len(), 0);

        // Mutate object 1 -> increments revision
        if let Some(obj1) = sector_mgr.get_object_mut(Guid(1)) {
            obj1.set_flag(mmo_core::world::world_object_flags::DAMAGED, true);
        }

        // Tick 4: Receives delta update for object 1
        let jobs4 = aoi.update_world_object_observers(&store, &sector_mgr);
        assert_eq!(jobs4.len(), 1);
        assert_eq!(jobs4[0].packets.len(), 1);
        match &jobs4[0].packets[0] {
            ClientPacket::WorldObjectDelta(delta) => {
                assert_eq!(delta.object_id, 1);
                assert_eq!(delta.revision, 2);
            }
            other => panic!("Expected WorldObjectDelta, got {:?}", other),
        }

        // Resync test
        let resync_pkt = aoi
            .handle_resync_request(100, 1, &sector_mgr)
            .expect("resync packet generated");
        match resync_pkt {
            ClientPacket::WorldObjectSpawn(spawn) => {
                assert_eq!(spawn.object_id, 1);
                assert_eq!(spawn.revision, 2);
            }
            other => panic!("Expected WorldObjectSpawn on resync, got {:?}", other),
        }

        // Player moves far away to (500, 500, 500) -> sector (0,0,0) exits AoI
        store.get_mut_by_guid(player).unwrap().position = (500.0, 500.0, 500.0);

        // Tick 5: Receives despawn packets for all 15 previously known objects
        let jobs5 = aoi.update_world_object_observers(&store, &sector_mgr);
        assert_eq!(jobs5.len(), 1);
        assert_eq!(jobs5[0].packets.len(), 15);
        for pkt in &jobs5[0].packets {
            assert!(matches!(pkt, ClientPacket::WorldObjectDespawn(_)));
        }
    }

    #[test]
    fn test_is_in_view_frustum_geometry() {
        let obs_pos = (0.0, 0.0, 0.0);
        let obs_yaw = 0.0; // facing +Z (fwd: (0, 0, 1))
        let fov = 90.0;
        let near_r = 5.0;

        // Directly ahead (+Z, 20m) -> true
        assert!(is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (0.0, 0.0, 20.0),
            fov,
            near_r
        ));

        // Directly behind (-Z, 20m, > near_r) -> false
        assert!(!is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (0.0, 0.0, -20.0),
            fov,
            near_r
        ));

        // Directly behind (-Z, 3m, <= near_r awareness bubble) -> true
        assert!(is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (0.0, 0.0, -3.0),
            fov,
            near_r
        ));

        // 100m straight up (Y=100m, > near_r 3D distance) -> false (prevents sky bypass)
        assert!(!is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (0.0, 100.0, 0.0),
            fov,
            near_r
        ));

        // 3m straight up (Y=3m, <= near_r 3D distance) -> true
        assert!(is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (0.0, 3.0, 0.0),
            fov,
            near_r
        ));

        // 30 degrees to the right (+X=10m, +Z=17.32m) -> true (within 45 deg half-FOV)
        assert!(is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (10.0, 0.0, 17.32),
            fov,
            near_r
        ));

        // 60 degrees to the right (+X=17.32m, +Z=10m) -> false (outside 45 deg half-FOV)
        assert!(!is_in_view_frustum(
            obs_pos,
            obs_yaw,
            (17.32, 0.0, 10.0),
            fov,
            near_r
        ));
    }

    #[test]
    fn test_aoi_manager_frustum_culling_integration() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));

        let observer_guid = make_guid(100);
        store.spawn(observer_guid, (0.0, 0.0, 0.0));
        store.get_mut_by_guid(observer_guid).unwrap().rotation = 0.0; // facing +Z
        octree.insert(observer_guid, (0.0, 0.0, 0.0));

        // Entity 1: in front (+Z=20m) -> should be visible
        let ent_front = make_guid(1);
        store.spawn(ent_front, (0.0, 0.0, 20.0));
        octree.insert(ent_front, (0.0, 0.0, 20.0));

        // Entity 2: behind (-Z=20m) -> should be culled
        let ent_behind = make_guid(2);
        store.spawn(ent_behind, (0.0, 0.0, -20.0));
        octree.insert(ent_behind, (0.0, 0.0, -20.0));

        // Entity 3: behind close (-Z=3m) -> in awareness bubble -> should be visible
        let ent_near = make_guid(3);
        store.spawn(ent_near, (0.0, 0.0, -3.0));
        octree.insert(ent_near, (0.0, 0.0, -3.0));

        let config = ObserverConfig {
            base_radius: 50.0,
            enable_frustum_culling: true,
            fov_degrees: 90.0,
            near_clip_radius: 5.0,
            ..Default::default()
        };

        aoi.register_observer(1, observer_guid, config);

        let jobs = aoi.update_observers(&store, &octree, 1, 1000);
        assert_eq!(jobs.len(), 1);

        let observed_ids: Vec<u64> = jobs[0]
            .packets
            .iter()
            .filter_map(|p| match p {
                ClientPacket::EntityState(s) => Some(s.entity_id),
                _ => None,
            })
            .collect();

        assert!(
            observed_ids.contains(&ent_front.raw()),
            "Front entity must be observed"
        );
        assert!(
            !observed_ids.contains(&ent_behind.raw()),
            "Distant entity behind must be culled"
        );
        assert!(
            observed_ids.contains(&ent_near.raw()),
            "Near entity behind must be observed via awareness bubble"
        );
    }

    #[test]
    fn test_yaw_wrap_around_delta_differencing() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));

        let obs_guid = make_guid(100);
        store.spawn(obs_guid, (0.0, 0.0, 0.0));
        octree.insert(obs_guid, (0.0, 0.0, 0.0));

        let target_guid = make_guid(200);
        store.spawn(target_guid, (0.0, 0.0, 10.0));
        store.get_mut_by_guid(target_guid).unwrap().rotation = 0.01; // nearly 0 rads
        octree.insert(target_guid, (0.0, 0.0, 10.0));

        let config = ObserverConfig {
            base_radius: 50.0,
            enable_delta_compression: true,
            ..Default::default()
        };
        aoi.register_observer(1, obs_guid, config);

        // Tick 1: keyframe
        aoi.update_observers(&store, &octree, 1, 1000);
        aoi.process_ack(1, 1, 0, 1);

        // Tick 2: rotation changes from 0.01 to near TAU (diff is wrapping to 0.013 <= 0.02)
        store.get_mut_by_guid(target_guid).unwrap().rotation = std::f32::consts::TAU - 0.003;
        let jobs2 = aoi.update_observers(&store, &octree, 2, 1050);

        // The angular difference across the 2PI boundary is only ~0.013 rads <= 0.02 threshold,
        // so NO yaw delta should be emitted!
        if !jobs2.is_empty() {
            for pkt in &jobs2[0].packets {
                if let ClientPacket::EntityDeltaBatch(batch) = pkt {
                    for delta in &batch.deltas {
                        if delta.entity_id == target_guid.raw() {
                            assert_eq!(
                                delta.field_mask & delta_flags::MASK_YAW,
                                0,
                                "Yaw wrap-around across 0 <-> 2PI boundary must not trigger false positive delta"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_expired_baseline_rate_limited_keyframe_fallback() {
        let mut aoi = AoiManager::new();
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));

        let obs_guid = make_guid(100);
        store.spawn(obs_guid, (0.0, 0.0, 0.0));
        octree.insert(obs_guid, (0.0, 0.0, 0.0));

        let target_guid = make_guid(200);
        store.spawn(target_guid, (0.0, 0.0, 10.0));
        octree.insert(target_guid, (0.0, 0.0, 10.0));

        let config = ObserverConfig {
            base_radius: 50.0,
            enable_delta_compression: true,
            full_refresh_interval_ticks: 20, // 1 sec at 20 Hz
            ..Default::default()
        };
        aoi.register_observer(1, obs_guid, config);

        // Tick 1: keyframe sent
        let jobs1 = aoi.update_observers(&store, &octree, 1, 1000);
        assert_eq!(jobs1.len(), 1);

        // Tick 2: Client does NOT ack (baseline remains unconfirmed / expired).
        // Since entity is stationary, server must NOT spam another keyframe!
        let jobs2 = aoi.update_observers(&store, &octree, 2, 1050);
        assert_eq!(
            jobs2.len(),
            0,
            "Stationary entity must not be keyframed every tick during baseline loss"
        );

        // Tick 21: Full refresh interval (20 ticks) elapsed -> rate-limited refresh is emitted
        let jobs21 = aoi.update_observers(&store, &octree, 21, 2000);
        assert_eq!(
            jobs21.len(),
            1,
            "Rate-limited refresh emitted after full_refresh_interval_ticks"
        );
    }

    #[test]
    fn test_register_observer_evicts_old_session_on_takeover() {
        let mut aoi = AoiManager::new();
        let player_guid = make_guid(1234);

        aoi.register_observer(10, player_guid, ObserverConfig::default());
        assert_eq!(aoi.observers.len(), 1);
        assert!(aoi.get_observer(10).is_some());
        assert_eq!(aoi.guid_to_session.get(&player_guid), Some(&10));

        // Reconnect / handover with session 20
        aoi.register_observer(20, player_guid, ObserverConfig::default());
        assert_eq!(aoi.observers.len(), 1);
        assert!(
            aoi.get_observer(10).is_none(),
            "Old session 10 must be evicted"
        );
        assert!(
            aoi.get_observer(20).is_some(),
            "New session 20 must be active"
        );
        assert_eq!(aoi.guid_to_session.get(&player_guid), Some(&20));
    }
}
