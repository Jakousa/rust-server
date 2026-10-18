//! Server-authoritative temporal history buffer and lag-compensated spatial queries.
//!
//! Provides [`SpatialHistoryBuffer`] maintaining a zero-allocation circular ring buffer
//! of entity snapshots across the past $N$ simulation ticks (~250ms–1600ms rewind window).
//! Supports anti-cheat rewind window clamping (clamped to $\le 250\text{ms}$), fractional tick
//! scene interpolation, and lag-compensated raycast queries against historical entity bounding boxes.

use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::time::{Duration, Instant};

use mmo_core::guid::Guid;
use mmo_core::octree::BoundingBox3D;

/// Spatial and kinematic snapshot of an active entity at a single simulation tick.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalEntityRecord {
    pub guid: Guid,
    pub position: (f32, f32, f32),
    pub bounds: BoundingBox3D,
    pub velocity: (f32, f32, f32),
    pub yaw: f32,
    pub pitch: f32,
}

/// Contiguous container holding all entity snapshots recorded during a single simulation tick.
#[derive(Debug, Clone)]
pub struct HistoricalTickFrame {
    pub tick: u64,
    pub timestamp: Instant,
    pub timestamp_ms: u64,
    pub entities: Vec<HistoricalEntityRecord>,
}

impl HistoricalTickFrame {
    /// Creates a preallocated frame with capacity for `capacity` entity records.
    pub fn new(capacity: usize) -> Self {
        Self {
            tick: 0,
            timestamp: Instant::now(),
            timestamp_ms: 0,
            entities: Vec::with_capacity(capacity),
        }
    }

    /// Clears entity records while preserving allocated heap capacity.
    pub fn clear(&mut self) {
        self.tick = 0;
        self.timestamp_ms = 0;
        self.entities.clear();
    }
}

/// Result of a successful lag-compensated raycast query.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalHitResult {
    /// GUID of the entity hit by the raycast.
    pub target_guid: Guid,
    /// 3D world impact point coordinates.
    pub hit_point: (f32, f32, f32),
    /// Distance along the ray from the origin to the impact point.
    pub distance: f32,
    /// Bounding simulation tick frame bracket `(tick_a, tick_b)` used for interpolation.
    pub tick_bracket: (u64, u64),
}

/// Errors returned by historical range and spatial query validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoricalQueryError {
    EmptyBuffer,
    TargetNotFound(Guid),
    TargetOutOfRange { distance: u32, max_range: u32 },
    InvalidRayVector,
}

impl Display for HistoricalQueryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::EmptyBuffer => write!(f, "spatial history buffer is empty"),
            Self::TargetNotFound(guid) => {
                write!(f, "target entity {guid} not found in historical scene")
            }
            Self::TargetOutOfRange {
                distance,
                max_range,
            } => {
                write!(
                    f,
                    "target out of range: distance {distance} > max {max_range}"
                )
            }
            Self::InvalidRayVector => write!(
                f,
                "ray origin or direction vector contains non-finite or degenerate values"
            ),
        }
    }
}

impl Error for HistoricalQueryError {}

/// Fixed-capacity circular ring buffer maintaining historical simulation tick frames.
///
/// Designed for zero allocations per simulation tick by reusing preallocated frame slabs.
pub struct SpatialHistoryBuffer {
    frames: Vec<HistoricalTickFrame>,
    capacity: usize,
    entity_capacity: usize,
    head: usize,
    count: usize,
    max_rewind_window: Duration,
}

impl SpatialHistoryBuffer {
    /// Default history capacity in ticks (32 ticks at 20 Hz = 1.6s).
    pub const DEFAULT_CAPACITY: usize = 32;
    /// Default preallocated entity records per frame.
    pub const DEFAULT_ENTITY_CAPACITY: usize = 256;
    /// Authoritative anti-cheat ceiling for maximum allowable client rewind (250ms).
    pub const MAX_REWIND_WINDOW: Duration = Duration::from_millis(250);

