//! 3D kinematic trajectory generator for simulated bot swarm.
//!
//! Generates realistic kinematic movements with random-walk wander behaviors
//! that cross configured boundary planes to exercise cluster route handovers.

use mmo_schema::client::Vec3;
use rand::Rng;

/// Defines a 3D bounding box for the simulation arena.
#[derive(Debug, Clone, Copy)]
pub struct WorldBounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl WorldBounds {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Default world bounds centered at origin (-100.0 to 100.0)
    pub fn default_arena() -> Self {
        Self {
            min: Vec3::new(-100.0, -100.0, -100.0),
            max: Vec3::new(100.0, 100.0, 100.0),
        }
    }

    /// Clamps a 3D coordinate inside the boundary volume.
    pub fn clamp(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            p.x.clamp(self.min.x, self.max.x),
            p.y.clamp(self.min.y, self.max.y),
            p.z.clamp(self.min.z, self.max.z),
        )
    }

    /// Checks if point is inside the bounding box.
    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }
}

/// Kinematic trajectory controller for a single simulated bot.
pub struct TrajectoryController {
    pub position: Vec3,
    pub velocity: Vec3,
    pub target: Vec3,
    pub max_speed: f32,
    pub bounds: WorldBounds,
}

impl TrajectoryController {
    /// Creates a new trajectory controller at the given start position.
    pub fn new(start: Vec3, max_speed: f32, bounds: WorldBounds) -> Self {
        let mut ctrl = Self {
            position: bounds.clamp(start),
            velocity: Vec3::ZERO,
            target: Vec3::ZERO,
            max_speed,
            bounds,
        };
        ctrl.pick_new_target();
        ctrl
    }

    /// Selects a new target waypoint, deliberately picking across boundary planes.
    pub fn pick_new_target(&mut self) {
        let mut rng = rand::thread_rng();

        // Pick a coordinate across the origin or quadrant boundary
        let target_x = if self.position.x > 0.0 {
            rng.gen_range(self.bounds.min.x..0.0)
        } else {
            rng.gen_range(0.0..self.bounds.max.x)
        };

        let target_y = rng.gen_range(self.bounds.min.y..self.bounds.max.y);

        let target_z = if self.position.z > 0.0 {
            rng.gen_range(self.bounds.min.z..0.0)
        } else {
            rng.gen_range(0.0..self.bounds.max.z)
        };

        self.target = Vec3::new(target_x, target_y, target_z);
    }

    /// Advances the trajectory by `dt_seconds` towards the target waypoint.
    ///
    /// Returns the updated `(position, velocity)`.
    pub fn step(&mut self, dt_seconds: f32) -> (Vec3, Vec3) {
        if dt_seconds <= 0.0 {
            return (self.position, self.velocity);
        }

        let dx = self.target.x - self.position.x;
        let dy = self.target.y - self.position.y;
        let dz = self.target.z - self.position.z;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        // If target reached within waypoint arrival radius, choose a new target
        if dist < 2.0 {
            self.pick_new_target();
            return self.step(dt_seconds);
        }

        // Compute direction unit vector
        let dir_x = dx / dist;
        let dir_y = dy / dist;
        let dir_z = dz / dist;

        // Kinematic step
        let speed = self.max_speed.min(dist / dt_seconds);
        self.velocity = Vec3::new(dir_x * speed, dir_y * speed, dir_z * speed);

        let new_pos = Vec3::new(
            self.position.x + self.velocity.x * dt_seconds,
            self.position.y + self.velocity.y * dt_seconds,
            self.position.z + self.velocity.z * dt_seconds,
        );

        self.position = self.bounds.clamp(new_pos);
        (self.position, self.velocity)
    }

    /// Checks if the bot recently crossed an axis boundary plane (X=0 or Z=0).
    pub fn crosses_boundary_plane(&self, prev_pos: Vec3) -> bool {
        (prev_pos.x * self.position.x < 0.0) || (prev_pos.z * self.position.z < 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trajectory_controller_stepping() {
        let bounds = WorldBounds::new(
            Vec3::new(-100.0, -100.0, -100.0),
            Vec3::new(100.0, 100.0, 100.0),
        );
        let mut ctrl = TrajectoryController::new(Vec3::new(10.0, 0.0, 10.0), 10.0, bounds);

        let initial_pos = ctrl.position;
        let (next_pos, vel) = ctrl.step(0.05); // 50ms tick

        assert_ne!(initial_pos, next_pos);
        assert!(bounds.contains(next_pos));
        assert!(vel.x.abs() <= 10.001);
        assert!(vel.z.abs() <= 10.001);
    }

    #[test]
    fn test_boundary_crossing_detection() {
        let bounds = WorldBounds::default_arena();
        let ctrl = TrajectoryController::new(Vec3::new(5.0, 0.0, 5.0), 10.0, bounds);

        assert!(ctrl.crosses_boundary_plane(Vec3::new(-5.0, 0.0, 5.0)));
        assert!(ctrl.crosses_boundary_plane(Vec3::new(5.0, 0.0, -5.0)));
        assert!(!ctrl.crosses_boundary_plane(Vec3::new(2.0, 0.0, 2.0)));
    }
}
