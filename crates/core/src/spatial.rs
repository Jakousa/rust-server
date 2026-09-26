//! Authoritative 3D Spatial Target Query Primitives.
//!
//! Provides geometric queries against the authoritative 3D [`Octree`] and coordinate space:
//! - Point-to-point range validation.
//! - Cone / Frontal Cleave angular arc queries.
//! - Sphere / Radial Area-of-Effect queries with deterministic distance sorting.
//! - Raycast Line-of-Sight validation against Axis-Aligned Bounding Boxes (AABBs).

use crate::guid::Guid;
use crate::octree::{BoundingBox3D, Octree};

/// 3D Vector primitive for spatial math calculations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length_squared(&self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn length(&self) -> f32 {
        self.length_squared().sqrt()
    }

    pub fn normalized(&self) -> Option<Self> {
        let len = self.length();
        if len > 1e-6 {
            Some(Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            })
        } else {
            None
        }
    }

    pub fn dot(&self, other: &Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn distance_squared(&self, other: &Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }

    pub fn distance(&self, other: &Self) -> f32 {
        self.distance_squared(other).sqrt()
    }
}

impl From<(f32, f32, f32)> for Vec3 {
    fn from(coords: (f32, f32, f32)) -> Self {
        Self::new(coords.0, coords.1, coords.2)
    }
}

impl From<Vec3> for (f32, f32, f32) {
    fn from(v: Vec3) -> Self {
        (v.x, v.y, v.z)
    }
}

/// Error returned when a spatial query validation fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpatialQueryError {
    TargetOutOfRange { distance: u32, max_range: u32 },
    TargetOutsideCone,
    LineOfSightBlocked,
    InvalidFacingVector,
}

/// Point-to-Point distance validation.
pub fn validate_point_range(
    from: Vec3,
    to: Vec3,
    max_range: f32,
) -> Result<f32, SpatialQueryError> {
    let dist = from.distance(&to);
    if dist <= max_range {
        Ok(dist)
    } else {
        Err(SpatialQueryError::TargetOutOfRange {
            distance: dist.round() as u32,
            max_range: max_range.round() as u32,
        })
    }
}

/// Validates whether a target lies within a frontal cone/cleave arc.
///
/// - `origin`: Initiator position.
/// - `facing`: Direction vector the initiator is facing.
/// - `target`: Target position.
/// - `max_range`: Radius of the cone.
/// - `half_angle_rad`: Half-angle of the cone in radians (e.g. $\pi / 4 = 45^\circ$, full arc $90^\circ$).
pub fn validate_cone_target(
    origin: Vec3,
    facing: Vec3,
    target: Vec3,
    max_range: f32,
    half_angle_rad: f32,
) -> Result<f32, SpatialQueryError> {
    let dist = validate_point_range(origin, target, max_range)?;
    if dist < 1e-4 {
        // Target is at the exact origin
        return Ok(dist);
    }

    let facing_norm = facing
        .normalized()
        .ok_or(SpatialQueryError::InvalidFacingVector)?;
    let to_target = Vec3::new(
        (target.x - origin.x) / dist,
        (target.y - origin.y) / dist,
        (target.z - origin.z) / dist,
    );

    let dot = facing_norm.dot(&to_target);
    let cos_half_angle = half_angle_rad.cos();

    if dot >= cos_half_angle {
        Ok(dist)
    } else {
        Err(SpatialQueryError::TargetOutsideCone)
    }
}

/// Queries all entities in the [`Octree`] within a spherical radius, sorted by distance.
pub fn query_sphere_targets(
    octree: &Octree,
    center: Vec3,
    radius: f32,
    max_targets: usize,
) -> Vec<(Guid, f32)> {
    let center_coords = (center.x, center.y, center.z);
    let nearby = octree.query_radius_with_pos(center_coords, radius);

    let mut scored: Vec<(Guid, f32)> = nearby
        .into_iter()
        .map(|(guid, pos)| {
            let dist = center.distance(&pos.into());
            (guid, dist)
        })
        .collect();

    // Sort by ascending distance deterministically
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_targets);
    scored
}

