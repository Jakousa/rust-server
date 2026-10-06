//! Fixed-point kinematic quantization and dequantization utilities.
//!
//! Provides deterministic mapping between 32-bit floating-point coordinates,
//! velocities, and Euler rotations to compact discrete integers with sub-centimeter
//! and sub-degree accuracy.

use std::f32::consts::PI;

/// Scale factor for position deltas: 128.0 units per meter (1/128m ≈ 7.81mm resolution).
pub const DELTA_POS_SCALE: f32 = 128.0;

/// Maximum safe displacement delta (in meters) before forcing a full keyframe.
/// Corresponds to 255.0 meters (under signed 16-bit capacity of 255.992m).
pub const MAX_DELTA_POS: f32 = 255.0;

/// Scale factor for velocities: 100.0 units per m/s (1 cm/s resolution).
pub const VELOCITY_SCALE: f32 = 100.0;

/// Checks whether a 3D displacement vector exceeds the representable delta range,
/// indicating teleportation, respawn, or extreme knockback that requires a keyframe.
#[inline]
pub fn is_teleport_delta(dx: f32, dy: f32, dz: f32) -> bool {
    if !dx.is_finite() || !dy.is_finite() || !dz.is_finite() {
        return true;
    }
    dx.abs() > MAX_DELTA_POS || dy.abs() > MAX_DELTA_POS || dz.abs() > MAX_DELTA_POS
}

