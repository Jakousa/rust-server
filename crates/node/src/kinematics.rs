//! Kinematic validation module for player movement sanity checking and anti-cheat.
//!
//! Validates client-reported coordinates against maximum speed limits, delta time bounds,
//! finite float values, and optional teleport distance thresholds, returning a snap-back
//! position to correct invalid movement.

use std::fmt;

use mmo_core::collision::{CollisionWorld, SpatialQueryFilter};
use mmo_core::guid::Guid;
use mmo_core::octree::BoundingBox3D;
use mmo_core::spatial::Vec3;

/// Specific reason for a kinematic movement violation.
#[derive(Debug, Clone, PartialEq)]
pub enum KinematicViolationReason {
    /// Distance traveled exceeded allowable speed over elapsed delta time.
    SpeedCapExceeded { speed: f32, max_speed: f32 },
    /// Instantaneous distance exceeded teleportation threshold.
    TeleportDetected { distance: f32, max_distance: f32 },
    /// Non-positive or non-finite delta time provided.
    InvalidDeltaTime { dt: f32 },
    /// Non-finite (NaN or infinite) coordinates in new position.
    NonFiniteCoordinates,
    /// Non-monotonic or replayed movement sequence number.
    NonMonotonicSequence { seq: u32, last_seq: u32 },
    /// Movement trajectory intersected solid collision geometry.
    ObstacleCollision {
        hit_point: (f32, f32, f32),
        entity_id: Option<Guid>,
    },
    /// Volumetric collision sweep detected penetration or clipping into solid collision geometry.
    WallClipping {
        hit_point: (f32, f32, f32),
        entity_id: Option<Guid>,
    },
    /// Rate of velocity change exceeded allowable acceleration limit.
    AccelerationCapExceeded {
        acceleration: f32,
        max_acceleration: f32,
    },
}

impl fmt::Display for KinematicViolationReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpeedCapExceeded { speed, max_speed } => {
                write!(
                    f,
                    "speed cap exceeded: speed {speed:.2} > max {max_speed:.2}"
                )
            }
            Self::TeleportDetected {
                distance,
                max_distance,
            } => {
                write!(
                    f,
                    "teleport detected: distance {distance:.2} > max {max_distance:.2}"
                )
            }
            Self::InvalidDeltaTime { dt } => {
                write!(f, "invalid delta time: {dt}")
            }
            Self::NonFiniteCoordinates => {
                write!(f, "non-finite coordinates detected")
            }
            Self::NonMonotonicSequence { seq, last_seq } => {
                write!(
                    f,
                    "non-monotonic sequence: packet seq {seq} <= last {last_seq}"
                )
            }
            Self::ObstacleCollision {
                hit_point,
                entity_id,
            } => {
                write!(
                    f,
                    "obstacle collision detected at ({:.2}, {:.2}, {:.2}) with entity {:?}",
                    hit_point.0, hit_point.1, hit_point.2, entity_id
                )
            }
            Self::WallClipping {
                hit_point,
                entity_id,
            } => {
                write!(
                    f,
                    "wall clipping detected at ({:.2}, {:.2}, {:.2}) with entity {:?}",
                    hit_point.0, hit_point.1, hit_point.2, entity_id
                )
            }
            Self::AccelerationCapExceeded {
                acceleration,
                max_acceleration,
            } => {
                write!(
                    f,
                    "acceleration cap exceeded: acceleration {acceleration:.2} > max {max_acceleration:.2}"
                )
            }
        }
    }
}

/// Server-authorized temporary movement allowance granting additional displacement budget
/// to exempt knockbacks, dashes, and teleports from anti-cheat speed violations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpulseAllowance {
    pub bonus_distance: f32,
    pub expires_at_tick: u64,
}

/// Kinematic motion model applied during entity movement simulation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum MotionMode {
    /// Discrete ground/character movement with direct position updates and AABB collision sweeps.
    #[default]
    DiscreteGround,
    /// 6-DoF Newtonian physics model with velocity, acceleration caps, and linear damping.
    Newtonian6DoF {
        max_acceleration: f32,
        linear_damping: f32,
    },
}

/// Configuration settings for kinematic motion validation.
#[derive(Debug, Clone, PartialEq)]
pub struct KinematicConfig {
    pub max_speed: f32,
    pub max_teleport_distance: Option<f32>,
    pub tolerance_multiplier: f32,
}

impl Default for KinematicConfig {
    fn default() -> Self {
        Self {
            max_speed: KinematicValidator::DEFAULT_MAX_SPEED,
            max_teleport_distance: None,
            tolerance_multiplier: 1.1,
        }
    }
}

/// Server-side monotonic kinematic tracking state for an active entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityKinematicState {
    pub last_move_time: std::time::Instant,
    pub last_sequence: u32,
    pub initialized: bool,
    pub last_position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub collider_half_extents: (f32, f32, f32),
    pub motion_mode: MotionMode,
    pub impulse_allowance: Option<ImpulseAllowance>,
    pub last_dt: f32,
    pub time_credit: f32,
}