    /// Creates a new `SpatialHistoryBuffer` preallocating `capacity` tick frames.
    pub fn new(capacity: usize, entity_capacity: usize) -> Self {
        let capacity = capacity.max(2);
        let frames = (0..capacity)
            .map(|_| HistoricalTickFrame::new(entity_capacity))
            .collect();

        Self {
            frames,
            capacity,
            entity_capacity,
            head: 0,
            count: 0,
            max_rewind_window: Self::MAX_REWIND_WINDOW,
        }
    }

    /// Configures the maximum allowable client rewind window duration.
    pub fn with_max_rewind_window(mut self, window: Duration) -> Self {
        self.max_rewind_window = window;
        self
    }

    /// Returns the number of valid tick frames currently stored in the buffer.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if no frames have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns the total ring buffer frame capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the preallocated entity capacity per frame.
    pub fn entity_capacity(&self) -> usize {
        self.entity_capacity
    }

    /// Returns the maximum allowable rewind window duration.
    pub fn max_rewind_window(&self) -> Duration {
        self.max_rewind_window
    }

    /// Returns a reference to the newest recorded tick frame, if any.
    pub fn newest_frame(&self) -> Option<&HistoricalTickFrame> {
        if self.count == 0 {
            None
        } else {
            Some(&self.frames[self.head])
        }
    }

    /// Returns the oldest valid frame index in circular storage.
    fn oldest_index(&self) -> usize {
        (self.head + self.capacity + 1 - self.count) % self.capacity
    }

    /// Returns a reference to the oldest recorded tick frame in the buffer.
    pub fn oldest_frame(&self) -> Option<&HistoricalTickFrame> {
        if self.count == 0 {
            None
        } else {
            Some(&self.frames[self.oldest_index()])
        }
    }

    /// Returns the newest recorded simulation tick number.
    pub fn newest_tick(&self) -> Option<u64> {
        self.newest_frame().map(|f| f.tick)
    }

    /// Returns the oldest recorded simulation tick number.
    pub fn oldest_tick(&self) -> Option<u64> {
        self.oldest_frame().map(|f| f.tick)
    }

    /// Ingests a new simulation tick frame, overwriting the oldest slot when full.
    ///
    /// Reuses existing vector memory via `.clear()` to achieve zero per-tick heap allocations.
    pub fn push_frame(
        &mut self,
        tick: u64,
        timestamp: Instant,
        timestamp_ms: u64,
        entities_iter: impl Iterator<Item = HistoricalEntityRecord>,
    ) {
        let index = if self.count > 0 {
            (self.head + 1) % self.capacity
        } else {
            0
        };

        let frame = &mut self.frames[index];
        frame.clear();
        frame.tick = tick;
        frame.timestamp = timestamp;
        frame.timestamp_ms = timestamp_ms;
        frame.entities.extend(entities_iter);

        self.head = index;
        self.count = (self.count + 1).min(self.capacity);
    }

    /// Finds a frame by its exact tick number.
    pub fn find_frame_by_tick(&self, tick: u64) -> Option<&HistoricalTickFrame> {
        if self.count == 0 {
            return None;
        }
        let oldest = self.oldest_index();
        for i in 0..self.count {
            let idx = (oldest + i) % self.capacity;
            if self.frames[idx].tick == tick {
                return Some(&self.frames[idx]);
            }
        }
        None
    }

    /// Clamps a requested target timestamp against anti-cheat boundaries:
    /// - Rejects future timestamps by clamping to `current_time`.
    /// - Enforces maximum rewind limit ($t_{\text{rewind}} \le \text{max\_rewind\_window}$).
    /// - Clamps to oldest available frame timestamp in the buffer.
    ///
    /// Returns `(clamped_instant, was_clamped)`.
    pub fn clamp_rewind_timestamp(
        &self,
        current_time: Instant,
        target_time: Instant,
    ) -> (Instant, bool) {
        let mut clamped = target_time;
        let mut was_clamped = false;

        // 1. Future timestamp defense
        if target_time > current_time {
            clamped = current_time;
            was_clamped = true;
        } else {
            // 2. Maximum rewind duration defense (<= 250ms)
            let rewind_duration = current_time.duration_since(target_time);
            if rewind_duration > self.max_rewind_window {
                clamped = current_time - self.max_rewind_window;
                was_clamped = true;
            }
        }

        // 3. Oldest available frame defense
        if let Some(oldest) = self.oldest_frame() {
            if clamped < oldest.timestamp {
                clamped = oldest.timestamp;
                was_clamped = true;
            }
        }

        if was_clamped {
            metrics::counter!("mmo_lag_compensation_rewind_clamped_total").increment(1);
        }

        (clamped, was_clamped)
    }

