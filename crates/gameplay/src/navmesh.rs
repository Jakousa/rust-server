//! Decoupled Polygonal Navigation Mesh Plugin & Spatial Traversal.
//!
//! Provides polygonal navigation mesh generation, dynamic obstacle carving,
//! A* dual-graph corridor search, string-pulling waypoint smoothing,
//! and implementation of the abstract [`mmo_core::collision::CollisionWorld`] trait.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use mmo_core::collision::{
    AabbCollisionWorld, CollisionWorld, RayHit, SpatialQueryFilter, SweepHit,
};
use mmo_core::guid::Guid;
use mmo_core::octree::BoundingBox3D;
use mmo_core::spatial::Vec3;
use mmo_core::{GameplayPlugin, PluginContext};

/// Convex polygon node in the navigation mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct NavMeshPolygon {
    pub id: usize,
    pub vertices: Vec<Vec3>,
    pub bounds: BoundingBox3D,
    pub centroid: Vec3,
    pub neighbors: Vec<usize>,
    pub is_blocked: bool,
    pub blocked_by: Option<Guid>,
}

impl NavMeshPolygon {
    /// Creates a new navigation polygon from 3D vertices.
    pub fn new(id: usize, vertices: Vec<Vec3>, neighbors: Vec<usize>) -> Self {
        let count = vertices.len() as f32;
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_z = 0.0;

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut max_z = f32::NEG_INFINITY;

        for v in &vertices {
            sum_x += v.x;
            sum_y += v.y;
            sum_z += v.z;

            min_x = min_x.min(v.x);
            min_y = min_y.min(v.y);
            min_z = min_z.min(v.z);
            max_x = max_x.max(v.x);
            max_y = max_y.max(v.y);
            max_z = max_z.max(v.z);
        }

        let centroid = if count > 0.0 {
            Vec3::new(sum_x / count, sum_y / count, sum_z / count)
        } else {
            Vec3::new(0.0, 0.0, 0.0)
        };

        let bounds = BoundingBox3D::new((min_x, min_y - 0.5, min_z), (max_x, max_y + 0.5, max_z));

        Self {
            id,
            vertices,
            bounds,
            centroid,
            neighbors,
            is_blocked: false,
            blocked_by: None,
        }
    }

    /// Tests if a 2D/3D point lies inside this convex polygon on the XZ plane.
    pub fn contains_point_xz(&self, p: Vec3) -> bool {
        if p.x < self.bounds.min.0
            || p.x > self.bounds.max.0
            || p.z < self.bounds.min.2
            || p.z > self.bounds.max.2
        {
            return false;
        }

        let n = self.vertices.len();
        if n < 3 {
            return false;
        }

        let mut positive = false;
        let mut negative = false;

        for i in 0..n {
            let v1 = self.vertices[i];
            let v2 = self.vertices[(i + 1) % n];

            let cross = (p.x - v1.x) * (v2.z - v1.z) - (p.z - v1.z) * (v2.x - v1.x);

            if cross > 1e-4 {
                positive = true;
            } else if cross < -1e-4 {
                negative = true;
            }

            if positive && negative {
                return false;
            }
        }

        true
    }
}

/// A* node for priority queue search.
#[derive(Copy, Clone, PartialEq)]
struct AStarNode {
    polygon_id: usize,
    f_score: f32,
}

impl Eq for AStarNode {}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap
        other
            .f_score
            .partial_cmp(&self.f_score)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Decoupled polygonal navigation mesh world supporting dynamic obstacle carving and A* routing.
#[derive(Debug, Default, Clone)]
pub struct NavMeshWorld {
    polygons: Vec<NavMeshPolygon>,
    aabb_world: AabbCollisionWorld,
}

impl NavMeshWorld {
    /// Creates an empty `NavMeshWorld`.
    pub fn new() -> Self {
        Self {
            polygons: Vec::new(),
            aabb_world: AabbCollisionWorld::new(),
        }
    }

    /// Constructs a `NavMeshWorld` from pre-defined convex polygons.
    pub fn from_polygons(polygons: Vec<NavMeshPolygon>) -> Self {
        Self {
            polygons,
            aabb_world: AabbCollisionWorld::new(),
        }
    }