impl EntityKinematicState {
    /// Creates a new uninitialized `EntityKinematicState` anchored to `initial_time`.
    pub fn new(initial_time: std::time::Instant) -> Self {
        Self {
            last_move_time: initial_time,
            last_sequence: 0,
            initialized: false,
            last_position: (0.0, 0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            collider_half_extents: (0.4, 0.9, 0.4),
            motion_mode: MotionMode::DiscreteGround,
            impulse_allowance: None,
            last_dt: 0.05,
            time_credit: 0.2,
        }
    }

    /// Sets the initial or updated last known position of the entity.
    pub fn with_position(mut self, pos: (f32, f32, f32)) -> Self {
        self.last_position = pos;
        self
    }

    /// Grants an authorized server impulse allowance.
    pub fn grant_impulse(&mut self, bonus_distance: f32, duration_ticks: u64, current_tick: u64) {
        self.impulse_allowance = Some(ImpulseAllowance {
            bonus_distance,
            expires_at_tick: current_tick + duration_ticks,
        });
    }
}

/// Pluggable motion integration strategy for validating physical movement steps.
pub trait MotionIntegrator: Send + Sync {
    fn validate_step(
        &self,
        config: &KinematicConfig,
        state: &mut EntityKinematicState,
        proposed_pos: (f32, f32, f32),
        current_tick: u64,
        collision: Option<&dyn CollisionWorld>,
        mover_guid: Option<Guid>,
    ) -> Result<(f32, f32, f32), KinematicViolation>;
}

/// Discrete ground movement integrator with AABB volumetric sweep testing.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscreteKinematicIntegrator;

impl MotionIntegrator for DiscreteKinematicIntegrator {
    fn validate_step(
        &self,
        config: &KinematicConfig,
        state: &mut EntityKinematicState,
        proposed_pos: (f32, f32, f32),
        current_tick: u64,
        collision: Option<&dyn CollisionWorld>,
        mover_guid: Option<Guid>,
    ) -> Result<(f32, f32, f32), KinematicViolation> {
        let current_pos = state.last_position;
        if !proposed_pos.0.is_finite() || !proposed_pos.1.is_finite() || !proposed_pos.2.is_finite()
        {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::NonFiniteCoordinates,
            ));
        }

        let dt_sec = state.last_dt;
        if dt_sec <= 0.0 || !dt_sec.is_finite() {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::InvalidDeltaTime { dt: dt_sec },
            ));
        }

        let dx = proposed_pos.0 - current_pos.0;
        let dy = proposed_pos.1 - current_pos.1;
        let dz = proposed_pos.2 - current_pos.2;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();

        let mut bonus_dist = 0.0;
        if let Some(allowance) = state.impulse_allowance {
            if allowance.expires_at_tick >= current_tick {
                bonus_dist = allowance.bonus_distance;
            }
            state.impulse_allowance = None;
        }

        if let Some(base_max_teleport) = config.max_teleport_distance {
            let effective_max_teleport = base_max_teleport + bonus_dist;
            if distance > effective_max_teleport + 1e-4 {
                return Err(KinematicViolation::new(
                    current_pos,
                    KinematicViolationReason::TeleportDetected {
                        distance,
                        max_distance: effective_max_teleport,
                    },
                ));
            }
        }

        let effective_max_speed = config.max_speed + (bonus_dist / dt_sec);
        let speed = distance / dt_sec;
        if speed > effective_max_speed + 1e-4 {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::SpeedCapExceeded {
                    speed,
                    max_speed: effective_max_speed,
                },
            ));
        }

        if distance > 1e-4 {
            if let Some(cw) = collision {
                let from_vec = Vec3::new(current_pos.0, current_pos.1, current_pos.2);
                let dir = Vec3::new(dx / distance, dy / distance, dz / distance);
                let filter = SpatialQueryFilter {
                    ignore_id: mover_guid,
                    collision_layers: u32::MAX,
                };

                let check_dist = (distance - 0.05).max(0.0);
                if check_dist > 0.0 {
                    if let Some(hit) = cw.raycast(from_vec, dir, check_dist, filter) {
                        if hit.distance < check_dist {
                            return Err(KinematicViolation::new(
                                current_pos,
                                KinematicViolationReason::ObstacleCollision {
                                    hit_point: (hit.point.x, hit.point.y, hit.point.z),
                                    entity_id: hit.entity_id,
                                },
                            ));
                        }
                    }
                }

                if state.collider_half_extents.0 > 0.0 {
                    let half = state.collider_half_extents;
                    let aabb = BoundingBox3D::new(
                        (
                            current_pos.0 - half.0,
                            current_pos.1 - half.1,
                            current_pos.2 - half.2,
                        ),
                        (
                            current_pos.0 + half.0,
                            current_pos.1 + half.1,
                            current_pos.2 + half.2,
                        ),
                    );
                    let translation = Vec3::new(dx, dy, dz);
                    if let Some(hit) = cw.sweep_test(&aabb, translation, filter) {
                        if hit.hit_fraction < 1.0 {
                            return Err(KinematicViolation::new(
                                current_pos,
                                KinematicViolationReason::WallClipping {
                                    hit_point: (hit.point.x, hit.point.y, hit.point.z),
                                    entity_id: hit.entity_id,
                                },
                            ));
                        }
                    }
                }

                let dest_vec = Vec3::new(proposed_pos.0, proposed_pos.1, proposed_pos.2);
                if !cw.is_point_traversable(dest_vec) {
                    return Err(KinematicViolation::new(
                        current_pos,
                        KinematicViolationReason::ObstacleCollision {
                            hit_point: proposed_pos,
                            entity_id: None,
                        },
                    ));
                }
            }
        }

        state.velocity = (dx / dt_sec, dy / dt_sec, dz / dt_sec);
        state.last_position = proposed_pos;
        Ok(proposed_pos)
    }
}

/// 6-DoF Newtonian physics integrator checking acceleration and damping limits.
#[derive(Debug, Clone, Copy, Default)]
pub struct Newtonian6DoFIntegrator;

