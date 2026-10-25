//! Abstract Collision and Spatial Traversal Primitives.
//!
//! Provides mathematically unopinionated, genre-agnostic 3D collision query traits,
//! raycasting hit structures, continuous collision detection (CCD) sweep tests,
//! and reference spatial substrates.

use std::collections::{BinaryHeap, HashMap};

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

    /// Tests whether a discrete 3D position is free of collision and traversable with a query filter.
    fn is_point_traversable_with_filter(&self, point: Vec3, _filter: SpatialQueryFilter) -> bool {
        self.is_point_traversable(point)
    }

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

    fn is_point_traversable_with_filter(&self, _point: Vec3, _filter: SpatialQueryFilter) -> bool {
        true
    }

    fn register_obstacle(&mut self, _id: Guid, _bounds: BoundingBox3D) {}

    fn unregister_obstacle(&mut self, _id: Guid) {}

    fn find_path(&self, start: Vec3, end: Vec3) -> Option<Vec<Vec3>> {
        Some(vec![start, end])
    }
}

/// Record storing an obstacle's bounding box and collision layer bitmask.
#[derive(Debug, Clone, PartialEq)]
pub struct ObstacleRecord {
    pub bounds: BoundingBox3D,
    pub layer_mask: u32,
}

/// High-performance reference collision world based on dynamic Axis-Aligned Bounding Boxes.
#[derive(Debug, Default, Clone)]
pub struct AabbCollisionWorld {
    obstacles: HashMap<Guid, ObstacleRecord>,
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
        self.obstacles.get(&id).map(|r| &r.bounds)
    }

    /// Retrieves the obstacle record of a registered obstacle.
    pub fn get_obstacle_record(&self, id: Guid) -> Option<&ObstacleRecord> {
        self.obstacles.get(&id)
    }

    /// Dynamically registers or updates an obstacle bounding box with an explicit collision layer bitmask.
    pub fn register_obstacle_with_layers(
        &mut self,
        id: Guid,
        bounds: BoundingBox3D,
        layer_mask: u32,
    ) {
        self.obstacles
            .insert(id, ObstacleRecord { bounds, layer_mask });
    }

    /// Tests whether a discrete 3D position is free of collision and traversable with a query filter.
    pub fn is_point_traversable_with_filter(
        &self,
        point: Vec3,
        filter: SpatialQueryFilter,
    ) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() || !point.z.is_finite() {
            return false;
        }

        let p_tuple = (point.x, point.y, point.z);
        for record in self.obstacles.values() {
            if (filter.collision_layers & record.layer_mask) != 0 && record.bounds.contains(p_tuple)
            {
                return false;
            }
        }
        true
    }

    /// Helper computing analytical ray intersection against an AABB with surface normal.
    fn ray_intersect_box(
        origin: Vec3,
        dir: Vec3,
        max_dist: f32,
        aabb: &BoundingBox3D,
    ) -> Option<(f32, Vec3)> {
        aabb.intersect_ray_with_normal(
            (origin.x, origin.y, origin.z),
            (dir.x, dir.y, dir.z),
            max_dist,
        )
        .map(|(dist, normal)| (dist, Vec3::new(normal.0, normal.1, normal.2)))
    }

    /// Checks whether a linear segment between `from` and `to` is completely unobstructed,
    /// testing both raycast line-of-sight and clearance bounding box sweep.
    pub fn is_segment_clear(&self, from: Vec3, to: Vec3, clearance: f32) -> bool {
        let diff = Vec3::new(to.x - from.x, to.y - from.y, to.z - from.z);
        let dist = diff.length();
        if dist < 1e-4 {
            return true;
        }
        let Some(norm_dir) = diff.normalized() else {
            return true;
        };

        // 1. Line-of-sight raycast check
        if self
            .raycast(from, norm_dir, dist, SpatialQueryFilter::all())
            .is_some()
        {
            return false;
        }

        // 2. Continuous collision sweep test with clearance volume
        if clearance > 0.0 {
            let clearance_box = BoundingBox3D::new(
                (from.x - clearance, from.y - clearance, from.z - clearance),
                (from.x + clearance, from.y + clearance, from.z + clearance),
            );
            if self
                .sweep_test(&clearance_box, diff, SpatialQueryFilter::all())
                .is_some()
            {
                return false;
            }
        }

        true
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

        for (&id, record) in &self.obstacles {
            if filter.ignore_id == Some(id) {
                continue;
            }
            if (filter.collision_layers & record.layer_mask) == 0 {
                continue;
            }

            if let Some((dist, normal)) =
                Self::ray_intersect_box(origin, norm_dir, max_distance, &record.bounds)
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

        for (&id, record) in &self.obstacles {
            if filter.ignore_id == Some(id) {
                continue;
            }
            if (filter.collision_layers & record.layer_mask) == 0 {
                continue;
            }

            // Minkowski sum: expand the obstacle AABB by the moving AABB's half extents
            let bounds = &record.bounds;
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
        self.is_point_traversable_with_filter(point, SpatialQueryFilter::all())
    }

    fn is_point_traversable_with_filter(&self, point: Vec3, filter: SpatialQueryFilter) -> bool {
        AabbCollisionWorld::is_point_traversable_with_filter(self, point, filter)
    }

    fn register_obstacle(&mut self, id: Guid, bounds: BoundingBox3D) {
        self.register_obstacle_with_layers(id, bounds, u32::MAX);
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

        // Fast Path 1: Check start & end point traversability
        if !self.is_point_traversable(start) || !self.is_point_traversable(end) {
            return None;
        }

        // Fast Path 2: Negligible distance
        let diff = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
        let dist = diff.length();
        if dist < 1e-4 {
            return Some(vec![start, end]);
        }

        const STEP: f32 = 1.0;
        const CLEARANCE: f32 = 0.4;
        const MAX_EXPANDED_NODES: usize = 2048;

        // Fast Path 3: Direct line-of-sight raycast & clearance sweep
        if self.is_segment_clear(start, end, CLEARANCE) {
            return Some(vec![start, end]);
        }

        // Discretized 3D/2.5D A* Lattice Search anchored at `start`
        #[derive(Copy, Clone, PartialEq)]
        struct AStarNode {
            f_score: f32,
            coord: (i32, i32, i32),
        }

        impl Eq for AStarNode {}

        impl PartialOrd for AStarNode {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for AStarNode {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                other
                    .f_score
                    .partial_cmp(&self.f_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        }

        let cell_pos = |c: (i32, i32, i32)| {
            Vec3::new(
                start.x + c.0 as f32 * STEP,
                start.y + c.1 as f32 * STEP,
                start.z + c.2 as f32 * STEP,
            )
        };

        let goal_cell = (
            ((end.x - start.x) / STEP).round() as i32,
            ((end.y - start.y) / STEP).round() as i32,
            ((end.z - start.z) / STEP).round() as i32,
        );

        let heuristic = |c: (i32, i32, i32)| {
            let pos = cell_pos(c);
            let dx = end.x - pos.x;
            let dy = end.y - pos.y;
            let dz = end.z - pos.z;
            (dx * dx + dy * dy + dz * dz).sqrt()
        };

        let mut open_set = BinaryHeap::new();
        let mut g_score: HashMap<(i32, i32, i32), f32> = HashMap::new();
        let mut came_from: HashMap<(i32, i32, i32), (i32, i32, i32)> = HashMap::new();

        let start_coord = (0, 0, 0);
        g_score.insert(start_coord, 0.0);
        open_set.push(AStarNode {
            f_score: heuristic(start_coord),
            coord: start_coord,
        });

        let mut expanded_count = 0;
        let mut reached_goal_coord: Option<(i32, i32, i32)> = None;
        let check_vertical = (end.y - start.y).abs() > 0.5;

        while let Some(current) = open_set.pop() {
            let curr_coord = current.coord;
            let curr_g = match g_score.get(&curr_coord) {
                Some(&g) => g,
                None => continue,
            };
            let curr_pos = cell_pos(curr_coord);

            // Goal condition: within <= 1.5 * STEP of end (or goal cell reached) and clear line to end
            let to_end = Vec3::new(end.x - curr_pos.x, end.y - curr_pos.y, end.z - curr_pos.z);
            let dist_to_end = to_end.length();
            if (dist_to_end <= 1.5 * STEP || curr_coord == goal_cell)
                && self.is_segment_clear(curr_pos, end, CLEARANCE)
            {
                reached_goal_coord = Some(curr_coord);
                break;
            }

            expanded_count += 1;
            if expanded_count >= MAX_EXPANDED_NODES {
                break;
            }

            let mut neighbors: Vec<((i32, i32, i32), f32)> = vec![
                ((curr_coord.0 + 1, curr_coord.1, curr_coord.2), 1.0),
                ((curr_coord.0 - 1, curr_coord.1, curr_coord.2), 1.0),
                ((curr_coord.0, curr_coord.1, curr_coord.2 + 1), 1.0),
                ((curr_coord.0, curr_coord.1, curr_coord.2 - 1), 1.0),
                (
                    (curr_coord.0 + 1, curr_coord.1, curr_coord.2 + 1),
                    std::f32::consts::SQRT_2,
                ),
                (
                    (curr_coord.0 - 1, curr_coord.1, curr_coord.2 + 1),
                    std::f32::consts::SQRT_2,
                ),
                (
                    (curr_coord.0 + 1, curr_coord.1, curr_coord.2 - 1),
                    std::f32::consts::SQRT_2,
                ),
                (
                    (curr_coord.0 - 1, curr_coord.1, curr_coord.2 - 1),
                    std::f32::consts::SQRT_2,
                ),
            ];
            if check_vertical {
                neighbors.push(((curr_coord.0, curr_coord.1 + 1, curr_coord.2), 1.0));
                neighbors.push(((curr_coord.0, curr_coord.1 - 1, curr_coord.2), 1.0));
            }

            for (next_coord, cost) in neighbors {
                let next_pos = cell_pos(next_coord);
                if !self.is_point_traversable(next_pos) {
                    continue;
                }
                if !self.is_segment_clear(curr_pos, next_pos, CLEARANCE) {
                    continue;
                }

                let tentative_g = curr_g + cost;
                if tentative_g < *g_score.get(&next_coord).unwrap_or(&f32::INFINITY) {
                    came_from.insert(next_coord, curr_coord);
                    g_score.insert(next_coord, tentative_g);
                    let f = tentative_g + heuristic(next_coord);
                    open_set.push(AStarNode {
                        f_score: f,
                        coord: next_coord,
                    });
                }
            }
        }

        let goal_coord = reached_goal_coord?;

        // Reconstruct raw waypoint sequence from goal to start
        let mut raw_coords = Vec::new();
        let mut curr = goal_coord;
        raw_coords.push(curr);
        while let Some(&prev) = came_from.get(&curr) {
            raw_coords.push(prev);
            curr = prev;
        }
        raw_coords.reverse();

        let mut raw: Vec<Vec3> = raw_coords.into_iter().map(cell_pos).collect();
        if raw.is_empty() {
            raw.push(start);
        } else {
            raw[0] = start;
        }

        let last_pos = *raw.last().unwrap();
        let dist_last_to_end =
            Vec3::new(end.x - last_pos.x, end.y - last_pos.y, end.z - last_pos.z).length();
        if dist_last_to_end > 1e-4 {
            raw.push(end);
        }

        // Line-of-sight string-pulling (greedy path smoothing)
        let mut smoothed = vec![raw[0]];
        let mut current_idx = 0;
        while current_idx + 1 < raw.len() {
            let mut furthest_idx = current_idx + 1;
            for candidate_idx in ((current_idx + 2)..raw.len()).rev() {
                if self.is_segment_clear(raw[current_idx], raw[candidate_idx], CLEARANCE) {
                    furthest_idx = candidate_idx;
                    break;
                }
            }
            smoothed.push(raw[furthest_idx]);
            current_idx = furthest_idx;
        }

        Some(smoothed)
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

    #[test]
    fn test_collision_layers_filtering() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(100);
        let obstacle_aabb = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0));
        // Register obstacle with layer mask 0b0010 (layer 2)
        world.register_obstacle_with_layers(obstacle_id, obstacle_aabb, 0b0010);

        let ray_origin = Vec3::new(-5.0, 0.0, 0.0);
        let ray_dir = Vec3::new(1.0, 0.0, 0.0);

        // Query with layer 0b0001 (layer 1) -> ignores obstacle
        let filter_mismatch = SpatialQueryFilter {
            ignore_id: None,
            collision_layers: 0b0001,
        };
        assert!(world
            .raycast(ray_origin, ray_dir, 10.0, filter_mismatch)
            .is_none());
        assert!(world.is_point_traversable_with_filter(Vec3::new(0.0, 0.0, 0.0), filter_mismatch));

        let small_box = create_test_aabb(Vec3::new(-5.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        assert!(world
            .sweep_test(&small_box, Vec3::new(10.0, 0.0, 0.0), filter_mismatch)
            .is_none());

        // Query with layer 0b0010 -> hits obstacle
        let filter_match = SpatialQueryFilter {
            ignore_id: None,
            collision_layers: 0b0010,
        };
        assert!(world
            .raycast(ray_origin, ray_dir, 10.0, filter_match)
            .is_some());
        assert!(!world.is_point_traversable_with_filter(Vec3::new(0.0, 0.0, 0.0), filter_match));
        assert!(world
            .sweep_test(&small_box, Vec3::new(10.0, 0.0, 0.0), filter_match)
            .is_some());
    }

    #[test]
    fn test_pathfinding_around_single_wall() {
        let mut world = AabbCollisionWorld::new();
        let obstacle_id = Guid(101);
        let wall = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 2.0, 3.0));
        world.register_obstacle(obstacle_id, wall);

        let start = Vec3::new(-5.0, 0.0, 0.0);
        let end = Vec3::new(5.0, 0.0, 0.0);
        let path = world
            .find_path(start, end)
            .expect("should find path around single wall");

        assert!(path.len() >= 3);
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&end));

        for i in 0..(path.len() - 1) {
            let from = path[i];
            let to = path[i + 1];
            assert!(world.is_segment_clear(from, to, 0.4));
        }
    }

    #[test]
    fn test_pathfinding_around_l_shaped_barrier() {
        let mut world = AabbCollisionWorld::new();
        // Wall 1: from x = -2.0 to 2.0 at z = 0.0 (half_size = (2.0, 1.0, 0.5))
        world.register_obstacle(
            Guid(201),
            create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.5)),
        );
        // Wall 2: extending from x = 2.0 along +z to z = 4.0 (center = (2.0, 0.0, 2.0), half_size = (0.5, 1.0, 2.0))
        world.register_obstacle(
            Guid(202),
            create_test_aabb(Vec3::new(2.0, 0.0, 2.0), Vec3::new(0.5, 1.0, 2.0)),
        );

        let start = Vec3::new(0.0, 0.0, 2.0);
        let end = Vec3::new(4.0, 0.0, -2.0);

        let path = world
            .find_path(start, end)
            .expect("should navigate around L-shaped barrier");
        assert!(path.len() >= 3);
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&end));

        for i in 0..(path.len() - 1) {
            let from = path[i];
            let to = path[i + 1];
            assert!(world.is_segment_clear(from, to, 0.4));
        }
    }

    #[test]
    fn test_pathfinding_around_u_shaped_trap() {
        let mut world = AabbCollisionWorld::new();
        // Back wall of U: x in [-3.0, 3.0], z at 0.0
        world.register_obstacle(
            Guid(301),
            create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(3.0, 1.0, 0.5)),
        );
        // Left arm of U: x at -3.0, z from 0.0 to 4.0
        world.register_obstacle(
            Guid(302),
            create_test_aabb(Vec3::new(-3.0, 0.0, 2.0), Vec3::new(0.5, 1.0, 2.0)),
        );
        // Right arm of U: x at 3.0, z from 0.0 to 4.0
        world.register_obstacle(
            Guid(303),
            create_test_aabb(Vec3::new(3.0, 0.0, 2.0), Vec3::new(0.5, 1.0, 2.0)),
        );

        let start = Vec3::new(0.0, 0.0, 1.0);
        let end = Vec3::new(0.0, 0.0, -3.0);

        let path = world
            .find_path(start, end)
            .expect("should escape U-trap and reach destination");
        assert!(path.len() >= 4);
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&end));

        for i in 0..(path.len() - 1) {
            let from = path[i];
            let to = path[i + 1];
            assert!(world.is_segment_clear(from, to, 0.4));
        }
    }

    #[test]
    fn test_pathfinding_unreachable_enclosed_target() {
        let mut world = AabbCollisionWorld::new();
        // 4 solid enclosing walls around (0, 0, 0)
        world.register_obstacle(
            Guid(401),
            create_test_aabb(Vec3::new(0.0, 0.0, 2.0), Vec3::new(2.5, 1.0, 0.5)),
        );
        world.register_obstacle(
            Guid(402),
            create_test_aabb(Vec3::new(0.0, 0.0, -2.0), Vec3::new(2.5, 1.0, 0.5)),
        );
        world.register_obstacle(
            Guid(403),
            create_test_aabb(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.5, 1.0, 2.5)),
        );
        world.register_obstacle(
            Guid(404),
            create_test_aabb(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.5, 1.0, 2.5)),
        );

        let start = Vec3::new(-10.0, 0.0, 0.0);
        let end = Vec3::new(0.0, 0.0, 0.0);

        let path = world.find_path(start, end);
        assert!(path.is_none(), "fully enclosed target must be unreachable");
    }

    #[test]
    fn test_mutation_killing_boundary_collision_coverage() {
        // 1. SpatialQueryFilter defaults
        let filter = SpatialQueryFilter::all();
        assert_eq!(filter.ignore_id, None);
        assert_eq!(filter.collision_layers, u32::MAX);

        // 2. EmptyCollisionWorld filter method
        let empty = EmptyCollisionWorld;
        assert!(empty.is_point_traversable_with_filter(Vec3::new(1.0, 2.0, 3.0), filter));

        // 3. AabbCollisionWorld len, is_empty, getters
        let mut world = AabbCollisionWorld::new();
        assert_eq!(world.len(), 0);
        assert!(world.is_empty());
        let id1 = Guid(11);
        let id2 = Guid(12);
        let box1 = create_test_aabb(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        let box2 = create_test_aabb(Vec3::new(5.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        world.register_obstacle(id1, box1);
        assert_eq!(world.len(), 1);
        assert!(!world.is_empty());
        assert_eq!(world.get_obstacle(id1), Some(&box1));
        assert_eq!(world.get_obstacle(id2), None);
        assert_eq!(world.get_obstacle_record(id1).unwrap().bounds, box1);
        assert_eq!(world.get_obstacle_record(id1).unwrap().layer_mask, u32::MAX);
        assert_eq!(world.get_obstacle_record(id2), None);

        // 4. Non-finite point traversability
        assert!(!world.is_point_traversable_with_filter(Vec3::new(f32::NAN, 0.0, 0.0), filter));
        assert!(!world.is_point_traversable_with_filter(Vec3::new(0.0, f32::INFINITY, 0.0), filter));
        assert!(
            !world.is_point_traversable_with_filter(Vec3::new(0.0, 0.0, f32::NEG_INFINITY), filter)
        );

        // 5. is_segment_clear zero distance & clearance sweep branch
        assert!(world.is_segment_clear(
            Vec3::new(10.0, 10.0, 10.0),
            Vec3::new(10.0, 10.0, 10.0),
            0.5
        ));
        // Ray from (0, 2, 0) to (10, 2, 0) misses obstacle at (2, 0, 0) [top is y=0.5]
        // But clearance box with radius 1.8 reaches down to y = 0.2 <= 0.5, hitting the box
        assert!(!world.is_segment_clear(Vec3::new(0.0, 2.0, 0.0), Vec3::new(10.0, 2.0, 0.0), 1.8));
        assert!(world.is_segment_clear(Vec3::new(0.0, 2.0, 0.0), Vec3::new(10.0, 2.0, 0.0), 0.0));

        // 6. Raycast non-finite & invalid distance handling
        assert!(world
            .raycast(
                Vec3::new(f32::NAN, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                10.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, f32::INFINITY, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                10.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, f32::NEG_INFINITY),
                Vec3::new(1.0, 0.0, 0.0),
                10.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(f32::NAN, 0.0, 0.0),
                10.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, f32::INFINITY, 0.0),
                10.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, f32::NEG_INFINITY),
                10.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                0.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                -5.0,
                filter
            )
            .is_none());
        assert!(world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                f32::NAN,
                filter
            )
            .is_none());

        // 7. Raycast closest hit ordering
        world.register_obstacle(id2, box2);
        let hit = world
            .raycast(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                20.0,
                filter,
            )
            .unwrap();
        assert_eq!(hit.entity_id, Some(id1));
        assert!((hit.distance - 1.5).abs() < 1e-4);
        assert!((hit.point.x - 1.5).abs() < 1e-4);
        assert!((hit.point.y - 0.0).abs() < 1e-4);
        assert!((hit.point.z - 0.0).abs() < 1e-4);

        // 8. Sweep test non-finite & tiny translation
        let moving = create_test_aabb(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        assert!(world
            .sweep_test(&moving, Vec3::new(f32::NAN, 0.0, 0.0), filter)
            .is_none());
        assert!(world
            .sweep_test(&moving, Vec3::new(0.0, f32::INFINITY, 0.0), filter)
            .is_none());
        assert!(world
            .sweep_test(&moving, Vec3::new(0.0, 0.0, f32::NEG_INFINITY), filter)
            .is_none());
        assert!(world
            .sweep_test(&moving, Vec3::new(1e-5, 0.0, 0.0), filter)
            .is_none());

        // 9. Sweep test exact hit fraction, coordinates and earlier hit resolution
        // Moving box center starts at (-2, 0, 0), half-extents (0.5, 0.5, 0.5)
        // Sweeps along +X by 10.0 units. Obstacle 1 center (2, 0, 0) [bounds [1.5, 2.5]]
        // Expanded box min_x = 1.5 - 0.5 = 1.0
        // Contact distance = 1.0 - (-2.0) = 3.0
        // Translation distance = 10.0 => fraction = 3.0 / 10.0 = 0.3
        let sweep_hit = world
            .sweep_test(
                &create_test_aabb(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5)),
                Vec3::new(10.0, 0.0, 0.0),
                filter,
            )
            .unwrap();
        assert_eq!(sweep_hit.entity_id, Some(id1));
        assert!((sweep_hit.hit_fraction - 0.3).abs() < 1e-4);
        assert!((sweep_hit.point.x - 1.0).abs() < 1e-4);
        assert!((sweep_hit.point.y - 0.0).abs() < 1e-4);
        assert!((sweep_hit.point.z - 0.0).abs() < 1e-4);
        assert!((sweep_hit.normal.x - (-1.0)).abs() < 1e-4);

        // 10. Find path invalid coordinates, inside obstacles, zero distance, vertical
        assert!(world
            .find_path(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0))
            .is_none());
        assert!(world
            .find_path(
                Vec3::new(0.0, f32::INFINITY, 0.0),
                Vec3::new(10.0, 0.0, 0.0)
            )
            .is_none());
        assert!(world
            .find_path(
                Vec3::new(0.0, 0.0, f32::NEG_INFINITY),
                Vec3::new(10.0, 0.0, 0.0)
            )
            .is_none());
        assert!(world
            .find_path(Vec3::new(0.0, 0.0, 0.0), Vec3::new(f32::NAN, 0.0, 0.0))
            .is_none());
        assert!(world
            .find_path(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, f32::INFINITY, 0.0))
            .is_none());
        assert!(world
            .find_path(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, f32::NEG_INFINITY)
            )
            .is_none());
        assert!(world
            .find_path(Vec3::new(2.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0))
            .is_none());
        assert!(world
            .find_path(Vec3::new(10.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0))
            .is_none());
        let same = world
            .find_path(Vec3::new(10.0, 10.0, 10.0), Vec3::new(10.0, 10.0, 10.0))
            .unwrap();
        assert_eq!(same.len(), 2);
        assert_eq!(same[0], Vec3::new(10.0, 10.0, 10.0));
        assert_eq!(same[1], Vec3::new(10.0, 10.0, 10.0));

        // Vertical pathfinding
        let vert = world
            .find_path(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 3.0, 0.0))
            .unwrap();
        assert_eq!(vert.len(), 2);
    }
}