    /// Constructs a connected grid of square polygons across the specified XZ area.
    pub fn from_grid(
        min_x: f32,
        max_x: f32,
        min_z: f32,
        max_z: f32,
        cell_size: f32,
        y_elevation: f32,
    ) -> Self {
        let cols = ((max_x - min_x) / cell_size).max(1.0).ceil() as usize;
        let rows = ((max_z - min_z) / cell_size).max(1.0).ceil() as usize;

        let mut polygons = Vec::with_capacity(cols * rows);

        for r in 0..rows {
            for c in 0..cols {
                let id = r * cols + c;
                let x0 = min_x + c as f32 * cell_size;
                let x1 = (x0 + cell_size).min(max_x);
                let z0 = min_z + r as f32 * cell_size;
                let z1 = (z0 + cell_size).min(max_z);

                let vertices = vec![
                    Vec3::new(x0, y_elevation, z0),
                    Vec3::new(x1, y_elevation, z0),
                    Vec3::new(x1, y_elevation, z1),
                    Vec3::new(x0, y_elevation, z1),
                ];

                let mut neighbors = Vec::new();
                if c > 0 {
                    neighbors.push(r * cols + (c - 1)); // West
                }
                if c + 1 < cols {
                    neighbors.push(r * cols + (c + 1)); // East
                }
                if r > 0 {
                    neighbors.push((r - 1) * cols + c); // South
                }
                if r + 1 < rows {
                    neighbors.push((r + 1) * cols + c); // North
                }

                polygons.push(NavMeshPolygon::new(id, vertices, neighbors));
            }
        }

        Self {
            polygons,
            aabb_world: AabbCollisionWorld::new(),
        }
    }

    /// Returns the total number of polygons in the navigation mesh.
    pub fn polygon_count(&self) -> usize {
        self.polygons.len()
    }

    /// Finds the polygon index enclosing the given coordinates, if any.
    pub fn find_polygon_at(&self, point: Vec3) -> Option<usize> {
        for (idx, poly) in self.polygons.iter().enumerate() {
            if poly.contains_point_xz(point) {
                return Some(idx);
            }
        }
        None
    }

    /// Finds the nearest unblocked polygon centroid to the given point.
    pub fn find_nearest_unblocked_polygon(&self, point: Vec3) -> Option<usize> {
        let mut nearest: Option<(usize, f32)> = None;
        for (idx, poly) in self.polygons.iter().enumerate() {
            if poly.is_blocked {
                continue;
            }
            let dist = point.distance_squared(&poly.centroid);
            match nearest {
                Some((_, best_d)) if dist < best_d => {
                    nearest = Some((idx, dist));
                }
                None => {
                    nearest = Some((idx, dist));
                }
                _ => {}
            }
        }
        nearest.map(|(idx, _)| idx)
    }

    /// Checks whether a point lies within any unblocked walkable polygon.
    pub fn is_within_walkable_mesh(&self, point: Vec3) -> bool {
        if self.polygons.is_empty() {
            return true;
        }
        match self.find_polygon_at(point) {
            Some(idx) => !self.polygons[idx].is_blocked,
            None => false,
        }
    }

    /// Carves/blocks all polygons overlapping with the given obstacle bounding box.
    fn carve_obstacle(&mut self, id: Guid, bounds: &BoundingBox3D) {
        for poly in &mut self.polygons {
            if poly.bounds.intersects(bounds) {
                poly.is_blocked = true;
                poly.blocked_by = Some(id);
            }
        }
    }

    /// Restores all polygons carved/blocked by the given obstacle GUID.
    fn uncarve_obstacle(&mut self, id: Guid) {
        for poly in &mut self.polygons {
            if poly.blocked_by == Some(id) {
                poly.is_blocked = false;
                poly.blocked_by = None;
            }
        }
    }
}

