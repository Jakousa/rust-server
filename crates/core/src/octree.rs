//! Sparse Octree spatial partitioning module with dynamic subdivision,
//! neighbor discovery, and Area of Interest (AoI) throttling.

use crate::guid::Guid;

/// Default maximum entities in a leaf node before subdividing into 8 child octants.
pub const DEFAULT_MAX_ENTITIES_PER_LEAF: usize = 16;

/// Default minimum entity count in a branch subtree before collapsing back into a leaf node.
pub const DEFAULT_MIN_ENTITIES_TO_MERGE: usize = 4;

/// Minimum dimension size of a bounding box below which no further subdivision occurs.
/// Prevents infinite recursion when multiple entities occupy identical or near-identical coordinates.
const MIN_BOUNDS_SIZE: f32 = 1e-4;

/// Axis-Aligned 3D Bounding Box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox3D {
    pub min: (f32, f32, f32),
    pub max: (f32, f32, f32),
}

impl BoundingBox3D {
    /// Constructs a new `BoundingBox3D` ensuring `min <= max` component-wise.
    pub fn new(min: (f32, f32, f32), max: (f32, f32, f32)) -> Self {
        Self {
            min: (min.0.min(max.0), min.1.min(max.1), min.2.min(max.2)),
            max: (min.0.max(max.0), min.1.max(max.1), min.2.max(max.2)),
        }
    }

    /// Returns `true` if the point is within or on the boundary of this bounding box.
    #[inline]
    pub fn contains(&self, point: (f32, f32, f32)) -> bool {
        point.0 >= self.min.0
            && point.0 <= self.max.0
            && point.1 >= self.min.1
            && point.1 <= self.max.1
            && point.2 >= self.min.2
            && point.2 <= self.max.2
    }

    /// Returns `true` if this bounding box intersects another bounding box.
    #[inline]
    pub fn intersects(&self, other: &BoundingBox3D) -> bool {
        self.min.0 <= other.max.0
            && self.max.0 >= other.min.0
            && self.min.1 <= other.max.1
            && self.max.1 >= other.min.1
            && self.min.2 <= other.max.2
            && self.max.2 >= other.min.2
    }

    /// Returns `true` if this bounding box intersects a sphere defined by `center` and `radius`.
    pub fn intersects_sphere(&self, center: (f32, f32, f32), radius: f32) -> bool {
        if radius < 0.0 {
            return false;
        }

        let cx = center.0.clamp(self.min.0, self.max.0);
        let cy = center.1.clamp(self.min.1, self.max.1);
        let cz = center.2.clamp(self.min.2, self.max.2);

        let dx = center.0 - cx;
        let dy = center.1 - cy;
        let dz = center.2 - cz;

        dx * dx + dy * dy + dz * dz <= radius * radius
    }

    /// Returns the center point of the bounding box.
    #[inline]
    pub fn center(&self) -> (f32, f32, f32) {
        (
            (self.min.0 + self.max.0) * 0.5,
            (self.min.1 + self.max.1) * 0.5,
            (self.min.2 + self.max.2) * 0.5,
        )
    }

    /// Returns the dimensional extent (width, height, depth) along the X, Y, and Z axes.
    #[inline]
    pub fn dimensions(&self) -> (f32, f32, f32) {
        (
            self.max.0 - self.min.0,
            self.max.1 - self.min.1,
            self.max.2 - self.min.2,
        )
    }

    /// Returns a new bounding box expanded uniformly by `margin` in all directions.
    #[inline]
    pub fn expanded(&self, margin: f32) -> Self {
        Self {
            min: (
                self.min.0 - margin,
                self.min.1 - margin,
                self.min.2 - margin,
            ),
            max: (
                self.max.0 + margin,
                self.max.1 + margin,
                self.max.2 + margin,
            ),
        }
    }
    /// Computes the octant index (0..8) for a given point relative to the box center.
    #[inline]
    pub fn child_index(&self, pos: (f32, f32, f32)) -> usize {
        let mid_x = (self.min.0 + self.max.0) * 0.5;
        let mid_y = (self.min.1 + self.max.1) * 0.5;
        let mid_z = (self.min.2 + self.max.2) * 0.5;

        let mut idx = 0;
        if pos.0 >= mid_x {
            idx |= 1;
        }
        if pos.1 >= mid_y {
            idx |= 2;
        }
        if pos.2 >= mid_z {
            idx |= 4;
        }
        idx
    }
}