    /// Millisecond variant of anti-cheat rewind window clamping.
    pub fn clamp_rewind_ms(&self, current_ms: u64, target_ms: u64) -> (u64, bool) {
        let max_rewind_ms = self.max_rewind_window.as_millis() as u64;
        let mut clamped = target_ms;
        let mut was_clamped = false;

        if target_ms > current_ms {
            clamped = current_ms;
            was_clamped = true;
        } else if current_ms.saturating_sub(target_ms) > max_rewind_ms {
            clamped = current_ms.saturating_sub(max_rewind_ms);
            was_clamped = true;
        }

        if let Some(oldest) = self.oldest_frame() {
            if clamped < oldest.timestamp_ms {
                clamped = oldest.timestamp_ms;
                was_clamped = true;
            }
        }

        if was_clamped {
            metrics::counter!("mmo_lag_compensation_rewind_clamped_total").increment(1);
        }

        (clamped, was_clamped)
    }

    /// Finds the two bounding frames surrounding `target_time` and computes the normalized
    /// interpolation factor $\alpha \in [0.0, 1.0]$.
    pub fn find_bounding_frames(
        &self,
        target_time: Instant,
    ) -> Option<(&HistoricalTickFrame, &HistoricalTickFrame, f32)> {
        if self.count == 0 {
            return None;
        }
        let oldest = self.oldest_frame()?;
        let newest = self.newest_frame()?;

        if target_time <= oldest.timestamp {
            return Some((oldest, oldest, 0.0));
        }
        if target_time >= newest.timestamp {
            return Some((newest, newest, 1.0));
        }

        let start = self.oldest_index();
        for i in 0..(self.count - 1) {
            let idx_a = (start + i) % self.capacity;
            let idx_b = (start + i + 1) % self.capacity;
            let f_a = &self.frames[idx_a];
            let f_b = &self.frames[idx_b];

            if target_time >= f_a.timestamp && target_time <= f_b.timestamp {
                let dt = f_b.timestamp.duration_since(f_a.timestamp).as_secs_f32();
                let alpha = if dt > 1e-6 {
                    (target_time.duration_since(f_a.timestamp).as_secs_f32() / dt).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return Some((f_a, f_b, alpha));
            }
        }

        Some((newest, newest, 1.0))
    }

    /// Finds the two bounding frames surrounding `target_ms` and computes $\alpha \in [0.0, 1.0]$.
    pub fn find_bounding_frames_ms(
        &self,
        target_ms: u64,
    ) -> Option<(&HistoricalTickFrame, &HistoricalTickFrame, f32)> {
        if self.count == 0 {
            return None;
        }
        let oldest = self.oldest_frame()?;
        let newest = self.newest_frame()?;

        if target_ms <= oldest.timestamp_ms {
            return Some((oldest, oldest, 0.0));
        }
        if target_ms >= newest.timestamp_ms {
            return Some((newest, newest, 1.0));
        }

        let start = self.oldest_index();
        for i in 0..(self.count - 1) {
            let idx_a = (start + i) % self.capacity;
            let idx_b = (start + i + 1) % self.capacity;
            let f_a = &self.frames[idx_a];
            let f_b = &self.frames[idx_b];

            if target_ms >= f_a.timestamp_ms && target_ms <= f_b.timestamp_ms {
                let dt = (f_b.timestamp_ms.saturating_sub(f_a.timestamp_ms)) as f32;
                let alpha = if dt > 1e-6 {
                    ((target_ms.saturating_sub(f_a.timestamp_ms)) as f32 / dt).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return Some((f_a, f_b, alpha));
            }
        }

        Some((newest, newest, 1.0))
    }

    /// Reconstructs the historical spatial scene at `target_time`, linearly interpolating
    /// entity positions, bounding boxes, velocities, and orientations between bounding frames.
    pub fn interpolate_scene(&self, target_time: Instant) -> Vec<HistoricalEntityRecord> {
        let Some((f_a, f_b, alpha)) = self.find_bounding_frames(target_time) else {
            return Vec::new();
        };
        interpolate_frames(f_a, f_b, alpha)
    }

    /// Reconstructs the historical spatial scene at `target_ms`.
    pub fn interpolate_scene_ms(&self, target_ms: u64) -> Vec<HistoricalEntityRecord> {
        let Some((f_a, f_b, alpha)) = self.find_bounding_frames_ms(target_ms) else {
            return Vec::new();
        };
        interpolate_frames(f_a, f_b, alpha)
    }

    /// Performs an authoritative lag-compensated raycast against historical entity bounding boxes.
    ///
    /// Clamps `target_time` against anti-cheat rewind window limits, reconstructs the historical
    /// spatial scene, authoritatively re-normalizes the ray direction vector, and tests for the
    /// closest ray-AABB slab intersection.
    ///
    /// Note: If a target entity is hit in the historical scene but despawned in subsequent ticks,
    /// this function returns the geometric hit with `target_guid`. Action callers should handle
    /// `entity_store.get_by_guid(target_guid) == None` gracefully.
    pub fn raycast_historical(
        &self,
        origin: (f32, f32, f32),
        direction: (f32, f32, f32),
        max_distance: f32,
        target_time: Instant,
        current_time: Instant,
        exclude_guid: Option<Guid>,
    ) -> Option<HistoricalHitResult> {
        metrics::counter!("mmo_lag_compensation_queries_total").increment(1);

        // Guard against non-finite float coordinates
        if !origin.0.is_finite()
            || !origin.1.is_finite()
            || !origin.2.is_finite()
            || !direction.0.is_finite()
            || !direction.1.is_finite()
            || !direction.2.is_finite()
            || !max_distance.is_finite()
            || max_distance <= 0.0
        {
            return None;
        }

        // Direction vector re-normalization and degenerate vector defense (< 1e-6)
        let dir_len_sq =
            direction.0 * direction.0 + direction.1 * direction.1 + direction.2 * direction.2;
        if dir_len_sq < 1e-12 {
            return None;
        }
        let inv_len = 1.0 / dir_len_sq.sqrt();
        let norm_dir = (
            direction.0 * inv_len,
            direction.1 * inv_len,
            direction.2 * inv_len,
        );

        // Anti-cheat rewind clamping (max 250ms, future timestamps clamped to current_time)
        let (clamped_target, _) = self.clamp_rewind_timestamp(current_time, target_time);

        let (f_a, f_b, _) = self.find_bounding_frames(clamped_target)?;
        let tick_bracket = (f_a.tick, f_b.tick);
        let entities = self.interpolate_scene(clamped_target);

        raycast_interpolated_scene(
            origin,
            norm_dir,
            max_distance,
            tick_bracket,
            &entities,
            exclude_guid,
        )
    }

    /// Millisecond variant of authoritative lag-compensated raycast.
    pub fn raycast_historical_ms(
        &self,
        origin: (f32, f32, f32),
        direction: (f32, f32, f32),
        max_distance: f32,
        target_ms: u64,
        current_ms: u64,
        exclude_guid: Option<Guid>,
    ) -> Option<HistoricalHitResult> {
        metrics::counter!("mmo_lag_compensation_queries_total").increment(1);

        if !origin.0.is_finite()
            || !origin.1.is_finite()
            || !origin.2.is_finite()
            || !direction.0.is_finite()
            || !direction.1.is_finite()
            || !direction.2.is_finite()
            || !max_distance.is_finite()
            || max_distance <= 0.0
        {
            return None;
        }

        let dir_len_sq =
            direction.0 * direction.0 + direction.1 * direction.1 + direction.2 * direction.2;
        if dir_len_sq < 1e-12 {
            return None;
        }
        let inv_len = 1.0 / dir_len_sq.sqrt();
        let norm_dir = (
            direction.0 * inv_len,
            direction.1 * inv_len,
            direction.2 * inv_len,
        );

        let (clamped_ms, _) = self.clamp_rewind_ms(current_ms, target_ms);
        let (f_a, f_b, _) = self.find_bounding_frames_ms(clamped_ms)?;
        let tick_bracket = (f_a.tick, f_b.tick);
        let entities = self.interpolate_scene_ms(clamped_ms);

        raycast_interpolated_scene(
            origin,
            norm_dir,
            max_distance,
            tick_bracket,
            &entities,
            exclude_guid,
        )
    }

    /// Authoritatively validates whether a target entity was within range at historical timestamp `target_time`.
    pub fn validate_historical_range(
        &self,
        attacker_pos: (f32, f32, f32),
        target_guid: Guid,
        target_time: Instant,
        current_time: Instant,
        max_range: f32,
    ) -> Result<f32, HistoricalQueryError> {
        if self.is_empty() {
            return Err(HistoricalQueryError::EmptyBuffer);
        }

        let (clamped_target, _) = self.clamp_rewind_timestamp(current_time, target_time);
        let entities = self.interpolate_scene(clamped_target);

        let target = entities
            .into_iter()
            .find(|e| e.guid == target_guid)
            .ok_or(HistoricalQueryError::TargetNotFound(target_guid))?;

        let dx = attacker_pos.0 - target.position.0;
        let dy = attacker_pos.1 - target.position.1;
        let dz = attacker_pos.2 - target.position.2;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        if dist <= max_range {
            Ok(dist)
        } else {
            Err(HistoricalQueryError::TargetOutOfRange {
                distance: dist.round() as u32,
                max_range: max_range.round() as u32,
            })
        }
    }
}

impl Default for SpatialHistoryBuffer {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY, Self::DEFAULT_ENTITY_CAPACITY)
    }
}