impl MotionIntegrator for Newtonian6DoFIntegrator {
    fn validate_step(
        &self,
        config: &KinematicConfig,
        state: &mut EntityKinematicState,
        proposed_pos: (f32, f32, f32),
        current_tick: u64,
        collision: Option<&dyn CollisionWorld>,
        mover_guid: Option<Guid>,
    ) -> Result<(f32, f32, f32), KinematicViolation> {
        let current_pos = state.last_position;
        if !proposed_pos.0.is_finite() || !proposed_pos.1.is_finite() || !proposed_pos.2.is_finite()
        {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::NonFiniteCoordinates,
            ));
        }

        let dt_sec = state.last_dt;
        if dt_sec <= 0.0 || !dt_sec.is_finite() {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::InvalidDeltaTime { dt: dt_sec },
            ));
        }

        let dx = proposed_pos.0 - current_pos.0;
        let dy = proposed_pos.1 - current_pos.1;
        let dz = proposed_pos.2 - current_pos.2;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();

        let mut bonus_dist = 0.0;
        if let Some(allowance) = state.impulse_allowance {
            if allowance.expires_at_tick >= current_tick {
                bonus_dist = allowance.bonus_distance;
            }
            state.impulse_allowance = None;
        }

        let effective_max_speed = config.max_speed + (bonus_dist / dt_sec);
        let speed = distance / dt_sec;
        if speed > effective_max_speed + 1e-4 {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::SpeedCapExceeded {
                    speed,
                    max_speed: effective_max_speed,
                },
            ));
        }

        let v_new = (dx / dt_sec, dy / dt_sec, dz / dt_sec);
        let dv = (
            v_new.0 - state.velocity.0,
            v_new.1 - state.velocity.1,
            v_new.2 - state.velocity.2,
        );
        let dv_mag = (dv.0 * dv.0 + dv.1 * dv.1 + dv.2 * dv.2).sqrt();
        let acceleration = dv_mag / dt_sec;

        let (max_acceleration, linear_damping) = match state.motion_mode {
            MotionMode::Newtonian6DoF {
                max_acceleration,
                linear_damping,
            } => (max_acceleration, linear_damping),
            _ => (50.0, 0.0),
        };

        let allowed_accel = max_acceleration * config.tolerance_multiplier;
        if acceleration > allowed_accel + 1e-4 {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::AccelerationCapExceeded {
                    acceleration,
                    max_acceleration,
                },
            ));
        }

        if distance > 1e-4 {
            if let Some(cw) = collision {
                let from_vec = Vec3::new(current_pos.0, current_pos.1, current_pos.2);
                let dir = Vec3::new(dx / distance, dy / distance, dz / distance);
                let filter = SpatialQueryFilter {
                    ignore_id: mover_guid,
                    collision_layers: u32::MAX,
                };

                let check_dist = (distance - 0.05).max(0.0);
                if check_dist > 0.0 {
                    if let Some(hit) = cw.raycast(from_vec, dir, check_dist, filter) {
                        if hit.distance < check_dist {
                            return Err(KinematicViolation::new(
                                current_pos,
                                KinematicViolationReason::ObstacleCollision {
                                    hit_point: (hit.point.x, hit.point.y, hit.point.z),
                                    entity_id: hit.entity_id,
                                },
                            ));
                        }
                    }
                }

                if state.collider_half_extents.0 > 0.0 {
                    let half = state.collider_half_extents;
                    let aabb = BoundingBox3D::new(
                        (
                            current_pos.0 - half.0,
                            current_pos.1 - half.1,
                            current_pos.2 - half.2,
                        ),
                        (
                            current_pos.0 + half.0,
                            current_pos.1 + half.1,
                            current_pos.2 + half.2,
                        ),
                    );
                    let translation = Vec3::new(dx, dy, dz);
                    if let Some(hit) = cw.sweep_test(&aabb, translation, filter) {
                        if hit.hit_fraction < 1.0 {
                            return Err(KinematicViolation::new(
                                current_pos,
                                KinematicViolationReason::WallClipping {
                                    hit_point: (hit.point.x, hit.point.y, hit.point.z),
                                    entity_id: hit.entity_id,
                                },
                            ));
                        }
                    }
                }

                let dest_vec = Vec3::new(proposed_pos.0, proposed_pos.1, proposed_pos.2);
                if !cw.is_point_traversable(dest_vec) {
                    return Err(KinematicViolation::new(
                        current_pos,
                        KinematicViolationReason::ObstacleCollision {
                            hit_point: proposed_pos,
                            entity_id: None,
                        },
                    ));
                }
            }
        }

        if linear_damping > 0.0 {
            let factor = (1.0 - linear_damping * dt_sec).max(0.0);
            state.velocity = (v_new.0 * factor, v_new.1 * factor, v_new.2 * factor);
        } else {
            state.velocity = v_new;
        }

        state.last_position = proposed_pos;
        Ok(proposed_pos)
    }
}

/// Represents a kinematic movement validation failure containing the snap-back destination.
#[derive(Debug, Clone, PartialEq)]
pub struct KinematicViolation {
    pub snap_back: (f32, f32, f32),
    pub reason: KinematicViolationReason,
}

impl KinematicViolation {
    /// Creates a new `KinematicViolation`.
    pub fn new(snap_back: (f32, f32, f32), reason: KinematicViolationReason) -> Self {
        Self { snap_back, reason }
    }

    /// Creates a new `WallClipping` kinematic violation.
    pub fn wall_clipping(
        snap_back: (f32, f32, f32),
        hit_point: (f32, f32, f32),
        entity_id: Option<Guid>,
    ) -> Self {
        Self::new(
            snap_back,
            KinematicViolationReason::WallClipping {
                hit_point,
                entity_id,
            },
        )
    }

    /// Creates a new `AccelerationCapExceeded` kinematic violation.
    pub fn acceleration_cap_exceeded(
        snap_back: (f32, f32, f32),
        acceleration: f32,
        max_acceleration: f32,
    ) -> Self {
        Self::new(
            snap_back,
            KinematicViolationReason::AccelerationCapExceeded {
                acceleration,
                max_acceleration,
            },
        )
    }

    /// Returns the snap-back destination coordinates.
    pub fn snap_back(&self) -> (f32, f32, f32) {
        self.snap_back
    }

    /// Returns a reference to the violation reason.
    pub fn reason(&self) -> &KinematicViolationReason {
        &self.reason
    }
}

impl fmt::Display for KinematicViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Kinematic violation: {} (snap back to {:?})",
            self.reason, self.snap_back
        )
    }
}

impl std::error::Error for KinematicViolation {}

/// Kinematic movement validator enforcing server-authoritative speed caps and sanity checks.
#[derive(Debug, Clone, PartialEq)]
pub struct KinematicValidator {
    pub max_speed: f32,
    pub max_teleport_distance: Option<f32>,
}

impl KinematicValidator {
    /// Default maximum speed in world units per second (e.g. 10.0 units/sec).
    pub const DEFAULT_MAX_SPEED: f32 = 10.0;