/// Represents the internal structure of an octree node: either a data-holding `Leaf`
/// or a subdivided `Branch` with 8 octant children.
#[derive(Debug, Clone, PartialEq)]
pub enum OctreeNode {
    Leaf {
        entities: Vec<(Guid, (f32, f32, f32))>,
    },
    Branch {
        children: Box<[Octree; 8]>,
    },
}

/// Hierarchical Sparse Octree for 3D spatial partitioning.
#[derive(Debug, Clone, PartialEq)]
pub struct Octree {
    pub bounds: BoundingBox3D,
    pub node: OctreeNode,
    pub max_entities_per_leaf: usize,
    pub min_entities_to_merge: usize,
}

impl Octree {
    /// Creates a new empty `Octree` with the given bounding box and default capacity thresholds.
    pub fn new(bounds: BoundingBox3D) -> Self {
        Self::with_config(
            bounds,
            DEFAULT_MAX_ENTITIES_PER_LEAF,
            DEFAULT_MIN_ENTITIES_TO_MERGE,
        )
    }

    /// Creates a new empty `Octree` with custom leaf capacity and merge thresholds.
    pub fn with_config(
        bounds: BoundingBox3D,
        max_entities_per_leaf: usize,
        min_entities_to_merge: usize,
    ) -> Self {
        Self {
            bounds,
            node: OctreeNode::Leaf {
                entities: Vec::new(),
            },
            max_entities_per_leaf,
            min_entities_to_merge,
        }
    }