/// Linearly interpolates entity positions, bounding boxes, velocities, and orientations between two bounding frames.
fn interpolate_frames(
    f_a: &HistoricalTickFrame,
    f_b: &HistoricalTickFrame,
    alpha: f32,
) -> Vec<HistoricalEntityRecord> {
    if f_a.tick == f_b.tick || alpha <= 1e-6 {
        return f_a.entities.clone();
    }
    if (alpha - 1.0).abs() <= 1e-6 {
        return f_b.entities.clone();
    }

    use std::collections::HashMap;
    let mut map_a: HashMap<Guid, &HistoricalEntityRecord> =
        HashMap::with_capacity(f_a.entities.len());
    for ent in &f_a.entities {
        map_a.insert(ent.guid, ent);
    }

    let mut reconstructed = Vec::with_capacity(f_b.entities.len());
    let inv_alpha = 1.0 - alpha;

    for ent_b in &f_b.entities {
        if let Some(ent_a) = map_a.remove(&ent_b.guid) {
            // Entity exists in both frames: linearly interpolate
            let pos = (
                ent_a.position.0 * inv_alpha + ent_b.position.0 * alpha,
                ent_a.position.1 * inv_alpha + ent_b.position.1 * alpha,
                ent_a.position.2 * inv_alpha + ent_b.position.2 * alpha,
            );
            let bounds = BoundingBox3D::new(
                (
                    ent_a.bounds.min.0 * inv_alpha + ent_b.bounds.min.0 * alpha,
                    ent_a.bounds.min.1 * inv_alpha + ent_b.bounds.min.1 * alpha,
                    ent_a.bounds.min.2 * inv_alpha + ent_b.bounds.min.2 * alpha,
                ),
                (
                    ent_a.bounds.max.0 * inv_alpha + ent_b.bounds.max.0 * alpha,
                    ent_a.bounds.max.1 * inv_alpha + ent_b.bounds.max.1 * alpha,
                    ent_a.bounds.max.2 * inv_alpha + ent_b.bounds.max.2 * alpha,
                ),
            );
            let vel = (
                ent_a.velocity.0 * inv_alpha + ent_b.velocity.0 * alpha,
                ent_a.velocity.1 * inv_alpha + ent_b.velocity.1 * alpha,
                ent_a.velocity.2 * inv_alpha + ent_b.velocity.2 * alpha,
            );
            let yaw = ent_a.yaw * inv_alpha + ent_b.yaw * alpha;
            let pitch = ent_a.pitch * inv_alpha + ent_b.pitch * alpha;

            reconstructed.push(HistoricalEntityRecord {
                guid: ent_b.guid,
                position: pos,
                bounds,
                velocity: vel,
                yaw,
                pitch,
            });
        } else if alpha >= 0.5 {
            // Entity only spawned in frame B; include if past midpoint
            reconstructed.push(ent_b.clone());
        }
    }

    // Include entities that despawned in frame B if alpha < 0.5
    if alpha < 0.5 {
        for (_, ent_a) in map_a {
            reconstructed.push(ent_a.clone());
        }
    }

    reconstructed
}