    /// Creates a new `KinematicValidator` with the given maximum speed in units/sec.
    pub fn new(max_speed: f32) -> Self {
        Self {
            max_speed,
            max_teleport_distance: None,
        }
    }

    /// Configures an optional maximum teleport distance threshold in a single move packet.
    pub fn with_teleport_distance(mut self, max_distance: f32) -> Self {
        self.max_teleport_distance = Some(max_distance);
        self
    }

    /// Grants a server-authorized impulse allowance to an entity kinematic state.
    pub fn grant_impulse(
        &self,
        state: &mut EntityKinematicState,
        bonus_distance: f32,
        duration_ticks: u64,
        current_tick: u64,
    ) {
        state.grant_impulse(bonus_distance, duration_ticks, current_tick);
    }

    /// Validates a proposed movement from `current_pos` to `new_pos` across `dt_sec`.
    ///
    /// Returns `Ok(())` if the movement is valid.
    /// Returns `Err(KinematicViolation)` containing `snap_back` set to `current_pos` on any violation.
    pub fn validate_move(
        &self,
        current_pos: (f32, f32, f32),
        new_pos: (f32, f32, f32),
        dt_sec: f32,
    ) -> Result<(), KinematicViolation> {
        let config = KinematicConfig {
            max_speed: self.max_speed,
            max_teleport_distance: self.max_teleport_distance,
            tolerance_multiplier: 1.1,
        };
        let mut temp_state = EntityKinematicState::new(std::time::Instant::now());
        temp_state.last_position = current_pos;
        temp_state.last_dt = dt_sec;
        DiscreteKinematicIntegrator.validate_step(
            &config,
            &mut temp_state,
            new_pos,
            0,
            None,
            None,
        )?;
        Ok(())
    }

    /// Validates a proposed movement against both kinematic speed/distance bounds,
    /// impulse allowances, volumetric sweep tests, and abstract collision geometry.
    ///
    /// Returns `Ok(())` if the trajectory is unobstructed and meets all kinematic constraints.
    /// Returns `Err(KinematicViolation)` snapping back to `current_pos` if an obstacle is struck.
    #[allow(clippy::too_many_arguments)]
    pub fn validate_move_with_collision(
        &self,
        current_pos: (f32, f32, f32),
        new_pos: (f32, f32, f32),
        dt_sec: f32,
        collision: &dyn CollisionWorld,
        mover_guid: Option<Guid>,
        mut state: Option<&mut EntityKinematicState>,
        current_tick: u64,
    ) -> Result<(), KinematicViolation> {
        let config = KinematicConfig {
            max_speed: self.max_speed,
            max_teleport_distance: self.max_teleport_distance,
            tolerance_multiplier: 1.1,
        };

        if let Some(ref mut s) = state {
            s.last_dt = dt_sec;
            s.last_position = current_pos;
            match s.motion_mode {
                MotionMode::DiscreteGround => {
                    DiscreteKinematicIntegrator.validate_step(
                        &config,
                        s,
                        new_pos,
                        current_tick,
                        Some(collision),
                        mover_guid,
                    )?;
                }
                MotionMode::Newtonian6DoF { .. } => {
                    Newtonian6DoFIntegrator.validate_step(
                        &config,
                        s,
                        new_pos,
                        current_tick,
                        Some(collision),
                        mover_guid,
                    )?;
                }
            }
        } else {
            let mut temp_state = EntityKinematicState::new(std::time::Instant::now());
            temp_state.last_position = current_pos;
            temp_state.last_dt = dt_sec;
            DiscreteKinematicIntegrator.validate_step(
                &config,
                &mut temp_state,
                new_pos,
                current_tick,
                Some(collision),
                mover_guid,
            )?;
        }

        Ok(())
    }

    /// Convenience 5-argument helper omitting tracking state for callers that only require
    /// basic obstacle collision and speed validation.
    pub fn validate_move_with_collision_simple(
        &self,
        current_pos: (f32, f32, f32),
        new_pos: (f32, f32, f32),
        dt_sec: f32,
        collision: &dyn CollisionWorld,
        mover_guid: Option<Guid>,
    ) -> Result<(), KinematicViolation> {
        self.validate_move_with_collision(
            current_pos,
            new_pos,
            dt_sec,
            collision,
            mover_guid,
            None,
            0,
        )
    }
}

impl Default for KinematicValidator {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_SPEED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_movement_pass() {
        let validator = KinematicValidator::new(10.0);
        let start = (0.0, 0.0, 0.0);
        // Move 4 units in 0.5s -> speed = 8.0 units/s <= 10.0
        let target = (4.0, 0.0, 0.0);
        assert!(validator.validate_move(start, target, 0.5).is_ok());
    }

    #[test]
    fn test_exact_max_speed_boundary() {
        let validator = KinematicValidator::new(10.0);
        let start = (0.0, 0.0, 0.0);
        // Move 0.5 units in 0.05s (20Hz tick) -> speed = 10.0 units/s
        let target = (0.5, 0.0, 0.0);
        assert!(validator.validate_move(start, target, 0.05).is_ok());
    }

    #[test]
    fn test_stationary_move_pass() {
        let validator = KinematicValidator::new(10.0);
        let start = (15.0, -20.0, 3.5);
        assert!(validator.validate_move(start, start, 0.05).is_ok());
    }

    #[test]
    fn test_speed_hack_rejection_and_snapback() {
        let validator = KinematicValidator::new(10.0);
        let start = (10.0, 10.0, 0.0);
        // Move 10 units in 0.05s -> speed = 200.0 units/s > 10.0
        let target = (20.0, 10.0, 0.0);
        let result = validator.validate_move(start, target, 0.05);

        assert!(result.is_err());
        let violation = result.unwrap_err();
        assert_eq!(violation.snap_back(), start);
        match violation.reason() {
            KinematicViolationReason::SpeedCapExceeded { speed, max_speed } => {
                assert!(*speed > 10.0);
                assert_eq!(*max_speed, 10.0);
            }
            other => panic!("expected SpeedCapExceeded, got {other:?}"),
        }
    }

