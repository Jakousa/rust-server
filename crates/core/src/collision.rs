//! Abstract Collision and Spatial Traversal Primitives.
//!
//! Provides mathematically unopinionated, genre-agnostic 3D collision query traits,
//! raycasting hit structures, continuous collision detection (CCD) sweep tests,
//! and reference spatial substrates.

use std::collections::HashMap;

use crate::guid::Guid;
use crate::octree::BoundingBox3D;
use crate::spatial::Vec3;

/// Query filter to prevent self-intersection and selectively mask collision layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpatialQueryFilter {
    /// Optional entity to ignore during query (e.g. self / shooter / mover).
    pub ignore_id: Option<Guid>,
    /// Bitmask of collision layers to test against.
    pub collision_layers: u32,
}

impl Default for SpatialQueryFilter {
    fn default() -> Self {
        Self {
            ignore_id: None,
            collision_layers: u32::MAX,
        }
    }
}

impl SpatialQueryFilter {
    /// Creates a filter that ignores a specific entity GUID.
    pub fn ignoring(entity_id: Guid) -> Self {
        Self {
            ignore_id: Some(entity_id),
            collision_layers: u32::MAX,
        }
    }

    /// Creates an empty query filter testing all entities and layers.
    pub fn all() -> Self {
        Self::default()
    }
}

/// Detailed hit information resulting from a raycast against collision geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit {
    /// Exact 3D point of impact in world space.
    pub point: Vec3,
    /// Surface unit normal at the impact point.
    pub normal: Vec3,
    /// Distance from ray origin to impact point.
    pub distance: f32,
    /// Identifier of the struck obstacle entity, if known.
    pub entity_id: Option<Guid>,
    /// Surface flags / material bitmask of the struck collider.
    pub surface_flags: u32,
}

/// Detailed collision information resulting from an AABB sweep test.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepHit {
    /// Normalized fraction along the sweep translation vector `[0.0, 1.0]` at impact.
    pub hit_fraction: f32,
    /// Exact 3D contact point at impact.
    pub point: Vec3,
    /// Outward unit surface normal at the contact point.
    pub normal: Vec3,
    /// Identifier of the struck obstacle entity, if known.
    pub entity_id: Option<Guid>,
}

/// Generalized, unopinionated 3D collision and spatial traversal trait.
pub trait CollisionWorld: Send + Sync {
    /// Casts an authoritative ray from `origin` in `direction` up to `max_distance`.
    fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        filter: SpatialQueryFilter,
    ) -> Option<RayHit>;

    /// Sweeps an Axis-Aligned Bounding Box along `translation`, testing for earliest collision.
    fn sweep_test(
        &self,
        aabb: &BoundingBox3D,
        translation: Vec3,
        filter: SpatialQueryFilter,
    ) -> Option<SweepHit>;

    /// Tests whether a discrete 3D position is free of collision and traversable.
    fn is_point_traversable(&self, point: Vec3) -> bool;

    /// Dynamically registers or updates an obstacle bounding box (e.g. placed prefab).
    fn register_obstacle(&mut self, id: Guid, bounds: BoundingBox3D);

    /// Dynamically unregisters an obstacle (e.g. destroyed prefab or unloaded sector).
    fn unregister_obstacle(&mut self, id: Guid);

    /// Finds a collision-free path between two coordinates, returning ordered waypoints.
    fn find_path(&self, start: Vec3, end: Vec3) -> Option<Vec<Vec3>>;
}

/// Default no-op / open space collision world (vacuum).
#[derive(Debug, Default, Clone)]
pub struct EmptyCollisionWorld;

impl CollisionWorld for EmptyCollisionWorld {
    fn raycast(
        &self,
        _origin: Vec3,
        _direction: Vec3,
        _max_distance: f32,
        _filter: SpatialQueryFilter,
    ) -> Option<RayHit> {
        None
    }

    fn sweep_test(
        &self,
        _aabb: &BoundingBox3D,
        _translation: Vec3,
        _filter: SpatialQueryFilter,
    ) -> Option<SweepHit> {
        None
    }

    fn is_point_traversable(&self, _point: Vec3) -> bool {
        true
    }

    fn register_obstacle(&mut self, _id: Guid, _bounds: BoundingBox3D) {}

    fn unregister_obstacle(&mut self, _id: Guid) {}

    fn find_path(&self, start: Vec3, end: Vec3) -> Option<Vec<Vec3>> {
        Some(vec![start, end])
    }
}

/// High-performance reference collision world based on dynamic Axis-Aligned Bounding Boxes.
#[derive(Debug, Default, Clone)]
pub struct AabbCollisionWorld {
    obstacles: HashMap<Guid, BoundingBox3D>,
}