/// Quantizes a 1D position delta (in meters) into a signed 16-bit integer.
///
/// Clamps to `[i16::MIN, i16::MAX]` without arithmetic overflow or panicking on NaN/Inf.
#[inline]
pub fn quantize_delta_i16(val: f32) -> i16 {
    if val.is_nan() {
        return 0;
    }
    let scaled = (val * DELTA_POS_SCALE).round();
    scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Dequantizes a signed 16-bit integer back to a 1D position delta (in meters).
#[inline]
pub fn dequantize_delta_i16(quantized: i16) -> f32 {
    quantized as f32 / DELTA_POS_SCALE
}

/// Quantizes a 3D position delta (in meters) into a tuple of three signed 16-bit integers.
#[inline]
pub fn quantize_delta_vec3(dx: f32, dy: f32, dz: f32) -> (i16, i16, i16) {
    (
        quantize_delta_i16(dx),
        quantize_delta_i16(dy),
        quantize_delta_i16(dz),
    )
}

/// Dequantizes three signed 16-bit integers back into a 3D position delta.
#[inline]
pub fn dequantize_delta_vec3(qx: i16, qy: i16, qz: i16) -> (f32, f32, f32) {
    (
        dequantize_delta_i16(qx),
        dequantize_delta_i16(qy),
        dequantize_delta_i16(qz),
    )
}

/// Quantizes a yaw angle (in radians) into an 8-bit unsigned integer (256 discrete angles, ~1.41°).
#[inline]
pub fn quantize_yaw_u8(yaw: f32) -> u8 {
    if yaw.is_nan() {
        return 0;
    }
    let two_pi = 2.0 * PI;
    let normalized = yaw.rem_euclid(two_pi);
    let step = 256.0 / two_pi;
    let val = (normalized * step).round();
    (val as u32 % 256) as u8
}

/// Dequantizes an 8-bit unsigned integer back into a yaw angle in radians [0, 2π).
#[inline]
pub fn dequantize_yaw_u8(quantized: u8) -> f32 {
    let two_pi = 2.0 * PI;
    (quantized as f32 / 256.0) * two_pi
}

/// Quantizes a pitch angle (in radians, nominally [-π/2, π/2]) into an 8-bit signed integer.
#[inline]
pub fn quantize_pitch_i8(pitch: f32) -> i8 {
    if pitch.is_nan() {
        return 0;
    }
    let half_pi = PI * 0.5;
    let clamped = pitch.clamp(-half_pi, half_pi);
    let step = 127.0 / half_pi;
    (clamped * step).round().clamp(-128.0, 127.0) as i8
}

/// Dequantizes an 8-bit signed integer back into a pitch angle in radians [-π/2, π/2].
#[inline]
pub fn dequantize_pitch_i8(quantized: i8) -> f32 {
    let half_pi = PI * 0.5;
    (quantized as f32 / 127.0) * half_pi
}

/// Quantizes a 1D velocity component (in m/s) into a signed 16-bit integer (1 cm/s resolution).
#[inline]
pub fn quantize_velocity_i16(val: f32) -> i16 {
    if val.is_nan() {
        return 0;
    }
    let scaled = (val * VELOCITY_SCALE).round();
    scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Dequantizes a signed 16-bit integer back to a 1D velocity component (in m/s).
#[inline]
pub fn dequantize_velocity_i16(quantized: i16) -> f32 {
    quantized as f32 / VELOCITY_SCALE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_quantization_precision() {
        let original_deltas = [
            0.0, 0.0078125, // exact 1/128
            0.5, -0.5, 12.345, -78.91, 254.99, -254.99,
        ];

        for &val in &original_deltas {
            let q = quantize_delta_i16(val);
            let reconstructed = dequantize_delta_i16(q);
            let error = (val - reconstructed).abs();
            assert!(
                error <= 0.008,
                "Delta error {error} exceeds 8mm tolerance for value {val}"
            );
        }
    }

    #[test]
    fn test_yaw_quantization_precision() {
        let angles = [
            0.0,
            PI * 0.25,
            PI * 0.5,
            PI,
            PI * 1.5,
            PI * 1.99,
            -PI * 0.5, // should wrap to 1.5 * PI
            -PI,
        ];

        for &val in &angles {
            let q = quantize_yaw_u8(val);
            let reconstructed = dequantize_yaw_u8(q);
            // Angle difference modulo 2*PI
            let diff = (val.rem_euclid(2.0 * PI) - reconstructed).abs();
            let angular_error = diff.min((2.0 * PI - diff).abs());
            let max_error_rad = (1.5 / 180.0) * PI; // 1.5 degrees
            assert!(
                angular_error <= max_error_rad,
                "Yaw angular error {angular_error} exceeds 1.5° for angle {val}"
            );
        }
    }

    #[test]
    fn test_pitch_quantization_precision() {
        let angles = [-PI * 0.5, -PI * 0.25, 0.0, PI * 0.25, PI * 0.5];

        for &val in &angles {
            let q = quantize_pitch_i8(val);
            let reconstructed = dequantize_pitch_i8(q);
            let error = (val - reconstructed).abs();
            let max_error_rad = (1.5 / 180.0) * PI;
            assert!(
                error <= max_error_rad,
                "Pitch error {error} exceeds 1.5° for pitch {val}"
            );
        }
    }

    #[test]
    fn test_velocity_quantization_precision() {
        let velocities = [0.0, 0.01, 1.5, -5.25, 25.0, -45.0];

        for &val in &velocities {
            let q = quantize_velocity_i16(val);
            let reconstructed = dequantize_velocity_i16(q);
            let error = (val - reconstructed).abs();
            assert!(
                error <= 0.01,
                "Velocity error {error} exceeds 1 cm/s tolerance for {val}"
            );
        }
    }

    #[test]
    fn test_teleport_detection_and_clamping() {
        assert!(!is_teleport_delta(10.0, 5.0, -2.0));
        assert!(is_teleport_delta(256.0, 0.0, 0.0));
        assert!(is_teleport_delta(0.0, -300.0, 0.0));
        assert!(is_teleport_delta(0.0, 0.0, 1000.0));

        // Non-finite (NaN / Infinity) coordinates must force teleport / keyframe fallback
        assert!(is_teleport_delta(f32::NAN, 0.0, 0.0));
        assert!(is_teleport_delta(0.0, f32::NAN, 0.0));
        assert!(is_teleport_delta(0.0, 0.0, f32::NAN));
        assert!(is_teleport_delta(f32::INFINITY, 0.0, 0.0));
        assert!(is_teleport_delta(0.0, f32::NEG_INFINITY, 0.0));

        // Exact boundary checks for teleport threshold
        assert!(!is_teleport_delta(255.0, 0.0, 0.0));
        assert!(!is_teleport_delta(0.0, 255.0, 0.0));
        assert!(!is_teleport_delta(0.0, 0.0, 255.0));
        assert!(!is_teleport_delta(-255.0, 0.0, 0.0));
        assert!(!is_teleport_delta(0.0, -255.0, 0.0));
        assert!(!is_teleport_delta(0.0, 0.0, -255.0));
        assert!(is_teleport_delta(255.01, 0.0, 0.0));
        assert!(is_teleport_delta(0.0, 255.01, 0.0));
        assert!(is_teleport_delta(0.0, 0.0, 255.01));
        assert!(is_teleport_delta(-255.01, 0.0, 0.0));
        assert!(is_teleport_delta(0.0, -255.01, 0.0));
        assert!(is_teleport_delta(0.0, 0.0, -255.01));

        // Clamping bounds
        let q_max = quantize_delta_i16(1000.0);
        assert_eq!(q_max, i16::MAX);
        let q_min = quantize_delta_i16(-1000.0);
        assert_eq!(q_min, i16::MIN);

        // NaN handling
        assert_eq!(quantize_delta_i16(f32::NAN), 0);
        assert_eq!(quantize_yaw_u8(f32::NAN), 0);
        assert_eq!(quantize_pitch_i8(f32::NAN), 0);
        assert_eq!(quantize_velocity_i16(f32::NAN), 0);
    }

    #[test]
    fn test_delta_vec3_quantization_precision() {
        let (qx, qy, qz) = quantize_delta_vec3(12.5, -45.25, 100.125);
        assert_eq!(qx, quantize_delta_i16(12.5));
        assert_eq!(qy, quantize_delta_i16(-45.25));
        assert_eq!(qz, quantize_delta_i16(100.125));
        assert_ne!((qx, qy, qz), (0, 0, 0));

        let (rx, ry, rz) = dequantize_delta_vec3(qx, qy, qz);
        assert!((rx - 12.5).abs() <= 0.008);
        assert!((ry - -45.25).abs() <= 0.008);
        assert!((rz - 100.125).abs() <= 0.008);

        // Ensure negative and diverse combinations are verified
        let (nx, ny, nz) = quantize_delta_vec3(-1.0, 1.0, -0.5);
        assert_eq!(nx, quantize_delta_i16(-1.0));
        assert_eq!(ny, quantize_delta_i16(1.0));
        assert_eq!(nz, quantize_delta_i16(-0.5));
        let (dx, dy, dz) = dequantize_delta_vec3(nx, ny, nz);
        assert!((dx - -1.0).abs() <= 0.008);
        assert!((dy - 1.0).abs() <= 0.008);
        assert!((dz - -0.5).abs() <= 0.008);
    }
}