impl CollisionWorld for NavMeshWorld {
    fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        filter: SpatialQueryFilter,
    ) -> Option<RayHit> {
        self.aabb_world
            .raycast(origin, direction, max_distance, filter)
    }

    fn sweep_test(
        &self,
        aabb: &BoundingBox3D,
        translation: Vec3,
        filter: SpatialQueryFilter,
    ) -> Option<SweepHit> {
        self.aabb_world.sweep_test(aabb, translation, filter)
    }

    fn is_point_traversable(&self, point: Vec3) -> bool {
        if !self.aabb_world.is_point_traversable(point) {
            return false;
        }
        self.is_within_walkable_mesh(point)
    }

    fn register_obstacle(&mut self, id: Guid, bounds: BoundingBox3D) {
        self.aabb_world.register_obstacle(id, bounds);
        self.carve_obstacle(id, &bounds);
    }

    fn unregister_obstacle(&mut self, id: Guid) {
        self.aabb_world.unregister_obstacle(id);
        self.uncarve_obstacle(id);
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

        let dir = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
        let dist = dir.length();
        if dist < 1e-4 {
            return Some(vec![start, end]);
        }

        // 1. Direct path check through line-of-sight if no mesh polygons are configured
        if self.polygons.is_empty() {
            return self.aabb_world.find_path(start, end);
        }

        // 2. Locate start and goal polygons
        let start_poly = self
            .find_polygon_at(start)
            .filter(|&id| !self.polygons[id].is_blocked)
            .or_else(|| self.find_nearest_unblocked_polygon(start))?;

        let end_poly = self
            .find_polygon_at(end)
            .filter(|&id| !self.polygons[id].is_blocked)
            .or_else(|| self.find_nearest_unblocked_polygon(end))?;

        if start_poly == end_poly {
            return Some(vec![start, end]);
        }

        // 3. A* Search over polygon dual graph
        let mut open_set = BinaryHeap::new();
        let mut came_from: HashMap<usize, usize> = HashMap::new();
        let mut g_score: HashMap<usize, f32> = HashMap::new();

        g_score.insert(start_poly, 0.0);
        let h_start = self.polygons[start_poly].centroid.distance(&end);
        open_set.push(AStarNode {
            polygon_id: start_poly,
            f_score: h_start,
        });

        let mut found = false;

        while let Some(current) = open_set.pop() {
            let u = current.polygon_id;
            if u == end_poly {
                found = true;
                break;
            }

            let u_cost = *g_score.get(&u).unwrap_or(&f32::INFINITY);
            let u_centroid = self.polygons[u].centroid;

            for &neighbor in &self.polygons[u].neighbors {
                if neighbor >= self.polygons.len() || self.polygons[neighbor].is_blocked {
                    continue;
                }

                let neighbor_centroid = self.polygons[neighbor].centroid;
                let step_cost = u_centroid.distance(&neighbor_centroid);
                let tentative_g = u_cost + step_cost;

                if tentative_g < *g_score.get(&neighbor).unwrap_or(&f32::INFINITY) {
                    came_from.insert(neighbor, u);
                    g_score.insert(neighbor, tentative_g);
                    let f = tentative_g + neighbor_centroid.distance(&end);
                    open_set.push(AStarNode {
                        polygon_id: neighbor,
                        f_score: f,
                    });
                }
            }
        }

        if !found {
            return None;
        }

        // 4. Reconstruct corridor waypoints
        let mut corridor = Vec::new();
        let mut curr = end_poly;
        while curr != start_poly {
            corridor.push(self.polygons[curr].centroid);
            match came_from.get(&curr) {
                Some(&prev) => curr = prev,
                None => break,
            }
        }
        corridor.reverse();

        // 5. Waypoint smoothing (String-Pulling / Funnel approximation)
        let mut waypoints = vec![start];
        for centroid in corridor {
            waypoints.push(centroid);
        }
        waypoints.push(end);

        // Simplify consecutive redundant waypoints
        let mut simplified = vec![waypoints[0]];
        let mut current_idx = 0;

        while current_idx < waypoints.len() - 1 {
            let mut next_idx = current_idx + 1;
            for test_idx in (current_idx + 2..waypoints.len()).rev() {
                let from = waypoints[current_idx];
                let to = waypoints[test_idx];
                let seg = Vec3::new(to.x - from.x, to.y - from.y, to.z - from.z);
                let seg_len = seg.length();
                if seg_len > 1e-4 {
                    if let Some(norm_dir) = seg.normalized() {
                        let hit =
                            self.raycast(from, norm_dir, seg_len - 0.1, SpatialQueryFilter::all());
                        if hit.is_none() {
                            next_idx = test_idx;
                            break;
                        }
                    }
                }
            }
            simplified.push(waypoints[next_idx]);
            current_idx = next_idx;
        }

        Some(simplified)
    }
}