impl AabbCollisionWorld {
    /// Constructs a new, empty `AabbCollisionWorld`.
    pub fn new() -> Self {
        Self {
            obstacles: HashMap::new(),
        }
    }

    /// Number of active obstacles registered in the collision world.
    pub fn len(&self) -> usize {
        self.obstacles.len()
    }

    /// Checks if the collision world contains any obstacles.
    pub fn is_empty(&self) -> bool {
        self.obstacles.is_empty()
    }

    /// Retrieves the bounding box of a registered obstacle.
    pub fn get_obstacle(&self, id: Guid) -> Option<&BoundingBox3D> {
        self.obstacles.get(&id)
    }

    /// Helper computing analytical ray intersection against an AABB with surface normal.
    fn ray_intersect_box(
        origin: Vec3,
        dir: Vec3,
        max_dist: f32,
        aabb: &BoundingBox3D,
    ) -> Option<(f32, Vec3)> {
        let mut t_min = 0.0f32;
        let mut t_max = max_dist;
        let mut normal = Vec3::new(0.0, 0.0, 0.0);

        // X slab
        if dir.x.abs() > 1e-6 {
            let inv_d = 1.0 / dir.x;
            let mut t1 = (aabb.min.0 - origin.x) * inv_d;
            let mut t2 = (aabb.max.0 - origin.x) * inv_d;
            let mut n1 = Vec3::new(-1.0, 0.0, 0.0);
            if inv_d < 0.0 {
                std::mem::swap(&mut t1, &mut t2);
                n1 = Vec3::new(1.0, 0.0, 0.0);
            }
            if t1 > t_min {
                t_min = t1;
                normal = n1;
            }
            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        } else if origin.x < aabb.min.0 || origin.x > aabb.max.0 {
            return None;
        }

        // Y slab
        if dir.y.abs() > 1e-6 {
            let inv_d = 1.0 / dir.y;
            let mut t1 = (aabb.min.1 - origin.y) * inv_d;
            let mut t2 = (aabb.max.1 - origin.y) * inv_d;
            let mut n1 = Vec3::new(0.0, -1.0, 0.0);
            if inv_d < 0.0 {
                std::mem::swap(&mut t1, &mut t2);
                n1 = Vec3::new(0.0, 1.0, 0.0);
            }
            if t1 > t_min {
                t_min = t1;
                normal = n1;
            }
            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        } else if origin.y < aabb.min.1 || origin.y > aabb.max.1 {
            return None;
        }

        // Z slab
        if dir.z.abs() > 1e-6 {
            let inv_d = 1.0 / dir.z;
            let mut t1 = (aabb.min.2 - origin.z) * inv_d;
            let mut t2 = (aabb.max.2 - origin.z) * inv_d;
            let mut n1 = Vec3::new(0.0, 0.0, -1.0);
            if inv_d < 0.0 {
                std::mem::swap(&mut t1, &mut t2);
                n1 = Vec3::new(0.0, 0.0, 1.0);
            }
            if t1 > t_min {
                t_min = t1;
                normal = n1;
            }
            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        } else if origin.z < aabb.min.2 || origin.z > aabb.max.2 {
            return None;
        }

        if t_min >= 0.0 && t_min <= max_dist {
            Some((t_min, normal))
        } else {
            None
        }
    }
}