    /// Inserts an entity into the octree at the specified 3D position.
    ///
    /// If the entity is outside the bounding box, returns `false`.
    /// Automatically subdivides a leaf into 8 child octants if capacity is exceeded.
    pub fn insert(&mut self, guid: Guid, pos: (f32, f32, f32)) -> bool {
        if !self.bounds.contains(pos) {
            return false;
        }

        let bounds = self.bounds;
        match &mut self.node {
            OctreeNode::Leaf { entities } => {
                let (dx, dy, dz) = bounds.dimensions();
                let min_dim = dx.min(dy).min(dz);

                if entities.len() < self.max_entities_per_leaf || min_dim <= MIN_BOUNDS_SIZE {
                    entities.push((guid, pos));
                    true
                } else {
                    self.subdivide();
                    self.insert(guid, pos)
                }
            }
            OctreeNode::Branch { children } => {
                let idx = bounds.child_index(pos);
                if children[idx].insert(guid, pos) {
                    return true;
                }
                // Fallback for boundary/precision edge cases
                for child in children.iter_mut() {
                    if child.insert(guid, pos) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Removes an entity with the given GUID and position from the octree.
    ///
    /// If the entity was found and removed, and the total entity count in the branch
    /// drops to or below `min_entities_to_merge`, collapses children back into a `Leaf`.
    /// Returns `true` if the entity was found and removed.
    pub fn remove(&mut self, guid: Guid, pos: (f32, f32, f32)) -> bool {
        if !self.bounds.contains(pos) {
            return false;
        }

        let bounds = self.bounds;
        let removed = match &mut self.node {
            OctreeNode::Leaf { entities } => {
                if let Some(idx) = entities.iter().position(|&(g, _)| g == guid) {
                    entities.swap_remove(idx);
                    true
                } else {
                    false
                }
            }
            OctreeNode::Branch { children } => {
                let idx = bounds.child_index(pos);
                let mut res = children[idx].remove(guid, pos);
                if !res {
                    for (i, child) in children.iter_mut().enumerate() {
                        if i != idx && child.bounds.contains(pos) && child.remove(guid, pos) {
                            res = true;
                            break;
                        }
                    }
                }
                res
            }
        };

        if removed {
            self.try_merge();
        }

        removed
    }

    /// Removes an entity by its GUID without requiring its position, traversing the tree.
    ///
    /// Automatically collapses branches if entity count falls below `min_entities_to_merge`.
    pub fn remove_by_guid(&mut self, guid: Guid) -> bool {
        let removed = match &mut self.node {
            OctreeNode::Leaf { entities } => {
                if let Some(idx) = entities.iter().position(|&(g, _)| g == guid) {
                    entities.swap_remove(idx);
                    true
                } else {
                    false
                }
            }
            OctreeNode::Branch { children } => {
                let mut res = false;
                for child in children.iter_mut() {
                    if child.remove_by_guid(guid) {
                        res = true;
                        break;
                    }
                }
                res
            }
        };

        if removed {
            self.try_merge();
        }

        removed
    }

    /// Queries and returns all entity GUIDs within radial distance from `center`.
    pub fn query_radius(&self, center: (f32, f32, f32), radius: f32) -> Vec<Guid> {
        let mut results = Vec::new();
        if radius >= 0.0 {
            self.query_radius_internal(center, radius, radius * radius, &mut results);
        }
        results
    }

    /// Queries and returns all entity GUIDs with their positions within radial distance from `center`.
    pub fn query_radius_with_pos(
        &self,
        center: (f32, f32, f32),
        radius: f32,
    ) -> Vec<(Guid, (f32, f32, f32))> {
        let mut results = Vec::new();
        if radius >= 0.0 {
            self.query_radius_with_pos_internal(center, radius, radius * radius, &mut results);
        }
        results
    }

    /// Queries all entity GUIDs within radial distance from `center`, appending into `results`.
    pub fn query_radius_into(&self, center: (f32, f32, f32), radius: f32, results: &mut Vec<Guid>) {
        if radius >= 0.0 {
            self.query_radius_internal(center, radius, radius * radius, results);
        }
    }

    /// Queries all entity GUIDs with their positions within radial distance from `center`, appending into `results`.
    pub fn query_radius_with_pos_into(
        &self,
        center: (f32, f32, f32),
        radius: f32,
        results: &mut Vec<(Guid, (f32, f32, f32))>,
    ) {
        if radius >= 0.0 {
            self.query_radius_with_pos_internal(center, radius, radius * radius, results);
        }
    }

    /// Queries all entity GUIDs with their positions and squared distance within radial distance from `center`, appending into `results`.
    pub fn query_radius_with_dist_sq_into(
        &self,
        center: (f32, f32, f32),
        radius: f32,
        results: &mut Vec<(Guid, (f32, f32, f32), f32)>,
    ) {
        if radius >= 0.0 {
            self.query_radius_with_dist_sq_internal(center, radius, radius * radius, results);
        }
    }

    /// Queries and returns all entity GUIDs located within the given axis-aligned bounding box.
    pub fn query_range(&self, aabb: &BoundingBox3D) -> Vec<Guid> {
        let mut results = Vec::new();
        self.query_range_internal(aabb, &mut results);
        results
    }

    /// Queries all entity GUIDs located within the given axis-aligned bounding box, appending into `results`.
    pub fn query_range_into(&self, aabb: &BoundingBox3D, results: &mut Vec<Guid>) {
        self.query_range_internal(aabb, results);
    }

    /// Returns the total number of entities stored across this octree node and its descendants.
    pub fn len(&self) -> usize {
        match &self.node {
            OctreeNode::Leaf { entities } => entities.len(),
            OctreeNode::Branch { children } => children.iter().map(|c| c.len()).sum(),
        }
    }

    /// Returns `true` if the octree contains no entities.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if this node is a `Leaf`.
    pub fn is_leaf(&self) -> bool {
        matches!(self.node, OctreeNode::Leaf { .. })
    }

    /// Returns `true` if this node is a `Branch`.
    pub fn is_branch(&self) -> bool {
        matches!(self.node, OctreeNode::Branch { .. })
    }

    /// Returns the depth of the tree from this node down to the deepest leaf.
    pub fn depth(&self) -> usize {
        match &self.node {
            OctreeNode::Leaf { .. } => 1,
            OctreeNode::Branch { children } => {
                1 + children.iter().map(|c| c.depth()).max().unwrap_or(0)
            }
        }
    }

    /// Returns the total number of leaf nodes in this subtree.
    pub fn leaf_count(&self) -> usize {
        match &self.node {
            OctreeNode::Leaf { .. } => 1,
            OctreeNode::Branch { children } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }

    /// Clears all entities from the tree, resetting it to a single leaf.
    pub fn clear(&mut self) {
        self.node = OctreeNode::Leaf {
            entities: Vec::new(),
        };
    }

    /// Finds the leaf octree node enclosing the specified position, if within bounds.
    pub fn find_leaf(&self, pos: (f32, f32, f32)) -> Option<&Octree> {
        if !self.bounds.contains(pos) {
            return None;
        }

        match &self.node {
            OctreeNode::Leaf { .. } => Some(self),
            OctreeNode::Branch { children } => {
                let idx = self.child_index(pos);
                children[idx].find_leaf(pos).or_else(|| {
                    children.iter().find_map(|child| child.find_leaf(pos))
                })
            }
        }
    }

    /// Queries all leaf nodes whose bounding box intersects the given AABB.
    pub fn query_leaves_intersecting<'a>(&'a self, aabb: &BoundingBox3D) -> Vec<&'a Octree> {
        let mut leaves = Vec::new();
        self.query_leaves_intersecting_internal(aabb, &mut leaves);
        leaves
    }

    /// Identifies all neighboring leaf octants that share a boundary face or edge within `margin`
    /// of an entity positioned at `pos`.
    pub fn find_boundary_neighbors(
        &self,
        pos: (f32, f32, f32),
        margin: f32,
    ) -> Vec<BoundingBox3D> {
        if margin <= 0.0 {
            return Vec::new();
        }

        let current_leaf = match self.find_leaf(pos) {
            Some(leaf) => leaf,
            None => return Vec::new(),
        };

        let b = &current_leaf.bounds;
        let near_boundary = (pos.0 - b.min.0) <= margin
            || (b.max.0 - pos.0) <= margin
            || (pos.1 - b.min.1) <= margin
            || (b.max.1 - pos.1) <= margin
            || (pos.2 - b.min.2) <= margin
            || (b.max.2 - pos.2) <= margin;

        if !near_boundary {
            return Vec::new();
        }

        let search_box = BoundingBox3D::new(
            (pos.0 - margin, pos.1 - margin, pos.2 - margin),
            (pos.0 + margin, pos.1 + margin, pos.2 + margin),
        );

        self.query_leaves_intersecting(&search_box)
            .into_iter()
            .filter(|leaf| leaf.bounds != current_leaf.bounds)
            .map(|leaf| leaf.bounds)
            .collect()
    }

    /// Dynamically scales visibility radius down when crowd density exceeds the slice budget (AoI throttling).
    ///
    /// When `current_density <= max_budget`, returns `base_radius` unmodified.
    /// When `current_density > max_budget`, contracts radius proportionally:
    /// `base_radius * sqrt(max_budget / current_density)`.
    #[inline]
    pub fn calculate_throttled_aoi(
        base_radius: f32,
        current_density: usize,
        max_budget: usize,
    ) -> f32 {
        calculate_throttled_aoi(base_radius, current_density, max_budget)
    }

    // --- Private Helper Methods ---

    fn query_radius_internal(
        &self,
        center: (f32, f32, f32),
        radius: f32,
        radius_sq: f32,
        results: &mut Vec<Guid>,
    ) {
        if !self.bounds.intersects_sphere(center, radius) {
            return;
        }

        match &self.node {
            OctreeNode::Leaf { entities } => {
                for &(guid, pos) in entities {
                    let dx = pos.0 - center.0;
                    let dy = pos.1 - center.1;
                    let dz = pos.2 - center.2;
                    if dx * dx + dy * dy + dz * dz <= radius_sq {
                        results.push(guid);
                    }
                }
            }
            OctreeNode::Branch { children } => {
                for child in children.iter() {
                    child.query_radius_internal(center, radius, radius_sq, results);
                }
            }
        }
    }

    fn query_radius_with_pos_internal(
        &self,
        center: (f32, f32, f32),
        radius: f32,
        radius_sq: f32,
        results: &mut Vec<(Guid, (f32, f32, f32))>,
    ) {
        if !self.bounds.intersects_sphere(center, radius) {
            return;
        }

        match &self.node {
            OctreeNode::Leaf { entities } => {
                for &(guid, pos) in entities {
                    let dx = pos.0 - center.0;
                    let dy = pos.1 - center.1;
                    let dz = pos.2 - center.2;
                    if dx * dx + dy * dy + dz * dz <= radius_sq {
                        results.push((guid, pos));
                    }
                }
            }
            OctreeNode::Branch { children } => {
                for child in children.iter() {
                    child.query_radius_with_pos_internal(center, radius, radius_sq, results);
                }
            }
        }
    }

    fn query_radius_with_dist_sq_internal(
        &self,
        center: (f32, f32, f32),
        radius: f32,
        radius_sq: f32,
        results: &mut Vec<(Guid, (f32, f32, f32), f32)>,
    ) {
        if !self.bounds.intersects_sphere(center, radius) {
            return;
        }

        match &self.node {
            OctreeNode::Leaf { entities } => {
                for &(guid, pos) in entities {
                    let dx = pos.0 - center.0;
                    let dy = pos.1 - center.1;
                    let dz = pos.2 - center.2;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq <= radius_sq {
                        results.push((guid, pos, dist_sq));
                    }
                }
            }
            OctreeNode::Branch { children } => {
                for child in children.iter() {
                    child.query_radius_with_dist_sq_internal(center, radius, radius_sq, results);
                }
            }
        }
    }

    fn query_range_internal(&self, aabb: &BoundingBox3D, results: &mut Vec<Guid>) {
        if !self.bounds.intersects(aabb) {
            return;
        }

        match &self.node {
            OctreeNode::Leaf { entities } => {
                for &(guid, pos) in entities {
                    if aabb.contains(pos) {
                        results.push(guid);
                    }
                }
            }
            OctreeNode::Branch { children } => {
                for child in children.iter() {
                    child.query_range_internal(aabb, results);
                }
            }
        }
    }

    fn query_leaves_intersecting_internal<'a>(
        &'a self,
        aabb: &BoundingBox3D,
        leaves: &mut Vec<&'a Octree>,
    ) {
        if !self.bounds.intersects(aabb) {
            return;
        }

        match &self.node {
            OctreeNode::Leaf { .. } => {
                leaves.push(self);
            }
            OctreeNode::Branch { children } => {
                for child in children.iter() {
                    child.query_leaves_intersecting_internal(aabb, leaves);
                }
            }
        }
    }

    /// Returns the octant child index (0..8) corresponding to the given position.
    #[inline]
    pub fn child_index(&self, pos: (f32, f32, f32)) -> usize {
        self.bounds.child_index(pos)
    }

    fn subdivide(&mut self) {
        let old_node = std::mem::replace(
            &mut self.node,
            OctreeNode::Leaf {
                entities: Vec::new(),
            },
        );

        let entities = match old_node {
            OctreeNode::Leaf { entities } => entities,
            OctreeNode::Branch { .. } => return,
        };

        let mid_x = (self.bounds.min.0 + self.bounds.max.0) * 0.5;
        let mid_y = (self.bounds.min.1 + self.bounds.max.1) * 0.5;
        let mid_z = (self.bounds.min.2 + self.bounds.max.2) * 0.5;

        let mut children_vec = Vec::with_capacity(8);
        for i in 0..8 {
            let x_min = if (i & 1) == 0 { self.bounds.min.0 } else { mid_x };
            let x_max = if (i & 1) == 0 { mid_x } else { self.bounds.max.0 };
            let y_min = if (i & 2) == 0 { self.bounds.min.1 } else { mid_y };
            let y_max = if (i & 2) == 0 { mid_y } else { self.bounds.max.1 };
            let z_min = if (i & 4) == 0 { self.bounds.min.2 } else { mid_z };
            let z_max = if (i & 4) == 0 { mid_z } else { self.bounds.max.2 };

            children_vec.push(Octree::with_config(
                BoundingBox3D::new((x_min, y_min, z_min), (x_max, y_max, z_max)),
                self.max_entities_per_leaf,
                self.min_entities_to_merge,
            ));
        }

        let children_box: Box<[Octree; 8]> = match children_vec.into_boxed_slice().try_into() {
            Ok(b) => b,
            Err(_) => unreachable!("Vector length is strictly 8"),
        };

        self.node = OctreeNode::Branch {
            children: children_box,
        };

        for (g, p) in entities {
            self.insert(g, p);
        }
    }

    fn try_merge(&mut self) {
        if let OctreeNode::Branch { children } = &mut self.node {
            let total: usize = children.iter().map(|c| c.len()).sum();
            if total <= self.min_entities_to_merge {
                let mut merged = Vec::with_capacity(total);
                for child in children.iter_mut() {
                    child.collect_all_entities(&mut merged);
                }
                self.node = OctreeNode::Leaf { entities: merged };
            }
        }
    }

    fn collect_all_entities(&mut self, out: &mut Vec<(Guid, (f32, f32, f32))>) {
        match &mut self.node {
            OctreeNode::Leaf { entities } => {
                out.append(entities);
            }
            OctreeNode::Branch { children } => {
                for child in children.iter_mut() {
                    child.collect_all_entities(out);
                }
            }
        }
    }
}

/// Standalone function for AoI throttling calculation.
///
/// Dynamically scales visibility radius down when crowd density exceeds the slice budget.
/// Returns `base_radius` if `current_density <= max_budget`.
/// Returns `0.0` if `base_radius <= 0.0` or `max_budget == 0`.
#[inline]
pub fn calculate_throttled_aoi(
    base_radius: f32,
    current_density: usize,
    max_budget: usize,
) -> f32 {
    if base_radius <= 0.0 {
        return 0.0;
    }
    if current_density <= max_budget {
        return base_radius;
    }
    if max_budget == 0 {
        return 0.0;
    }

    let scale = (max_budget as f32 / current_density as f32).sqrt();
    base_radius * scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;

    fn make_guid(seq: u32) -> Guid {
        Guid::new(EntityType::Player, 1, seq)
    }

    #[test]
    fn test_bounding_box_contains_and_intersects() {
        let bbox = BoundingBox3D::new((0.0, 0.0, 0.0), (10.0, 10.0, 10.0));

        // Contains checks
        assert!(bbox.contains((5.0, 5.0, 5.0)));
        assert!(bbox.contains((0.0, 0.0, 0.0)));
        assert!(bbox.contains((10.0, 10.0, 10.0)));
        assert!(!bbox.contains((-0.1, 5.0, 5.0)));
        assert!(!bbox.contains((10.1, 5.0, 5.0)));

        // Box-box intersection
        let overlapping = BoundingBox3D::new((5.0, 5.0, 5.0), (15.0, 15.0, 15.0));
        assert!(bbox.intersects(&overlapping));
        assert!(overlapping.intersects(&bbox));

        let separate = BoundingBox3D::new((10.1, 10.1, 10.1), (20.0, 20.0, 20.0));
        assert!(!bbox.intersects(&separate));

        let touching = BoundingBox3D::new((10.0, 0.0, 0.0), (20.0, 10.0, 10.0));
        assert!(bbox.intersects(&touching));

        // Sphere intersection
        assert!(bbox.intersects_sphere((5.0, 5.0, 5.0), 1.0));
        assert!(bbox.intersects_sphere((12.0, 5.0, 5.0), 2.5));
        assert!(!bbox.intersects_sphere((15.0, 5.0, 5.0), 1.0));
        assert!(!bbox.intersects_sphere((5.0, 5.0, 5.0), -1.0));
    }

    #[test]
    fn test_octree_insert_and_automatic_subdivision() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::with_config(bounds, 4, 2);

        assert!(octree.is_leaf());
        assert_eq!(octree.len(), 0);

        // Insert up to leaf capacity
        assert!(octree.insert(make_guid(1), (10.0, 10.0, 10.0)));
        assert!(octree.insert(make_guid(2), (20.0, 20.0, 20.0)));
        assert!(octree.insert(make_guid(3), (30.0, 30.0, 30.0)));
        assert!(octree.insert(make_guid(4), (40.0, 40.0, 40.0)));

        assert!(octree.is_leaf());
        assert_eq!(octree.len(), 4);

        // 5th entity exceeds capacity -> automatic subdivision into branch
        assert!(octree.insert(make_guid(5), (80.0, 80.0, 80.0)));
        assert!(octree.is_branch());
        assert_eq!(octree.len(), 5);

        // Out of bounds insertion fails
        assert!(!octree.insert(make_guid(6), (150.0, 50.0, 50.0)));
        assert_eq!(octree.len(), 5);
    }

    #[test]
    fn test_octree_query_radius() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::with_config(bounds, 4, 2);

        let center = (50.0, 50.0, 50.0);
        let g1 = make_guid(1); // distance 0
        let g2 = make_guid(2); // distance 5
        let g3 = make_guid(3); // distance 10
        let g4 = make_guid(4); // distance 20
        let g5 = make_guid(5); // distance 40

        octree.insert(g1, center);
        octree.insert(g2, (55.0, 50.0, 50.0));
        octree.insert(g3, (50.0, 60.0, 50.0));
        octree.insert(g4, (50.0, 50.0, 70.0));
        octree.insert(g5, (90.0, 50.0, 50.0));

        // Query radius 6.0: should return g1 and g2
        let res6 = octree.query_radius(center, 6.0);
        assert_eq!(res6.len(), 2);
        assert!(res6.contains(&g1));
        assert!(res6.contains(&g2));

        // Query radius 15.0: should return g1, g2, g3
        let res15 = octree.query_radius(center, 15.0);
        assert_eq!(res15.len(), 3);
        assert!(res15.contains(&g1));
        assert!(res15.contains(&g2));
        assert!(res15.contains(&g3));

        // Query radius 25.0: should return g1, g2, g3, g4
        let res25 = octree.query_radius(center, 25.0);
        assert_eq!(res25.len(), 4);
        assert!(!res25.contains(&g5));

        // Query radius 50.0: should return all 5
        let res50 = octree.query_radius(center, 50.0);
        assert_eq!(res50.len(), 5);

        // Negative radius returns empty
        assert!(octree.query_radius(center, -5.0).is_empty());
    }