    #[test]
    fn test_zero_and_negative_dt_rejection() {
        let validator = KinematicValidator::new(10.0);
        let start = (0.0, 0.0, 0.0);
        let target = (1.0, 0.0, 0.0);

        let res_zero = validator.validate_move(start, target, 0.0);
        assert!(res_zero.is_err());
        assert_eq!(res_zero.unwrap_err().snap_back(), start);

        let res_neg = validator.validate_move(start, target, -0.05);
        assert!(res_neg.is_err());
        assert_eq!(res_neg.unwrap_err().snap_back(), start);
    }

    #[test]
    fn test_nan_and_inf_rejection() {
        let validator = KinematicValidator::new(10.0);
        let start = (0.0, 0.0, 0.0);

        let res_nan = validator.validate_move(start, (f32::NAN, 0.0, 0.0), 0.05);
        assert!(res_nan.is_err());
        assert_eq!(res_nan.unwrap_err().snap_back(), start);

        let res_inf = validator.validate_move(start, (0.0, f32::INFINITY, 0.0), 0.05);
        assert!(res_inf.is_err());
        assert_eq!(res_inf.unwrap_err().snap_back(), start);
    }

    #[test]
    fn test_teleport_detection_even_with_large_dt() {
        let validator = KinematicValidator::new(10.0).with_teleport_distance(50.0);
        let start = (0.0, 0.0, 0.0);
        // Distance 100.0, dt = 20.0s -> speed = 5.0 (below max_speed 10.0), but distance > 50.0
        let target = (100.0, 0.0, 0.0);
        let result = validator.validate_move(start, target, 20.0);

        assert!(result.is_err());
        let violation = result.unwrap_err();
        assert_eq!(violation.snap_back(), start);
        match violation.reason() {
            KinematicViolationReason::TeleportDetected {
                distance,
                max_distance,
            } => {
                assert_eq!(*distance, 100.0);
                assert_eq!(*max_distance, 50.0);
            }
            other => panic!("expected TeleportDetected, got {other:?}"),
        }
    }

    #[test]
    fn test_validate_move_with_collision_unobstructed_and_obstructed() {
        use mmo_core::collision::AabbCollisionWorld;
        use mmo_core::octree::BoundingBox3D;

        let validator = KinematicValidator::new(10.0);
        let mut collision = AabbCollisionWorld::new();
        let obstacle = BoundingBox3D::new((-1.0, -1.0, -1.0), (1.0, 1.0, 1.0));
        let obstacle_guid = Guid(42);
        collision.register_obstacle(obstacle_guid, obstacle);

        let start = (-3.0, 0.0, 0.0);
        // Clear move to (-2.0, 0, 0)
        let clear_target = (-2.0, 0.0, 0.0);
        let res_clear = validator.validate_move_with_collision_simple(
            start,
            clear_target,
            0.5,
            &collision,
            None,
        );
        assert!(res_clear.is_ok());

        // Obstructed move straight through the box from -3.0 to +3.0
        let obstructed_target = (3.0, 0.0, 0.0);
        let res_obstructed = validator.validate_move_with_collision_simple(
            start,
            obstructed_target,
            1.0,
            &collision,
            None,
        );
        assert!(res_obstructed.is_err());
        let violation = res_obstructed.unwrap_err();
        assert_eq!(violation.snap_back(), start);
        match violation.reason() {
            KinematicViolationReason::ObstacleCollision {
                hit_point,
                entity_id,
            } => {
                assert!((hit_point.0 - (-1.0)).abs() < 1e-4);
                assert_eq!(*entity_id, Some(obstacle_guid));
            }
            other => panic!("expected ObstacleCollision, got {other:?}"),
        }

        // Destination inside box
        let inside_target = (0.0, 0.0, 0.0);
        let res_inside = validator.validate_move_with_collision_simple(
            start,
            inside_target,
            0.5,
            &collision,
            None,
        );
        assert!(res_inside.is_err());
    }

    #[test]
    fn test_impulse_allowance_knockback_exemption() {
        use mmo_core::collision::AabbCollisionWorld;

        let validator = KinematicValidator::new(7.0).with_teleport_distance(15.0);
        let collision = AabbCollisionWorld::new();
        let mut state = EntityKinematicState::new(std::time::Instant::now());

        // Grant impulse allowance of 25m for 5 ticks
        validator.grant_impulse(&mut state, 25.0, 5, 100);
        assert!(state.impulse_allowance.is_some());

        let start = (0.0, 0.0, 0.0);
        let knockback_dest = (30.0, 0.0, 0.0); // 30m displacement in 1 second

        // Movement with active impulse allowance should succeed
        let res = validator.validate_move_with_collision(
            start,
            knockback_dest,
            1.0,
            &collision,
            None,
            Some(&mut state),
            101, // Tick within 100 + 5
        );
        assert!(res.is_ok(), "Knockback within impulse allowance must pass");
        assert_eq!(state.last_position, knockback_dest);
        assert!(
            state.impulse_allowance.is_none(),
            "Impulse allowance must be consumed"
        );

        // Subsequent 30m move without allowance must be rejected
        let res_subsequent = validator.validate_move_with_collision(
            knockback_dest,
            (60.0, 0.0, 0.0),
            1.0,
            &collision,
            None,
            Some(&mut state),
            102,
        );
        assert!(
            res_subsequent.is_err(),
            "Displacement without impulse allowance must fail"
        );
    }

