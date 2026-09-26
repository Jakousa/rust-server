//! Kinematic validation module for player movement sanity checking and anti-cheat.
//!
//! Validates client-reported coordinates against maximum speed limits, delta time bounds,
//! finite float values, and optional teleport distance thresholds, returning a snap-back
//! position to correct invalid movement.

use std::fmt;

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
        }
    }
}

/// Server-side monotonic kinematic tracking state for an active entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityKinematicState {
    pub last_move_time: std::time::Instant,
    pub last_sequence: u32,
    pub initialized: bool,
}

impl EntityKinematicState {
    /// Creates a new uninitialized `EntityKinematicState` anchored to `initial_time`.
    pub fn new(initial_time: std::time::Instant) -> Self {
        Self {
            last_move_time: initial_time,
            last_sequence: 0,
            initialized: false,
        }
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
    /// Small tolerance to prevent floating point inaccuracies from triggering false positives.
    const EPSILON: f32 = 1e-4;

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
        // Guard against NaN / Infinite coordinates
        if !new_pos.0.is_finite() || !new_pos.1.is_finite() || !new_pos.2.is_finite() {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::NonFiniteCoordinates,
            ));
        }

        // Guard against non-positive or non-finite delta time
        if dt_sec <= 0.0 || !dt_sec.is_finite() {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::InvalidDeltaTime { dt: dt_sec },
            ));
        }

        let dx = new_pos.0 - current_pos.0;
        let dy = new_pos.1 - current_pos.1;
        let dz = new_pos.2 - current_pos.2;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();

        // Check teleport distance threshold if enabled
        if let Some(max_dist) = self.max_teleport_distance {
            if distance > max_dist + Self::EPSILON {
                return Err(KinematicViolation::new(
                    current_pos,
                    KinematicViolationReason::TeleportDetected {
                        distance,
                        max_distance: max_dist,
                    },
                ));
            }
        }

        // Check speed cap
        let speed = distance / dt_sec;
        if speed > self.max_speed + Self::EPSILON {
            return Err(KinematicViolation::new(
                current_pos,
                KinematicViolationReason::SpeedCapExceeded {
                    speed,
                    max_speed: self.max_speed,
                },
            ));
        }

        Ok(())
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
}