    #[test]
    fn test_octree_query_range() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::with_config(bounds, 2, 1);

        let g1 = make_guid(10);
        let g2 = make_guid(20);
        let g3 = make_guid(30);

        octree.insert(g1, (10.0, 10.0, 10.0));
        octree.insert(g2, (20.0, 20.0, 20.0));
        octree.insert(g3, (80.0, 80.0, 80.0));

        let query_box = BoundingBox3D::new((5.0, 5.0, 5.0), (25.0, 25.0, 25.0));
        let found = octree.query_range(&query_box);

        assert_eq!(found.len(), 2);
        assert!(found.contains(&g1));
        assert!(found.contains(&g2));
        assert!(!found.contains(&g3));
    }

    #[test]
    fn test_octree_remove_and_merge() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        // max 4 per leaf, merge when total <= 2
        let mut octree = Octree::with_config(bounds, 4, 2);

        let g1 = make_guid(1);
        let g2 = make_guid(2);
        let g3 = make_guid(3);
        let g4 = make_guid(4);
        let g5 = make_guid(5);

        octree.insert(g1, (10.0, 10.0, 10.0));
        octree.insert(g2, (20.0, 10.0, 10.0));
        octree.insert(g3, (30.0, 10.0, 10.0));
        octree.insert(g4, (40.0, 10.0, 10.0));
        octree.insert(g5, (90.0, 90.0, 90.0));

        assert!(octree.is_branch());
        assert_eq!(octree.len(), 5);

        // Removing non-existent returns false
        assert!(!octree.remove(make_guid(999), (10.0, 10.0, 10.0)));

        // Remove 1 entity -> 4 remain, still branch (> 2)
        assert!(octree.remove(g1, (10.0, 10.0, 10.0)));
        assert_eq!(octree.len(), 4);
        assert!(octree.is_branch());

        // Remove another -> 3 remain, still branch (> 2)
        assert!(octree.remove(g2, (20.0, 10.0, 10.0)));
        assert_eq!(octree.len(), 3);
        assert!(octree.is_branch());

        // Remove 3rd entity -> 2 remain (<= min_entities_to_merge: 2) -> collapse to Leaf!
        assert!(octree.remove(g3, (30.0, 10.0, 10.0)));
        assert_eq!(octree.len(), 2);
        assert!(octree.is_leaf());

        // Remaining entities are intact
        let remaining = octree.query_radius((50.0, 50.0, 50.0), 100.0);
        assert_eq!(remaining.len(), 2);
        assert!(remaining.contains(&g4));
        assert!(remaining.contains(&g5));
    }

    #[test]
    fn test_aoi_throttling_calculation() {
        let base_radius = 100.0;
        let budget = 50;

        // Density within budget -> full radius
        assert_eq!(
            calculate_throttled_aoi(base_radius, 25, budget),
            base_radius
        );
        assert_eq!(
            calculate_throttled_aoi(base_radius, 50, budget),
            base_radius
        );

        // Density 4x budget (200 entities) -> radius throttled by sqrt(50/200) = 0.5 -> 50.0
        let throttled_200 = calculate_throttled_aoi(base_radius, 200, budget);
        assert!((throttled_200 - 50.0).abs() < 1e-4);

        // Density 9x budget (450 entities) -> radius throttled by sqrt(50/450) = 1/3 -> ~33.333
        let throttled_450 = calculate_throttled_aoi(base_radius, 450, budget);
        assert!((throttled_450 - (100.0 / 3.0)).abs() < 1e-4);

        // Monotonic decrease: higher density -> strictly smaller visibility radius
        let d100 = calculate_throttled_aoi(base_radius, 100, budget);
        let d200 = calculate_throttled_aoi(base_radius, 200, budget);
        let d300 = calculate_throttled_aoi(base_radius, 300, budget);
        assert!(d100 > d200);
        assert!(d200 > d300);

        // Zero budget -> 0 radius
        assert_eq!(calculate_throttled_aoi(base_radius, 100, 0), 0.0);

        // Non-positive base radius
        assert_eq!(calculate_throttled_aoi(0.0, 100, budget), 0.0);
        assert_eq!(calculate_throttled_aoi(-10.0, 100, budget), 0.0);

        // Associated function on Octree behaves identically
        assert_eq!(
            Octree::calculate_throttled_aoi(base_radius, 200, budget),
            throttled_200
        );
    }

    #[test]
    fn test_neighbor_boundary_discovery() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::with_config(bounds, 2, 1);

        // Insert entities into opposite corners to force subdivision
        octree.insert(make_guid(1), (10.0, 10.0, 10.0));
        octree.insert(make_guid(2), (20.0, 20.0, 20.0));
        octree.insert(make_guid(3), (80.0, 80.0, 80.0));
        octree.insert(make_guid(4), (90.0, 90.0, 90.0));

        assert!(octree.is_branch());

        // Midpoint of octree is at (50, 50, 50)
        // Entity at (49.0, 25.0, 25.0) is in Child 0 (x: 0..50), 1.0 unit away from boundary at x = 50.0
        let pos_near_boundary = (49.0, 25.0, 25.0);
        let margin = 2.0;

        let neighbors = octree.find_boundary_neighbors(pos_near_boundary, margin);
        assert!(!neighbors.is_empty());
        // Should find adjacent octant where x >= 50
        assert!(neighbors.iter().any(|b| b.min.0 >= 50.0));

        // Entity in the deep interior of child 0 (margin 1.0, distance 25.0) -> no boundary neighbors
        let pos_interior = (25.0, 25.0, 25.0);
        let no_neighbors = octree.find_boundary_neighbors(pos_interior, 1.0);
        assert!(no_neighbors.is_empty());
    }

    #[test]
    fn test_identical_positions_subdivision_guard() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::with_config(bounds, 4, 2);

        // Insert 20 entities at the exact same location
        for i in 0..20 {
            assert!(octree.insert(make_guid(i), (50.0, 50.0, 50.0)));
        }

        assert_eq!(octree.len(), 20);

        // Range query finds all 20
        let found = octree.query_radius((50.0, 50.0, 50.0), 1.0);
        assert_eq!(found.len(), 20);
    }

    #[test]
    fn test_zero_allocation_query_into() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::with_config(bounds, 2, 1);

        octree.insert(make_guid(1), (10.0, 10.0, 10.0));
        octree.insert(make_guid(2), (12.0, 10.0, 10.0));
        octree.insert(make_guid(3), (50.0, 50.0, 50.0));

        let mut guids_buf = Vec::new();
        octree.query_radius_into((10.0, 10.0, 10.0), 5.0, &mut guids_buf);
        assert_eq!(guids_buf.len(), 2);
        assert!(guids_buf.contains(&make_guid(1)));
        assert!(guids_buf.contains(&make_guid(2)));

        let mut pos_buf = Vec::new();
        octree.query_radius_with_pos_into((10.0, 10.0, 10.0), 5.0, &mut pos_buf);
        assert_eq!(pos_buf.len(), 2);

        let mut dist_buf = Vec::new();
        octree.query_radius_with_dist_sq_into((10.0, 10.0, 10.0), 5.0, &mut dist_buf);
        assert_eq!(dist_buf.len(), 2);
        for &(g, pos, dist_sq) in &dist_buf {
            if g == make_guid(1) {
                assert_eq!(pos, (10.0, 10.0, 10.0));
                assert_eq!(dist_sq, 0.0);
            } else if g == make_guid(2) {
                assert_eq!(pos, (12.0, 10.0, 10.0));
                assert_eq!(dist_sq, 4.0);
            }
        }

        let mut aabb_buf = Vec::new();
        let aabb = BoundingBox3D::new((0.0, 0.0, 0.0), (20.0, 20.0, 20.0));
        octree.query_range_into(&aabb, &mut aabb_buf);
        assert_eq!(aabb_buf.len(), 2);
    }
}