    #[test]
    fn test_newtonian_6dof_acceleration_validation() {
        use mmo_core::collision::AabbCollisionWorld;

        let validator = KinematicValidator::new(100.0);
        let collision = AabbCollisionWorld::new();
        let mut state = EntityKinematicState::new(std::time::Instant::now());
        state.motion_mode = MotionMode::Newtonian6DoF {
            max_acceleration: 20.0,
            linear_damping: 0.0,
        };

        let start = (0.0, 0.0, 0.0);
        // Step 1: Accelerate from rest (velocity = 0) to 15 m/s in 1.0s (accel = 15 <= 20 * 1.1)
        let step1 = (15.0, 0.0, 0.0);
        let res1 = validator.validate_move_with_collision(
            start,
            step1,
            1.0,
            &collision,
            None,
            Some(&mut state),
            1,
        );
        assert!(res1.is_ok(), "Acceleration 15 m/s^2 <= 20 m/s^2 must pass");
        assert_eq!(state.velocity, (15.0, 0.0, 0.0));

        // Step 2: Accelerate from 15 m/s to 50 m/s in 1.0s (dv = 35 m/s / 1.0s = 35 > 22.0)
        let step2 = (65.0, 0.0, 0.0); // 15 + 50 = 65
        let res2 = validator.validate_move_with_collision(
            step1,
            step2,
            1.0,
            &collision,
            None,
            Some(&mut state),
            2,
        );
        assert!(
            res2.is_err(),
            "Acceleration 35 m/s^2 > 22 m/s^2 must be rejected"
        );
        match res2.unwrap_err().reason() {
            KinematicViolationReason::AccelerationCapExceeded {
                acceleration,
                max_acceleration,
            } => {
                assert!((*acceleration - 35.0).abs() < 1e-4);
                assert_eq!(*max_acceleration, 20.0);
            }
            other => panic!("expected AccelerationCapExceeded, got {other:?}"),
        }
    }

    #[test]
    fn test_volumetric_sweep_test_wall_clipping_rejection() {
        use mmo_core::collision::AabbCollisionWorld;
        use mmo_core::octree::BoundingBox3D;

        let validator = KinematicValidator::new(10.0);
        let mut collision = AabbCollisionWorld::new();
        // Obstacle from (-1, -1, -1) to (1, 1, 1)
        let obstacle = BoundingBox3D::new((-1.0, -1.0, -1.0), (1.0, 1.0, 1.0));
        collision.register_obstacle(Guid(10), obstacle);

        let mut state = EntityKinematicState::new(std::time::Instant::now());
        // Character height 1.8m (half_extent Y = 0.9m)
        state.collider_half_extents = (0.4, 0.9, 0.4);

        // Center ray at Y=1.5 moves from X=-3 to X=3 above the obstacle (obstacle top is Y=1.0)
        // Raycast alone would NOT hit because Y=1.5 > 1.0!
        // BUT character bounds extend down to Y = 1.5 - 0.9 = 0.6 <= 1.0, clipping into obstacle!
        let start = (-3.0, 1.5, 0.0);
        let target = (3.0, 1.5, 0.0);

        let res = validator.validate_move_with_collision(
            start,
            target,
            1.0,
            &collision,
            None,
            Some(&mut state),
            1,
        );
        assert!(res.is_err(), "Volumetric sweep test must catch clipping");
        let violation = res.unwrap_err();
        match violation.reason() {
            KinematicViolationReason::WallClipping { hit_point, .. } => {
                assert!(hit_point.0 <= 1.0 && hit_point.0 >= -1.5);
            }
            other => panic!("expected WallClipping, got {other:?}"),
        }
    }