/// Checks whether a ray from `origin` to `target` intersects an Axis-Aligned Bounding Box (AABB)
/// using the slab method.
///
/// Returns `true` if an intersection occurs before reaching the target.
pub fn ray_intersects_aabb(origin: Vec3, target: Vec3, aabb: &BoundingBox3D) -> bool {
    let dir = Vec3::new(
        target.x - origin.x,
        target.y - origin.y,
        target.z - origin.z,
    );
    let target_dist = dir.length();
    if target_dist < 1e-6 {
        return false;
    }

    let ray_dir = Vec3::new(
        dir.x / target_dist,
        dir.y / target_dist,
        dir.z / target_dist,
    );

    let (mut t_min, mut t_max) = (0.0f32, target_dist);

    // X slab
    if ray_dir.x.abs() > 1e-6 {
        let inv_d = 1.0 / ray_dir.x;
        let mut t1 = (aabb.min.0 - origin.x) * inv_d;
        let mut t2 = (aabb.max.0 - origin.x) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return false;
        }
    } else if origin.x < aabb.min.0 || origin.x > aabb.max.0 {
        return false;
    }

    // Y slab
    if ray_dir.y.abs() > 1e-6 {
        let inv_d = 1.0 / ray_dir.y;
        let mut t1 = (aabb.min.1 - origin.y) * inv_d;
        let mut t2 = (aabb.max.1 - origin.y) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return false;
        }
    } else if origin.y < aabb.min.1 || origin.y > aabb.max.1 {
        return false;
    }

    // Z slab
    if ray_dir.z.abs() > 1e-6 {
        let inv_d = 1.0 / ray_dir.z;
        let mut t1 = (aabb.min.2 - origin.z) * inv_d;
        let mut t2 = (aabb.max.2 - origin.z) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return false;
        }
    } else if origin.z < aabb.min.2 || origin.z > aabb.max.2 {
        return false;
    }

    // Intersected strictly between origin and target
    t_max > 1e-4 && t_min < target_dist - 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;

    #[test]
    fn test_validate_point_range() {
        let p1 = Vec3::new(0.0, 0.0, 0.0);
        let p2 = Vec3::new(10.0, 0.0, 0.0);

        assert!(validate_point_range(p1, p2, 10.0).is_ok());
        assert!(validate_point_range(p1, p2, 15.0).is_ok());
        assert!(matches!(
            validate_point_range(p1, p2, 9.9),
            Err(SpatialQueryError::TargetOutOfRange { .. })
        ));
    }

    #[test]
    fn test_validate_cone_target() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let facing = Vec3::new(1.0, 0.0, 0.0); // Facing +X
        let half_angle = std::f32::consts::FRAC_PI_4; // 45 deg

        // Directly in front at (5, 0, 0)
        let in_front = Vec3::new(5.0, 0.0, 0.0);
        assert!(validate_cone_target(origin, facing, in_front, 10.0, half_angle).is_ok());

        // Angled within 45 deg: at (5, 4, 0) -> angle = atan(4/5) ~ 38.6 deg < 45 deg
        let angled = Vec3::new(5.0, 4.0, 0.0);
        assert!(validate_cone_target(origin, facing, angled, 10.0, half_angle).is_ok());

        // Angled outside 45 deg: at (5, 6, 0) -> angle = atan(6/5) ~ 50.2 deg > 45 deg
        let outside_angle = Vec3::new(5.0, 6.0, 0.0);
        assert!(matches!(
            validate_cone_target(origin, facing, outside_angle, 10.0, half_angle),
            Err(SpatialQueryError::TargetOutsideCone)
        ));

        // Behind at (-5, 0, 0)
        let behind = Vec3::new(-5.0, 0.0, 0.0);
        assert!(matches!(
            validate_cone_target(origin, facing, behind, 10.0, half_angle),
            Err(SpatialQueryError::TargetOutsideCone)
        ));
    }

    #[test]
    fn test_query_sphere_targets_sorting_and_cap() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);
        let g1 = Guid::new(EntityType::Npc, 1, 1);
        let g2 = Guid::new(EntityType::Npc, 1, 2);
        let g3 = Guid::new(EntityType::Npc, 1, 3);

        octree.insert(g1, (10.0, 0.0, 0.0));
        octree.insert(g2, (2.0, 0.0, 0.0));
        octree.insert(g3, (5.0, 0.0, 0.0));

        let results = query_sphere_targets(&octree, Vec3::new(0.0, 0.0, 0.0), 15.0, 2);
        assert_eq!(results.len(), 2);
        // Sorted ascending: g2 (dist 2.0), g3 (dist 5.0)
        assert_eq!(results[0].0, g2);
        assert_eq!(results[1].0, g3);
    }

    #[test]
    fn test_ray_intersects_aabb_los() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let target = Vec3::new(20.0, 0.0, 0.0);

        // Blocking obstacle directly between origin and target: X in [8, 12]
        let obstacle = BoundingBox3D::new((8.0, -2.0, -2.0), (12.0, 2.0, 2.0));
        assert!(ray_intersects_aabb(origin, target, &obstacle));

        // Off-path obstacle: Y in [5, 10]
        let off_path = BoundingBox3D::new((8.0, 5.0, -2.0), (12.0, 10.0, 2.0));
        assert!(!ray_intersects_aabb(origin, target, &off_path));

        // Obstacle behind target: X in [25, 30]
        let behind_target = BoundingBox3D::new((25.0, -2.0, -2.0), (30.0, 2.0, 2.0));
        assert!(!ray_intersects_aabb(origin, target, &behind_target));
    }
}