impl CollisionWorld for AabbCollisionWorld {
    fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        filter: SpatialQueryFilter,
    ) -> Option<RayHit> {
        if !origin.x.is_finite()
            || !origin.y.is_finite()
            || !origin.z.is_finite()
            || !direction.x.is_finite()
            || !direction.y.is_finite()
            || !direction.z.is_finite()
            || !max_distance.is_finite()
            || max_distance <= 0.0
        {
            return None;
        }

        let norm_dir = direction.normalized()?;
        let mut closest_hit: Option<RayHit> = None;

        for (&id, bounds) in &self.obstacles {
            if filter.ignore_id == Some(id) {
                continue;
            }

            if let Some((dist, normal)) =
                Self::ray_intersect_box(origin, norm_dir, max_distance, bounds)
            {
                let is_closer = match &closest_hit {
                    Some(prev) => dist < prev.distance,
                    None => true,
                };
                if is_closer {
                    let point = Vec3::new(
                        origin.x + norm_dir.x * dist,
                        origin.y + norm_dir.y * dist,
                        origin.z + norm_dir.z * dist,
                    );
                    closest_hit = Some(RayHit {
                        point,
                        normal,
                        distance: dist,
                        entity_id: Some(id),
                        surface_flags: 0,
                    });
                }
            }
        }

        closest_hit
    }

    fn sweep_test(
        &self,
        aabb: &BoundingBox3D,
        translation: Vec3,
        filter: SpatialQueryFilter,
    ) -> Option<SweepHit> {
        if !translation.x.is_finite()
            || !translation.y.is_finite()
            || !translation.z.is_finite()
            || translation.length_squared() < 1e-8
        {
            return None;
        }

        let translation_dist = translation.length();
        let norm_trans = translation.normalized()?;

        // Half-extents of the moving AABB
        let half_x = (aabb.max.0 - aabb.min.0) * 0.5;
        let half_y = (aabb.max.1 - aabb.min.1) * 0.5;
        let half_z = (aabb.max.2 - aabb.min.2) * 0.5;
        let center = Vec3::new(
            aabb.min.0 + half_x,
            aabb.min.1 + half_y,
            aabb.min.2 + half_z,
        );

        let mut earliest_hit: Option<SweepHit> = None;

        for (&id, bounds) in &self.obstacles {
            if filter.ignore_id == Some(id) {
                continue;
            }

            // Minkowski sum: expand the obstacle AABB by the moving AABB's half extents
            let expanded_box = BoundingBox3D::new(
                (
                    bounds.min.0 - half_x,
                    bounds.min.1 - half_y,
                    bounds.min.2 - half_z,
                ),
                (
                    bounds.max.0 + half_x,
                    bounds.max.1 + half_y,
                    bounds.max.2 + half_z,
                ),
            );

            // Raycast center through expanded box
            if let Some((dist, normal)) =
                Self::ray_intersect_box(center, norm_trans, translation_dist, &expanded_box)
            {
                let fraction = (dist / translation_dist).clamp(0.0, 1.0);
                let is_earlier = match &earliest_hit {
                    Some(prev) => fraction < prev.hit_fraction,
                    None => true,
                };
                if is_earlier {
                    let point = Vec3::new(
                        center.x + norm_trans.x * dist,
                        center.y + norm_trans.y * dist,
                        center.z + norm_trans.z * dist,
                    );
                    earliest_hit = Some(SweepHit {
                        hit_fraction: fraction,
                        point,
                        normal,
                        entity_id: Some(id),
                    });
                }
            }
        }

        earliest_hit
    }

    fn is_point_traversable(&self, point: Vec3) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() || !point.z.is_finite() {
            return false;
        }

        let p_tuple = (point.x, point.y, point.z);
        for bounds in self.obstacles.values() {
            if bounds.contains(p_tuple) {
                return false;
            }
        }
        true
    }

    fn register_obstacle(&mut self, id: Guid, bounds: BoundingBox3D) {
        self.obstacles.insert(id, bounds);
    }

    fn unregister_obstacle(&mut self, id: Guid) {
        self.obstacles.remove(&id);
    }

    fn find_path(&self, start: Vec3, end: Vec3) -> Option<Vec<Vec3>> {
        if !start.x.is_finite()
            || !start.y.is_finite()
            || !start.z.is_finite()
            || !end.x.is_finite()
            || !end.y.is_finite()
            || !end.z.is_finite()
        {
            return None;
        }

        // 1. Direct path check
        let dir = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
        let dist = dir.length();
        if dist < 1e-4 {
            return Some(vec![start, end]);
        }

        let norm_dir = dir.normalized()?;
        // Check if direct ray hits anything before end
        let hit = self.raycast(start, norm_dir, dist, SpatialQueryFilter::all());
        if hit.is_none() {
            return Some(vec![start, end]);
        }

        // 2. Simple obstacle avoidance routing around the blocking obstacle
        let hit_hit = hit.unwrap();
        if let Some(obstacle_id) = hit_hit.entity_id {
            if let Some(bounds) = self.get_obstacle(obstacle_id) {
                // Route around the obstacle corners
                let margin = 0.5f32;
                // Corner 1: (min.x - margin, max.y, min.z - margin)
                let wp1 = Vec3::new(bounds.min.0 - margin, start.y, bounds.min.2 - margin);
                // Corner 2: (max.x + margin, max.y, min.z - margin)
                let wp2 = Vec3::new(bounds.max.0 + margin, start.y, bounds.min.2 - margin);

                let d1 = start.distance(&wp1) + wp1.distance(&end);
                let d2 = start.distance(&wp2) + wp2.distance(&end);
                let chosen_wp = if d1 <= d2 { wp1 } else { wp2 };

                return Some(vec![start, chosen_wp, end]);
            }
        }

        Some(vec![start, end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_aabb(center: Vec3, half_size: Vec3) -> BoundingBox3D {
        BoundingBox3D::new(
            (
                center.x - half_size.x,
                center.y - half_size.y,
                center.z - half_size.z,
            ),
            (
                center.x + half_size.x,
                center.y + half_size.y,
                center.z + half_size.z,
            ),
        )
    }

    #[test]
    fn test_raycast_hit_and_miss() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        world.register_obstacle(obstacle_id, obstacle_aabb);

        // Ray hits the obstacle from -2.0 along +X
        let ray_origin = Vec3::new(-2.0, 0.0, 0.0);
        let ray_direction = Vec3::new(1.0, 0.0, 0.0);
        let hit = world.raycast(ray_origin, ray_direction, 10.0, SpatialQueryFilter::all());
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert!((hit.point.x - (-0.5)).abs() < 1e-4);
        assert!((hit.normal.x - (-1.0)).abs() < 1e-4);
        assert!((hit.distance - 1.5).abs() < 1e-4);
        assert_eq!(hit.entity_id, Some(obstacle_id));

        // Ray misses the obstacle (offset along Y)
        let ray_origin = Vec3::new(-2.0, 5.0, 0.0);
        let hit = world.raycast(ray_origin, ray_direction, 10.0, SpatialQueryFilter::all());
        assert!(hit.is_none());

        // Ray in opposite direction misses
        let hit = world.raycast(
            ray_origin,
            Vec3::new(-1.0, 0.0, 0.0),
            10.0,
            SpatialQueryFilter::all(),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn test_raycast_query_filter_ignore() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        world.register_obstacle(obstacle_id, obstacle_aabb);

        let ray_origin = Vec3::new(-2.0, 0.0, 0.0);
        let ray_direction = Vec3::new(1.0, 0.0, 0.0);
        let filter = SpatialQueryFilter::ignoring(obstacle_id);
        let hit = world.raycast(ray_origin, ray_direction, 10.0, filter);
        assert!(hit.is_none());
    }

    #[test]
    fn test_sweep_test_hit_and_clear() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        world.register_obstacle(obstacle_id, obstacle_aabb);

        // Moving box of size 0.2 half-extent starting at (-2.0, 0, 0)
        let moving_box = create_test_aabb(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.2, 0.2, 0.2));
        let translation = Vec3::new(4.0, 0.0, 0.0);
        let hit = world.sweep_test(&moving_box, translation, SpatialQueryFilter::all());
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert!(hit.hit_fraction > 0.0 && hit.hit_fraction < 1.0);
        assert_eq!(hit.entity_id, Some(obstacle_id));

        // Moving box passing above obstacle
        let clear_box = create_test_aabb(Vec3::new(-2.0, 5.0, 0.0), Vec3::new(0.2, 0.2, 0.2));
        let hit = world.sweep_test(&clear_box, translation, SpatialQueryFilter::all());
        assert!(hit.is_none());
    }

    #[test]
    fn test_sweep_test_zero_length_and_nan() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        world.register_obstacle(obstacle_id, obstacle_aabb);

        let moving_box = create_test_aabb(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.2, 0.2, 0.2));

        // Zero-length translation
        let hit = world.sweep_test(
            &moving_box,
            Vec3::new(0.0, 0.0, 0.0),
            SpatialQueryFilter::all(),
        );
        assert!(hit.is_none());

        // Non-finite translation
        let hit = world.sweep_test(
            &moving_box,
            Vec3::new(f32::NAN, 0.0, 0.0),
            SpatialQueryFilter::all(),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn test_is_point_traversable_and_lifecycle() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0));
        world.register_obstacle(obstacle_id, obstacle_aabb);

        // Point inside bounds
        assert!(!world.is_point_traversable(Vec3::new(0.0, 0.0, 0.0)));
        // Point outside bounds
        assert!(world.is_point_traversable(Vec3::new(5.0, 0.0, 0.0)));
        // Non-finite point
        assert!(!world.is_point_traversable(Vec3::new(f32::NAN, 0.0, 0.0)));

        // Unregister obstacle
        world.unregister_obstacle(obstacle_id);
        assert!(world.is_point_traversable(Vec3::new(0.0, 0.0, 0.0)));
    }

    #[test]
    fn test_find_path_unobstructed_and_obstructed() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0));
        world.register_obstacle(obstacle_id, obstacle_aabb);

        // Clear path (offset)
        let start = Vec3::new(-5.0, 5.0, 0.0);
        let end = Vec3::new(5.0, 5.0, 0.0);
        let path = world.find_path(start, end).unwrap();
        assert_eq!(path.len(), 2);

        // Obstructed path directly through the box
        let start = Vec3::new(-5.0, 0.0, 0.0);
        let end = Vec3::new(5.0, 0.0, 0.0);
        let path = world.find_path(start, end).unwrap();
        assert!(path.len() >= 3);
    }

    #[test]
    fn test_empty_collision_world() {
        let world = EmptyCollisionWorld;
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                10.0,
                SpatialQueryFilter::all()
            )
            .is_none());
        assert!(world.is_point_traversable(Vec3::new(0.0, 0.0, 0.0)));
        let start = Vec3::new(0.0, 0.0, 0.0);
        let end = Vec3::new(1.0, 1.0, 1.0);
        assert_eq!(world.find_path(start, end), Some(vec![start, end]));
    }
}