/// Decoupled NavMesh gameplay plugin registering authoritative spatial navigation.
#[derive(Default)]
pub struct NavMeshPlugin;

impl GameplayPlugin for NavMeshPlugin {
    fn id(&self) -> &'static str {
        "navmesh"
    }

    fn build(&self, _ctx: &mut PluginContext) {
        // NavMesh world is accessible and extensible via node runtime and systems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navmesh_grid_creation_and_containment() {
        // 10x10 world from 0.0 to 10.0 with 2.0 cell size
        let mesh = NavMeshWorld::from_grid(0.0, 10.0, 0.0, 10.0, 2.0, 0.0);
        assert_eq!(mesh.polygon_count(), 25);

        let p1 = Vec3::new(1.0, 0.0, 1.0);
        let poly_opt = mesh.find_polygon_at(p1);
        assert!(poly_opt.is_some());
        assert_eq!(poly_opt.unwrap(), 0);

        let p_out = Vec3::new(50.0, 0.0, 50.0);
        assert!(mesh.find_polygon_at(p_out).is_none());
    }

    #[test]
    fn test_dynamic_obstacle_carving_and_pathfinding_routing() {
        // 5x5 grid from 0.0 to 10.0 (cell_size 2.0)
        // Row 0: cols 0..5 (z in 0..2)
        // Row 1: cols 0..5 (z in 2..4)
        // Row 2: cols 0..5 (z in 4..6)
        // Row 3: cols 0..5 (z in 6..8)
        // Row 4: cols 0..5 (z in 8..10)
        let mut mesh = NavMeshWorld::from_grid(0.0, 10.0, 0.0, 10.0, 2.0, 0.0);

        let start = Vec3::new(1.0, 0.0, 5.0); // (col 0, row 2)
        let goal = Vec3::new(9.0, 0.0, 5.0); // (col 4, row 2)

        // Initial path should find direct route
        let path_init = mesh.find_path(start, goal);
        assert!(path_init.is_some());

        // Now place an obstacle at (5.0, 0.0, 5.0) blocking col 2, row 2
        let obstacle_id = Guid(999);
        let obstacle_box = BoundingBox3D::new((4.1, -1.0, 4.1), (5.9, 1.0, 5.9));
        mesh.register_obstacle(obstacle_id, obstacle_box);

        // Center point should no longer be traversable
        assert!(!mesh.is_point_traversable(Vec3::new(5.0, 0.0, 5.0)));

        // Pathfinding should now route around the obstacle
        let path_routed = mesh.find_path(start, goal);
        assert!(path_routed.is_some());
        let waypoints = path_routed.unwrap();
        assert!(waypoints.len() >= 3);

        // Ensure no waypoint in the path is inside the obstacle
        for wp in &waypoints {
            assert!(!obstacle_box.contains((wp.x, wp.y, wp.z)));
        }

        // Unregister obstacle: restores traversability
        mesh.unregister_obstacle(obstacle_id);
        assert!(mesh.is_point_traversable(Vec3::new(5.0, 0.0, 5.0)));
    }

    #[test]
    fn test_navmesh_sweep_test_and_raycast() {
        let mut mesh = NavMeshWorld::from_grid(0.0, 10.0, 0.0, 10.0, 2.0, 0.0);
        let obstacle_id = Guid(77);
        let obstacle_box = BoundingBox3D::new((4.0, -1.0, 4.0), (6.0, 1.0, 6.0));
        mesh.register_obstacle(obstacle_id, obstacle_box);

        // Raycast from (1, 0, 5) to (9, 0, 5) strikes the obstacle
        let hit = mesh.raycast(
            Vec3::new(1.0, 0.0, 5.0),
            Vec3::new(1.0, 0.0, 0.0),
            8.0,
            SpatialQueryFilter::all(),
        );
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().entity_id, Some(obstacle_id));

        // Sweep test with moving box
        let mover = BoundingBox3D::new((0.5, -0.5, 4.5), (1.5, 0.5, 5.5));
        let sweep = mesh.sweep_test(&mover, Vec3::new(6.0, 0.0, 0.0), SpatialQueryFilter::all());
        assert!(sweep.is_some());
        assert_eq!(sweep.unwrap().entity_id, Some(obstacle_id));
    }
}
