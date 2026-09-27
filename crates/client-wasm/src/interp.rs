//! Temporal Interpolation Buffer for entity positions and yaw.
//!
//! Implements a 100ms jitter buffer (2 server ticks at 20 Hz), smooth linear interpolation,
//! shortest-arc yaw spherical interpolation, 20m snapback threshold, and velocity extrapolation clamp.

use std::collections::{HashMap, VecDeque};

/// A timestamped snapshot of an entity's kinematic state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntitySnapshot {
    pub tick: u64,
    pub timestamp_sec: f64,
    pub position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub yaw: f32,
}

/// Buffer storing historical snapshots for client-side temporal smoothing.
pub struct InterpolationBuffer {
    /// Render delay behind real-time (default: 0.10s = 100ms).
    pub render_delay_sec: f64,
    /// Distance threshold beyond which position snaps instead of interpolating (e.g. 20.0m).
    pub snapback_threshold: f32,
    /// Maximum extrapolation time under packet starvation (e.g. 0.20s).
    pub max_extrapolation_sec: f64,
    /// Extrapolation timeout after which entity clamps to stationary (e.g. 0.25s).
    pub clamp_timeout_sec: f64,
    /// Max snapshots kept in ring buffer per entity.
    max_history: usize,
    /// Snapshot queues keyed by entity ID.
    history: HashMap<u64, VecDeque<EntitySnapshot>>,
}

impl Default for InterpolationBuffer {
    fn default() -> Self {
        Self::new(0.10)
    }
}

impl InterpolationBuffer {
    /// Creates a new `InterpolationBuffer` with the specified render delay in seconds.
    pub fn new(render_delay_sec: f64) -> Self {
        Self {
            render_delay_sec,
            snapback_threshold: 20.0,
            max_extrapolation_sec: 0.20,
            clamp_timeout_sec: 0.25,
            max_history: 32,
            history: HashMap::new(),
        }
    }

    /// Pushes a new kinematic snapshot for an entity.
    pub fn push_snapshot(
        &mut self,
        entity_id: u64,
        tick: u64,
        timestamp_sec: f64,
        position: (f32, f32, f32),
        velocity: (f32, f32, f32),
        yaw: f32,
    ) {
        let queue = self.history.entry(entity_id).or_default();

        // Discard out-of-order ticks if any
        if let Some(back) = queue.back() {
            if timestamp_sec <= back.timestamp_sec {
                return;
            }
        }

        queue.push_back(EntitySnapshot {
            tick,
            timestamp_sec,
            position,
            velocity,
            yaw,
        });

        while queue.len() > self.max_history {
            queue.pop_front();
        }
    }

    /// Samples the interpolated or extrapolated `(position, yaw)` for an entity at `current_time_sec`.
    pub fn sample(&self, entity_id: u64, current_time_sec: f64) -> Option<((f32, f32, f32), f32)> {
        let queue = self.history.get(&entity_id)?;
        if queue.is_empty() {
            return None;
        }

        let render_time = current_time_sec - self.render_delay_sec;

        // If only 1 snapshot, return it directly
        if queue.len() == 1 {
            let s = &queue[0];
            return Some((s.position, s.yaw));
        }

        let oldest = queue.front().unwrap();
        let newest = queue.back().unwrap();

        // Case 1: Render time is older than all snapshots in history
        if render_time <= oldest.timestamp_sec {
            return Some((oldest.position, oldest.yaw));
        }

        // Case 2: Render time is newer than newest snapshot -> Extrapolate or clamp
        if render_time >= newest.timestamp_sec {
            let delta = render_time - newest.timestamp_sec;
            if delta > self.clamp_timeout_sec {
                // Starvation clamp: clamp to stationary position
                return Some((newest.position, newest.yaw));
            }
            let extrap_dt = delta.min(self.max_extrapolation_sec) as f32;
            let ext_x = newest.position.0 + newest.velocity.0 * extrap_dt;
            let ext_y = newest.position.1 + newest.velocity.1 * extrap_dt;
            let ext_z = newest.position.2 + newest.velocity.2 * extrap_dt;
            return Some(((ext_x, ext_y, ext_z), newest.yaw));
        }

        // Case 3: Interpolate between two surrounding snapshots S0 and S1
        for i in 0..(queue.len() - 1) {
            let s0 = &queue[i];
            let s1 = &queue[i + 1];

            if s0.timestamp_sec <= render_time && render_time <= s1.timestamp_sec {
                let dt = s1.timestamp_sec - s0.timestamp_sec;
                if dt <= 1e-6 {
                    return Some((s1.position, s1.yaw));
                }

                // Check snapback threshold (e.g. teleport or boundary warp)
                let dx = s1.position.0 - s0.position.0;
                let dy = s1.position.1 - s0.position.1;
                let dz = s1.position.2 - s0.position.2;
                let dist_sq = dx * dx + dy * dy + dz * dz;

                if dist_sq > self.snapback_threshold * self.snapback_threshold {
                    // Snap immediately to latest target
                    return Some((s1.position, s1.yaw));
                }

                let alpha = ((render_time - s0.timestamp_sec) / dt).clamp(0.0, 1.0) as f32;

                // LERP position
                let interp_pos = (
                    s0.position.0 + (s1.position.0 - s0.position.0) * alpha,
                    s0.position.1 + (s1.position.1 - s0.position.1) * alpha,
                    s0.position.2 + (s1.position.2 - s0.position.2) * alpha,
                );

                // Shortest-arc angular interpolation for yaw
                let interp_yaw = lerp_angle(s0.yaw, s1.yaw, alpha);

                return Some((interp_pos, interp_yaw));
            }
        }

        Some((newest.position, newest.yaw))
    }