    #[test]
    fn test_mutation_killing_boundary_kinematics_coverage() {
        use mmo_core::collision::AabbCollisionWorld;
        use mmo_core::octree::BoundingBox3D;

        // 1. KinematicConfig and Default
        let cfg = KinematicConfig::default();
        assert_eq!(cfg.max_speed, 10.0);
        assert_eq!(cfg.max_teleport_distance, None);
        assert!((cfg.tolerance_multiplier - 1.1).abs() < 1e-4);

        // 2. KinematicViolationReason Display formatters
        assert!(format!(
            "{}",
            KinematicViolationReason::SpeedCapExceeded {
                speed: 12.0,
                max_speed: 10.0
            }
        )
        .contains("speed cap exceeded"));
        assert!(format!(
            "{}",
            KinematicViolationReason::TeleportDetected {
                distance: 50.0,
                max_distance: 20.0
            }
        )
        .contains("teleport detected"));
        assert!(format!(
            "{}",
            KinematicViolationReason::InvalidDeltaTime { dt: -0.1 }
        )
        .contains("invalid delta time"));
        assert!(
            format!("{}", KinematicViolationReason::NonFiniteCoordinates)
                .contains("non-finite coordinates")
        );
        assert!(format!(
            "{}",
            KinematicViolationReason::NonMonotonicSequence {
                seq: 1,
                last_seq: 2
            }
        )
        .contains("non-monotonic sequence"));
        assert!(format!(
            "{}",
            KinematicViolationReason::ObstacleCollision {
                hit_point: (1.0, 2.0, 3.0),
                entity_id: Some(Guid(5))
            }
        )
        .contains("obstacle collision detected"));
        assert!(format!(
            "{}",
            KinematicViolationReason::WallClipping {
                hit_point: (1.0, 2.0, 3.0),
                entity_id: None
            }
        )
        .contains("wall clipping detected"));
        assert!(format!(
            "{}",
            KinematicViolationReason::AccelerationCapExceeded {
                acceleration: 30.0,
                max_acceleration: 20.0
            }
        )
        .contains("acceleration cap exceeded"));

        // 3. KinematicViolation constructors & Display
        let v_clip =
            KinematicViolation::wall_clipping((0.0, 0.0, 0.0), (1.0, 1.0, 1.0), Some(Guid(7)));
        assert_eq!(v_clip.snap_back(), (0.0, 0.0, 0.0));
        assert!(matches!(
            v_clip.reason(),
            KinematicViolationReason::WallClipping { .. }
        ));
        assert!(format!("{v_clip}").contains("Kinematic violation"));

        let v_accel = KinematicViolation::acceleration_cap_exceeded((1.0, 1.0, 1.0), 30.0, 20.0);
        assert_eq!(v_accel.snap_back(), (1.0, 1.0, 1.0));
        assert!(matches!(
            v_accel.reason(),
            KinematicViolationReason::AccelerationCapExceeded { .. }
        ));

        // 4. EntityKinematicState with_position & grant_impulse
        let mut state =
            EntityKinematicState::new(std::time::Instant::now()).with_position((5.0, 5.0, 5.0));
        assert_eq!(state.last_position, (5.0, 5.0, 5.0));
        state.grant_impulse(10.0, 5, 100);
        assert_eq!(
            state.impulse_allowance,
            Some(ImpulseAllowance {
                bonus_distance: 10.0,
                expires_at_tick: 105
            })
        );

        // 5. KinematicValidator with_teleport_distance, grant_impulse, and Default
        let validator = KinematicValidator::default().with_teleport_distance(15.0);
        assert_eq!(validator.max_speed, 10.0);
        assert_eq!(validator.max_teleport_distance, Some(15.0));
        validator.grant_impulse(&mut state, 20.0, 10, 200);
        assert_eq!(
            state.impulse_allowance,
            Some(ImpulseAllowance {
                bonus_distance: 20.0,
                expires_at_tick: 210
            })
        );

        // 6. Impulse allowance expiration boundary
        let mut col = AabbCollisionWorld::new();
        col.register_obstacle(
            Guid(99),
            BoundingBox3D::new((100.0, 100.0, 100.0), (101.0, 101.0, 101.0)),
        );
        // Tick 210: allowance still valid
        let res_valid = validator.validate_move_with_collision(
            (0.0, 0.0, 0.0),
            (25.0, 0.0, 0.0),
            1.0,
            &col,
            None,
            Some(&mut state),
            210,
        );
        assert!(res_valid.is_ok());
        assert_eq!(state.impulse_allowance, None);

        // Expired impulse (tick 211 > 210)
        state.grant_impulse(20.0, 10, 200); // expires at 210
        let res_expired = validator.validate_move_with_collision(
            (0.0, 0.0, 0.0),
            (25.0, 0.0, 0.0),
            1.0,
            &col,
            None,
            Some(&mut state),
            211,
        );
        assert!(res_expired.is_err());

        // 7. validate_move_with_collision_simple
        assert!(validator
            .validate_move_with_collision_simple((0.0, 0.0, 0.0), (5.0, 0.0, 0.0), 1.0, &col, None)
            .is_ok());
        assert!(validator
            .validate_move_with_collision_simple((0.0, 0.0, 0.0), (20.0, 0.0, 0.0), 1.0, &col, None)
            .is_err());

        // 8. DiscreteKinematicIntegrator step validation
        let integrator = DiscreteKinematicIntegrator;
        let mut disc_state = EntityKinematicState::new(std::time::Instant::now());
        disc_state.last_position = (0.0, 0.0, 0.0);
        disc_state.last_dt = 0.1;
        let disc_cfg = KinematicConfig {
            max_speed: 10.0,
            max_teleport_distance: Some(15.0),
            tolerance_multiplier: 1.1,
        };
        assert!(integrator
            .validate_step(
                &disc_cfg,
                &mut disc_state,
                (f32::NAN, 0.0, 0.0),
                1,
                None,
                None
            )
            .is_err());
        assert!(integrator
            .validate_step(
                &disc_cfg,
                &mut disc_state,
                (0.0, f32::INFINITY, 0.0),
                1,
                None,
                None
            )
            .is_err());
        assert!(integrator
            .validate_step(
                &disc_cfg,
                &mut disc_state,
                (0.0, 0.0, f32::NEG_INFINITY),
                1,
                None,
                None
            )
            .is_err());
        disc_state.last_dt = -1.0;
        assert!(matches!(
            integrator.validate_step(&disc_cfg, &mut disc_state, (0.1, 0.0, 0.0), 1, None, None),
            Err(KinematicViolation {
                reason: KinematicViolationReason::InvalidDeltaTime { .. },
                ..
            })
        ));
        disc_state.last_dt = 0.0;
        assert!(matches!(
            integrator.validate_step(&disc_cfg, &mut disc_state, (0.1, 0.0, 0.0), 1, None, None),
            Err(KinematicViolation {
                reason: KinematicViolationReason::InvalidDeltaTime { .. },
                ..
            })
        ));
        disc_state.last_dt = f32::NAN;
        assert!(matches!(
            integrator.validate_step(&disc_cfg, &mut disc_state, (0.1, 0.0, 0.0), 1, None, None),
            Err(KinematicViolation {
                reason: KinematicViolationReason::InvalidDeltaTime { .. },
                ..
            })
        ));
        disc_state.last_dt = 0.05;
        assert!(integrator
            .validate_step(&disc_cfg, &mut disc_state, (0.1, 0.0, 0.0), 1, None, None)
            .is_ok());
        assert_eq!(disc_state.last_position, (0.1, 0.0, 0.0));

        // 9. Newtonian6DoF linear damping check
        let newton_integrator = Newtonian6DoFIntegrator;
        let mut newton_state = EntityKinematicState::new(std::time::Instant::now());
        newton_state.last_position = (0.0, 0.0, 0.0);
        newton_state.velocity = (0.0, 0.0, 0.0);
        newton_state.last_dt = 1.0;
        newton_state.motion_mode = MotionMode::Newtonian6DoF {
            max_acceleration: 20.0,
            linear_damping: 0.5,
        };
        let newton_cfg = KinematicConfig {
            max_speed: 100.0,
            max_teleport_distance: None,
            tolerance_multiplier: 1.1,
        };
        let pos = newton_integrator
            .validate_step(
                &newton_cfg,
                &mut newton_state,
                (10.0, 0.0, 0.0),
                1,
                None,
                None,
            )
            .unwrap();
        assert_eq!(pos, (10.0, 0.0, 0.0));
        // factor = 1.0 - 0.5 * 1.0 = 0.5 => velocity.0 = 10.0 * 0.5 = 5.0
        assert!((newton_state.velocity.0 - 5.0).abs() < 1e-4);

        // 10. Exhaustive 3D math, boundary, and collision coverage for DiscreteKinematicIntegrator
        let mut full_state = EntityKinematicState::new(std::time::Instant::now());
        full_state.last_position = (10.0, 20.0, 30.0);
        full_state.last_dt = 1.0;
        let full_cfg = KinematicConfig {
            max_speed: 10.0,
            max_teleport_distance: Some(15.0),
            tolerance_multiplier: 1.1,
        };

        // dx=2, dy=3, dz=6 -> distance = 7.0, speed = 7.0 <= 10.0
        let target_3d = (12.0, 23.0, 36.0);
        let ok_res = integrator.validate_step(&full_cfg, &mut full_state, target_3d, 1, None, None);
        assert!(ok_res.is_ok());
        assert_eq!(full_state.last_position, target_3d);
        assert!((full_state.velocity.0 - 2.0).abs() < 1e-4);
        assert!((full_state.velocity.1 - 3.0).abs() < 1e-4);
        assert!((full_state.velocity.2 - 6.0).abs() < 1e-4);

        // Stationary move (distance == 0.0 <= 1e-4)
        let stat_res =
            integrator.validate_step(&full_cfg, &mut full_state, target_3d, 1, None, None);
        assert!(stat_res.is_ok());

        // Speed cap exact boundary check
        full_state.last_dt = 0.7; // 7.0 / 0.7 = 10.0 m/s == max_speed
        full_state.last_position = (10.0, 20.0, 30.0);
        assert!(integrator
            .validate_step(&full_cfg, &mut full_state, target_3d, 1, None, None)
            .is_ok());

        // Speed cap exceeded check
        full_state.last_position = (10.0, 20.0, 30.0);
        full_state.last_dt = 0.5; // 7.0 / 0.5 = 14.0 m/s > 10.0
        let speed_err = integrator
            .validate_step(&full_cfg, &mut full_state, target_3d, 1, None, None)
            .unwrap_err();
        assert!(matches!(
            speed_err.reason(),
            KinematicViolationReason::SpeedCapExceeded { .. }
        ));

        // Teleport detected check
        full_state.last_position = (10.0, 20.0, 30.0);
        full_state.last_dt = 10.0;
        let far_target = (10.0 + 20.0, 20.0, 30.0); // distance = 20.0 > 15.0 max_teleport
        let tele_err = integrator
            .validate_step(&full_cfg, &mut full_state, far_target, 1, None, None)
            .unwrap_err();
        assert!(matches!(
            tele_err.reason(),
            KinematicViolationReason::TeleportDetected { .. }
        ));

        // 11. Exhaustive Newtonian6DoFIntegrator validation
        let mut newton_full = EntityKinematicState::new(std::time::Instant::now());
        newton_full.last_position = (10.0, 20.0, 30.0);
        newton_full.velocity = (1.0, 2.0, 3.0);
        newton_full.last_dt = 1.0;
        newton_full.motion_mode = MotionMode::Newtonian6DoF {
            max_acceleration: 10.0,
            linear_damping: 0.0,
        };
        let newton_cfg_full = KinematicConfig {
            max_speed: 100.0,
            max_teleport_distance: None,
            tolerance_multiplier: 1.1, // allowed_accel = 11.0
        };

        // Non-finite proposed coords
        assert!(newton_integrator
            .validate_step(
                &newton_cfg_full,
                &mut newton_full,
                (f32::NAN, 20.0, 30.0),
                1,
                None,
                None
            )
            .is_err());
        assert!(newton_integrator
            .validate_step(
                &newton_cfg_full,
                &mut newton_full,
                (10.0, f32::NAN, 30.0),
                1,
                None,
                None
            )
            .is_err());
        assert!(newton_integrator
            .validate_step(
                &newton_cfg_full,
                &mut newton_full,
                (10.0, 20.0, f32::NAN),
                1,
                None,
                None
            )
            .is_err());

        // Invalid dt in Newtonian
        newton_full.last_dt = 0.0;
        assert!(newton_integrator
            .validate_step(&newton_cfg_full, &mut newton_full, target_3d, 1, None, None)
            .is_err());
        newton_full.last_dt = -0.5;
        assert!(newton_integrator
            .validate_step(&newton_cfg_full, &mut newton_full, target_3d, 1, None, None)
            .is_err());
        newton_full.last_dt = f32::NAN;
        assert!(newton_integrator
            .validate_step(&newton_cfg_full, &mut newton_full, target_3d, 1, None, None)
            .is_err());

        // Valid acceleration within tolerance
        newton_full.last_dt = 1.0;
        // target_3d -> v_new = (2, 3, 6). dv = (1, 1, 3) -> dv_mag = sqrt(11) ≈ 3.316 <= 11.0
        assert!(newton_integrator
            .validate_step(&newton_cfg_full, &mut newton_full, target_3d, 1, None, None)
            .is_ok());

        // Acceleration cap exceeded
        newton_full.last_position = (10.0, 20.0, 30.0);
        newton_full.velocity = (0.0, 0.0, 0.0);
        let accel_target = (10.0 + 50.0, 20.0, 30.0); // v_new = (50, 0, 0), dv = 50 > 11.0
        let accel_err = newton_integrator
            .validate_step(
                &newton_cfg_full,
                &mut newton_full,
                accel_target,
                1,
                None,
                None,
            )
            .unwrap_err();
        assert!(matches!(
            accel_err.reason(),
            KinematicViolationReason::AccelerationCapExceeded { .. }
        ));

        // Clamped linear damping (1.0 - linear_damping * dt < 0 -> factor = 0)
        newton_full.motion_mode = MotionMode::Newtonian6DoF {
            max_acceleration: 100.0,
            linear_damping: 5.0, // dt = 1.0 -> 1.0 - 5.0 = -4.0 clamped to 0.0
        };
        newton_full.last_position = (0.0, 0.0, 0.0);
        newton_full.velocity = (0.0, 0.0, 0.0);
        let clamped_res = newton_integrator
            .validate_step(
                &newton_cfg_full,
                &mut newton_full,
                (5.0, 0.0, 0.0),
                1,
                None,
                None,
            )
            .unwrap();
        assert_eq!(clamped_res, (5.0, 0.0, 0.0));
        assert_eq!(newton_full.velocity, (0.0, 0.0, 0.0));
    }
}