/// Helper performing closest raycast against historical entity bounding boxes.
fn raycast_interpolated_scene(
    origin: (f32, f32, f32),
    norm_dir: (f32, f32, f32),
    max_distance: f32,
    tick_bracket: (u64, u64),
    entities: &[HistoricalEntityRecord],
    exclude_guid: Option<Guid>,
) -> Option<HistoricalHitResult> {
    let mut closest_hit: Option<HistoricalHitResult> = None;
    let mut min_t = max_distance;

    for ent in entities {
        if Some(ent.guid) == exclude_guid {
            continue;
        }

        if let Some(t) = ent.bounds.intersects_ray(origin, norm_dir, min_t) {
            if t >= 0.0 && t <= min_t {
                min_t = t;
                let hit_point = (
                    origin.0 + norm_dir.0 * t,
                    origin.1 + norm_dir.1 * t,
                    origin.2 + norm_dir.2 * t,
                );
                closest_hit = Some(HistoricalHitResult {
                    target_guid: ent.guid,
                    hit_point,
                    distance: t,
                    tick_bracket,
                });
            }
        }
    }

    if closest_hit.is_some() {
        metrics::counter!("mmo_lag_compensation_hits_total").increment(1);
    }

    closest_hit
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    fn make_test_record(guid_seq: u32, pos: (f32, f32, f32)) -> HistoricalEntityRecord {
        let half = 0.5f32;
        let bounds = BoundingBox3D::new(
            (pos.0 - half, pos.1 - half, pos.2 - half),
            (pos.0 + half, pos.1 + half, pos.2 + half),
        );
        HistoricalEntityRecord {
            guid: Guid::new(EntityType::Player, 1, guid_seq),
            position: pos,
            bounds,
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    #[test]
    fn test_ring_buffer_push_and_eviction() {
        let mut buffer = SpatialHistoryBuffer::new(4, 16);
        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), 4);
        assert_eq!(buffer.entity_capacity(), 16);

        let t0 = Instant::now();
        for i in 1..=6 {
            let ent = make_test_record(1, (i as f32 * 10.0, 0.0, 0.0));
            buffer.push_frame(
                i,
                t0 + Duration::from_millis(i * 50),
                i * 50,
                std::iter::once(ent),
            );
        }

        assert_eq!(buffer.len(), 4);
        assert_eq!(buffer.oldest_tick(), Some(3));
        assert_eq!(buffer.newest_tick(), Some(6));

        // Finding frame by tick
        assert!(buffer.find_frame_by_tick(1).is_none());
        assert!(buffer.find_frame_by_tick(2).is_none());
        assert!(buffer.find_frame_by_tick(3).is_some());
        assert!(buffer.find_frame_by_tick(6).is_some());
    }

    #[test]
    fn test_rewind_anti_cheat_clamping() {
        let mut buffer = SpatialHistoryBuffer::new(10, 16);
        let t_now = Instant::now();

        // Push frame at t_now - 100ms
        let t_frame = t_now - Duration::from_millis(100);
        buffer.push_frame(1, t_frame, 900, std::iter::empty());

        // 1. Future timestamp is clamped to current_time
        let t_future = t_now + Duration::from_millis(500);
        let (clamped_future, was_clamped) = buffer.clamp_rewind_timestamp(t_now, t_future);
        assert!(was_clamped);
        assert_eq!(clamped_future, t_now);

        // 2. Target within 250ms is accepted without clamping
        let t_valid = t_now - Duration::from_millis(50);
        let (clamped_valid, was_clamped_valid) = buffer.clamp_rewind_timestamp(t_now, t_valid);
        assert!(!was_clamped_valid);
        assert_eq!(clamped_valid, t_valid);

        // 3. Excessive rewind (> 250ms) is clamped to t_now - 250ms
        let t_ancient = t_now - Duration::from_millis(600);
        let (clamped_ancient, was_clamped_ancient) =
            buffer.clamp_rewind_timestamp(t_now, t_ancient);
        assert!(was_clamped_ancient);
        // Note: clamped to oldest available frame if oldest > t_now - 250ms
        assert_eq!(clamped_ancient, t_frame);
    }

    #[test]
    fn test_scene_interpolation_and_raycast_hit() {
        let mut buffer = SpatialHistoryBuffer::new(10, 16);
        let t0 = Instant::now();

        // Target moves from (10, 0, 0) at t=0ms to (20, 0, 0) at t=100ms
        let target_guid = Guid::new(EntityType::Player, 1, 100);
        let r0 = HistoricalEntityRecord {
            guid: target_guid,
            position: (10.0, 0.0, 0.0),
            bounds: BoundingBox3D::new((9.5, -0.5, -0.5), (10.5, 0.5, 0.5)),
            velocity: (10.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
        };
        buffer.push_frame(1, t0, 0, std::iter::once(r0));

        let t1 = t0 + Duration::from_millis(100);
        let r1 = HistoricalEntityRecord {
            guid: target_guid,
            position: (20.0, 0.0, 0.0),
            bounds: BoundingBox3D::new((19.5, -0.5, -0.5), (20.5, 0.5, 0.5)),
            velocity: (10.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
        };
        buffer.push_frame(2, t1, 100, std::iter::once(r1));

        // Query at t = 50ms (halfway): target should be at (15, 0, 0)
        let t_mid = t0 + Duration::from_millis(50);
        let interpolated = buffer.interpolate_scene(t_mid);
        assert_eq!(interpolated.len(), 1);
        assert!((interpolated[0].position.0 - 15.0).abs() < 1e-4);
        assert!((interpolated[0].bounds.min.0 - 14.5).abs() < 1e-4);
        assert!((interpolated[0].bounds.max.0 - 15.5).abs() < 1e-4);

        // Raycast from origin (0, 0, 0) along +X axis towards target at t=50ms
        let hit =
            buffer.raycast_historical((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), 50.0, t_mid, t1, None);

        assert!(hit.is_some());
        let hit_res = hit.unwrap();
        assert_eq!(hit_res.target_guid, target_guid);
        assert!((hit_res.distance - 14.5).abs() < 1e-3);
        assert!((hit_res.hit_point.0 - 14.5).abs() < 1e-3);
    }

    #[test]
    fn test_degenerate_direction_and_out_of_range_rejected() {
        let mut buffer = SpatialHistoryBuffer::new(10, 16);
        let t0 = Instant::now();
        let target = make_test_record(1, (10.0, 0.0, 0.0));
        buffer.push_frame(1, t0, 0, std::iter::once(target));

        // Degenerate zero vector
        assert!(buffer
            .raycast_historical((0.0, 0.0, 0.0), (0.0, 0.0, 0.0), 50.0, t0, t0, None)
            .is_none());

        // Negative max distance
        assert!(buffer
            .raycast_historical((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), -10.0, t0, t0, None)
            .is_none());

        // NaN coordinates
        assert!(buffer
            .raycast_historical((f32::NAN, 0.0, 0.0), (1.0, 0.0, 0.0), 50.0, t0, t0, None)
            .is_none());
    }
}