    /// Removes all history for an entity upon despawn.
    pub fn remove_entity(&mut self, entity_id: u64) {
        self.history.remove(&entity_id);
    }

    /// Clears all interpolation history.
    pub fn clear(&mut self) {
        self.history.clear();
    }
}

/// Computes shortest-arc interpolation between two angles in radians.
fn lerp_angle(a0: f32, a1: f32, t: f32) -> f32 {
    let two_pi = std::f32::consts::PI * 2.0;
    let diff = (a1 - a0).rem_euclid(two_pi);
    let shortest_diff = if diff > std::f32::consts::PI {
        diff - two_pi
    } else {
        diff
    };
    (a0 + shortest_diff * t).rem_euclid(two_pi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interpolation_between_two_snapshots() {
        let mut buffer = InterpolationBuffer::new(0.10); // 100ms render delay
        let entity_id = 42;

        // Push S0 at t=1.0s, S1 at t=1.1s (100ms apart)
        buffer.push_snapshot(entity_id, 1, 1.0, (0.0, 0.0, 0.0), (10.0, 0.0, 0.0), 0.0);
        buffer.push_snapshot(entity_id, 2, 1.1, (10.0, 0.0, 0.0), (10.0, 0.0, 0.0), 1.0);

        // At current_time = 1.15s, render_time = 1.15 - 0.10 = 1.05s (exactly halfway)
        let (pos, yaw) = buffer.sample(entity_id, 1.15).expect("should sample");

        assert!((pos.0 - 5.0).abs() < 1e-4);
        assert!((pos.1 - 0.0).abs() < 1e-4);
        assert!((pos.2 - 0.0).abs() < 1e-4);
        assert!((yaw - 0.5).abs() < 1e-4);
    }

    #[test]
    fn test_snapback_threshold_teleport() {
        let mut buffer = InterpolationBuffer::new(0.10);
        let entity_id = 101;

        // Teleport from (0,0,0) to (100,0,0) > 20m threshold
        buffer.push_snapshot(entity_id, 1, 1.0, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0), 0.0);
        buffer.push_snapshot(entity_id, 2, 1.1, (100.0, 0.0, 0.0), (0.0, 0.0, 0.0), 0.0);

        // Render time halfway at 1.05s
        let (pos, _) = buffer.sample(entity_id, 1.15).expect("should sample");

        // Should snap immediately to S1 (100.0) instead of smoothly blending through 50.0m
        assert_eq!(pos.0, 100.0);
    }

    #[test]
    fn test_velocity_extrapolation_and_starvation_clamp() {
        let mut buffer = InterpolationBuffer::new(0.10);
        let entity_id = 202;

        buffer.push_snapshot(entity_id, 1, 1.0, (10.0, 0.0, 0.0), (5.0, 0.0, 0.0), 0.0);
        buffer.push_snapshot(entity_id, 2, 1.1, (10.5, 0.0, 0.0), (5.0, 0.0, 0.0), 0.0);

        // Current time = 1.25s -> render_time = 1.15s (50ms past newest snapshot at 1.1s)
        // Extrapolation: 10.5 + 5.0 * 0.05 = 10.75m
        let (pos, _) = buffer.sample(entity_id, 1.25).expect("should extrapolate");
        assert!((pos.0 - 10.75).abs() < 1e-3);

        // Starvation clamp: Current time = 1.50s -> render_time = 1.40s (300ms past newest > 250ms timeout)
        // Must clamp to stationary newest snapshot position (10.5)
        let (clamped_pos, _) = buffer.sample(entity_id, 1.50).expect("should clamp");
        assert_eq!(clamped_pos.0, 10.5);
    }
}
