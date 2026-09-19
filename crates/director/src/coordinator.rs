//! Global Octree Coordinator for distributed spatial topology.
//!
//! Tracks cluster-wide octree partition bounds, evaluates node density thresholds,
//! issues split and merge commands, and assigns sub-octants to simulation nodes.

use mmo_core::{BoundingBox3D, EntityType, Guid, Octree};

use crate::registry::NodeRegistry;

/// Default threshold of entities per octant before triggering a split.
pub const DEFAULT_SPLIT_THRESHOLD: u32 = 50;

/// Default threshold of total entities in an octant's subtree before merging.
pub const DEFAULT_MERGE_THRESHOLD: u32 = 10;

/// Default global bounding box covering the entire world.
pub const DEFAULT_WORLD_BOUNDS: BoundingBox3D = BoundingBox3D {
    min: (-1000.0, -1000.0, -1000.0),
    max: (1000.0, 1000.0, 1000.0),
};

/// Subdivides a 3D bounding box into 8 equal child octants.
pub fn subdivide_bounds(bounds: &BoundingBox3D) -> [BoundingBox3D; 8] {
    let mid_x = (bounds.min.0 + bounds.max.0) * 0.5;
    let mid_y = (bounds.min.1 + bounds.max.1) * 0.5;
    let mid_z = (bounds.min.2 + bounds.max.2) * 0.5;

    [
        // Index 0: ---
        BoundingBox3D::new((bounds.min.0, bounds.min.1, bounds.min.2), (mid_x, mid_y, mid_z)),
        // Index 1: +--
        BoundingBox3D::new((mid_x, bounds.min.1, bounds.min.2), (bounds.max.0, mid_y, mid_z)),
        // Index 2: -+-
        BoundingBox3D::new((bounds.min.0, mid_y, bounds.min.2), (mid_x, bounds.max.1, mid_z)),
        // Index 3: ++-
        BoundingBox3D::new((mid_x, mid_y, bounds.min.2), (bounds.max.0, bounds.max.1, mid_z)),
        // Index 4: --+
        BoundingBox3D::new((bounds.min.0, bounds.min.1, mid_z), (mid_x, mid_y, bounds.max.2)),
        // Index 5: +-+
        BoundingBox3D::new((mid_x, bounds.min.1, mid_z), (bounds.max.0, mid_y, bounds.max.2)),
        // Index 6: -++
        BoundingBox3D::new((bounds.min.0, mid_y, mid_z), (mid_x, bounds.max.1, bounds.max.2)),
        // Index 7: +++
        BoundingBox3D::new((mid_x, mid_y, mid_z), (bounds.max.0, bounds.max.1, bounds.max.2)),
    ]
}

/// Node representation in the global octree topology hierarchy.
#[derive(Debug, Clone, PartialEq)]
pub enum GlobalOctreeNode {
    Leaf {
        bounds: BoundingBox3D,
        assigned_node_id: Option<u32>,
        entity_count: u32,
    },
    Branch {
        bounds: BoundingBox3D,
        children: Box<[GlobalOctreeNode; 8]>,
    },
}

impl GlobalOctreeNode {
    /// Returns the bounding box of this octree node.
    pub fn bounds(&self) -> &BoundingBox3D {
        match self {
            GlobalOctreeNode::Leaf { bounds, .. } => bounds,
            GlobalOctreeNode::Branch { bounds, .. } => bounds,
        }
    }

    /// Finds the assigned simulation node ID for the given 3D position, if within bounds.
    pub fn find_node(&self, pos: (f32, f32, f32)) -> Option<u32> {
        match self {
            GlobalOctreeNode::Leaf {
                bounds,
                assigned_node_id,
                ..
            } => {
                if bounds.contains(pos) {
                    *assigned_node_id
                } else {
                    None
                }
            }
            GlobalOctreeNode::Branch { bounds, children } => {
                if !bounds.contains(pos) {
                    return None;
                }
                let idx = bounds.child_index(pos);
                if let Some(node_id) = children[idx].find_node(pos) {
                    return Some(node_id);
                }
                // Fallback for edge/boundary coordinates
                for child in children.iter() {
                    if let Some(node_id) = child.find_node(pos) {
                        return Some(node_id);
                    }
                }
                None
            }
        }
    }

    /// Recursively collects all leaf octants with their assigned node IDs.
    pub fn collect_leaves<'a>(&'a self, out: &mut Vec<(&'a BoundingBox3D, Option<u32>)>) {
        match self {
            GlobalOctreeNode::Leaf {
                bounds,
                assigned_node_id,
                ..
            } => {
                out.push((bounds, *assigned_node_id));
            }
            GlobalOctreeNode::Branch { children, .. } => {
                for child in children.iter() {
                    child.collect_leaves(out);
                }
            }
        }
    }

    /// Recursively collects all bounds assigned to a specific node ID.
    pub fn collect_assigned_bounds(&self, node_id: u32, out: &mut Vec<BoundingBox3D>) {
        match self {
            GlobalOctreeNode::Leaf {
                bounds,
                assigned_node_id,
                ..
            } => {
                if *assigned_node_id == Some(node_id) {
                    out.push(*bounds);
                }
            }
            GlobalOctreeNode::Branch { children, .. } => {
                for child in children.iter() {
                    child.collect_assigned_bounds(node_id, out);
                }
            }
        }
    }

    /// Splits the leaf node matching `target_bounds` into 8 child leaves.
    pub fn split_at(
        &mut self,
        target_bounds: &BoundingBox3D,
        assignments: &[(BoundingBox3D, u32); 8],
    ) -> bool {
        match self {
            GlobalOctreeNode::Leaf { bounds, .. } => {
                if bounds == target_bounds {
                    let children_vec: Vec<GlobalOctreeNode> = assignments
                        .iter()
                        .map(|(cb, nid)| GlobalOctreeNode::Leaf {
                            bounds: *cb,
                            assigned_node_id: Some(*nid),
                            entity_count: 0,
                        })
                        .collect();

                    let children: Box<[GlobalOctreeNode; 8]> =
                        match children_vec.into_boxed_slice().try_into() {
                            Ok(b) => b,
                            Err(_) => unreachable!("Vector length is guaranteed to be 8"),
                        };

                    *self = GlobalOctreeNode::Branch {
                        bounds: *target_bounds,
                        children,
                    };
                    true
                } else {
                    false
                }
            }
            GlobalOctreeNode::Branch { bounds, children } => {
                if !bounds.intersects(target_bounds) {
                    return false;
                }
                for child in children.iter_mut() {
                    if child.split_at(target_bounds, assignments) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Finds leaves under a specific branch matching `parent_bounds`.
    pub fn find_branch_leaves(
        &self,
        parent_bounds: &BoundingBox3D,
    ) -> Option<Vec<(BoundingBox3D, Option<u32>)>> {
        match self {
            GlobalOctreeNode::Leaf { .. } => None,
            GlobalOctreeNode::Branch { bounds, children } => {
                if bounds == parent_bounds {
                    let mut leaves = Vec::new();
                    for child in children.iter() {
                        child.collect_leaves(&mut leaves);
                    }
                    Some(leaves.into_iter().map(|(b, nid)| (*b, nid)).collect())
                } else {
                    for child in children.iter() {
                        if let Some(res) = child.find_branch_leaves(parent_bounds) {
                            return Some(res);
                        }
                    }
                    None
                }
            }
        }
    }

    /// Collapses the branch matching `parent_bounds` into a single leaf assigned to `new_node_id`.
    pub fn merge_at(&mut self, parent_bounds: &BoundingBox3D, new_node_id: u32) -> bool {
        match self {
            GlobalOctreeNode::Leaf { .. } => false,
            GlobalOctreeNode::Branch { bounds, children } => {
                if bounds == parent_bounds {
                    *self = GlobalOctreeNode::Leaf {
                        bounds: *parent_bounds,
                        assigned_node_id: Some(new_node_id),
                        entity_count: 0,
                    };
                    true
                } else {
                    for child in children.iter_mut() {
                        if child.merge_at(parent_bounds, new_node_id) {
                            return true;
                        }
                    }
                    false
                }
            }
        }
    }
}

/// Global coordinator tracking cluster octree spatial partitioning and dynamic subdivision.
#[derive(Debug)]
pub struct GlobalOctreeCoordinator {
    pub split_threshold: u32,
    pub merge_threshold: u32,
    pub root: GlobalOctreeNode,
    pub spatial_index: Octree,
}

impl Default for GlobalOctreeCoordinator {
    fn default() -> Self {
        Self::new(DEFAULT_WORLD_BOUNDS)
    }
}

impl GlobalOctreeCoordinator {
    /// Creates a new `GlobalOctreeCoordinator` with the specified root bounds and default thresholds.
    pub fn new(root_bounds: BoundingBox3D) -> Self {
        Self::with_thresholds(root_bounds, DEFAULT_SPLIT_THRESHOLD, DEFAULT_MERGE_THRESHOLD)
    }

    /// Creates a new `GlobalOctreeCoordinator` with custom split and merge thresholds.
    pub fn with_thresholds(
        root_bounds: BoundingBox3D,
        split_threshold: u32,
        merge_threshold: u32,
    ) -> Self {
        let root = GlobalOctreeNode::Leaf {
            bounds: root_bounds,
            assigned_node_id: None,
            entity_count: 0,
        };

        let spatial_index = Octree::new(root_bounds);

        Self {
            split_threshold,
            merge_threshold,
            root,
            spatial_index,
        }
    }

    /// Returns `true` if an octant's reported entity count meets or exceeds `split_threshold`.
    pub fn should_split(&self, entity_count: u32) -> bool {
        entity_count >= self.split_threshold
    }

    /// Returns `true` if an octant's entity count drops to or below `merge_threshold`.
    pub fn should_merge(&self, entity_count: u32) -> bool {
        entity_count <= self.merge_threshold
    }

    /// Assigns the root octant to an initial node if it is currently unassigned.
    pub fn assign_initial_node(&mut self, node_id: u32) {
        if let GlobalOctreeNode::Leaf {
            assigned_node_id,
            bounds,
            ..
        } = &mut self.root
        {
            if assigned_node_id.is_none() {
                *assigned_node_id = Some(node_id);
                let guid = Guid::new(EntityType::Player, node_id as u16, 0);
                self.spatial_index.insert(guid, bounds.center());
            }
        }
    }

    /// Finds the authoritative simulation node ID for the given position.
    pub fn find_node_for_point(&self, pos: (f32, f32, f32)) -> Option<u32> {
        self.root.find_node(pos)
    }

    /// Returns all bounding boxes assigned to the specified node ID.
    pub fn get_assigned_bounds_for_node(&self, node_id: u32) -> Vec<BoundingBox3D> {
        let mut bounds = Vec::new();
        self.root.collect_assigned_bounds(node_id, &mut bounds);
        bounds
    }

    /// Returns all leaf octants with their assigned node IDs.
    pub fn leaf_octants(&self) -> Vec<(BoundingBox3D, Option<u32>)> {
        let mut leaves = Vec::new();
        self.root.collect_leaves(&mut leaves);
        leaves.into_iter().map(|(b, nid)| (*b, nid)).collect()
    }

    /// Evaluates whether an incoming heartbeat triggers a split recommendation.
    pub fn evaluate_heartbeat(&self, _node_id: u32, entity_count: u32) -> bool {
        self.should_split(entity_count)
    }

    /// Handles an octant split report from a simulation node.
    ///
    /// Subdivides the target octant into 8 child octants, assigns each child
    /// to available nodes (balancing across least-loaded nodes), updates the node registry,
    /// and returns the 8 assigned node IDs.
    pub fn handle_split(
        &mut self,
        reporting_node_id: u32,
        original_bounds: BoundingBox3D,
        child_bounds: Vec<BoundingBox3D>,
        registry: &mut NodeRegistry,
    ) -> Vec<u32> {
        // 1. Determine child bounds (subdivide if not provided or incomplete)
        let sub_bounds: [BoundingBox3D; 8] = if child_bounds.len() == 8 {
            child_bounds
                .try_into()
                .expect("Guaranteed length 8 by condition")
        } else {
            subdivide_bounds(&original_bounds)
        };

        // 2. Select node assignments for the 8 sub-octants
        let mut active_nodes = registry.active_node_ids();
        if active_nodes.is_empty() {
            active_nodes.push(reporting_node_id);
        }

        let mut assigned_node_ids = Vec::with_capacity(8);

        if active_nodes.len() == 1 {
            // Only 1 node registered: it retains all 8 children
            let target_node = active_nodes[0];
            for _ in 0..8 {
                assigned_node_ids.push(target_node);
            }
        } else {
            // Distribute across nodes balancing load
            let mut other_nodes: Vec<u32> = active_nodes
                .iter()
                .copied()
                .filter(|&id| id != reporting_node_id)
                .collect();

            // Sort other_nodes by load (entity_count, cpu_percent, node_id)
            other_nodes.sort_by(|&a, &b| {
                let na = registry.get_node(a);
                let nb = registry.get_node(b);
                match (na, nb) {
                    (Some(na), Some(nb)) => na
                        .entity_count
                        .cmp(&nb.entity_count)
                        .then_with(|| {
                            na.cpu_percent
                                .partial_cmp(&nb.cpu_percent)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .then_with(|| na.node_id.cmp(&nb.node_id)),
                    _ => a.cmp(&b),
                }
            });

            for (i, _) in sub_bounds.iter().enumerate() {
                if i == 0 {
                    // Retain reporting node on child 0 for spatial continuity
                    assigned_node_ids.push(reporting_node_id);
                } else if !other_nodes.is_empty() {
                    // Round-robin over other available least-loaded nodes
                    let chosen = other_nodes[(i - 1) % other_nodes.len()];
                    assigned_node_ids.push(chosen);
                } else {
                    assigned_node_ids.push(reporting_node_id);
                }
            }
        }

        // 3. Update NodeRegistry assigned bounds
        registry.remove_assigned_bounds(reporting_node_id, &original_bounds);
        for (&child, &assigned_id) in sub_bounds.iter().zip(assigned_node_ids.iter()) {
            registry.add_assigned_bounds(assigned_id, child);
        }

        // 4. Update coordinator topology tree
        let mut assignments_arr: [(BoundingBox3D, u32); 8] = [(original_bounds, reporting_node_id); 8];
        for i in 0..8 {
            assignments_arr[i] = (sub_bounds[i], assigned_node_ids[i]);
        }
        self.root.split_at(&original_bounds, &assignments_arr);

        // 5. Update spatial index
        let center = original_bounds.center();
        let query_guids = self.spatial_index.query_radius(center, 0.1);
        for guid in query_guids {
            if guid.node_id() as u32 == reporting_node_id {
                self.spatial_index.remove(guid, center);
            }
        }
        for (i, (&child, &assigned_id)) in sub_bounds.iter().zip(assigned_node_ids.iter()).enumerate() {
            let guid = Guid::new(EntityType::Player, assigned_id as u16, i as u32);
            self.spatial_index.insert(guid, child.center());
        }

        assigned_node_ids
    }

    /// Handles merging 8 child octants back into a single parent octant.
    ///
    /// Reassigns the combined parent bounds to `target_node_id` and cleans up child bounds.
    pub fn handle_merge(
        &mut self,
        parent_bounds: BoundingBox3D,
        target_node_id: u32,
        registry: &mut NodeRegistry,
    ) -> bool {
        // Collect existing child leaves under this parent
        let child_leaves = match self.root.find_branch_leaves(&parent_bounds) {
            Some(leaves) => leaves,
            None => return false,
        };

        // Remove child bounds from each assigned node in the registry
        for (child_bound, assigned_nid) in child_leaves {
            if let Some(nid) = assigned_nid {
                registry.remove_assigned_bounds(nid, &child_bound);
                let center = child_bound.center();
                let query = self.spatial_index.query_radius(center, 0.1);
                for guid in query {
                    if guid.node_id() as u32 == nid {
                        self.spatial_index.remove(guid, center);
                    }
                }
            }
        }

        // Add parent bounds to the target node
        registry.add_assigned_bounds(target_node_id, parent_bounds);

        // Collapse branch in topology
        let merged = self.root.merge_at(&parent_bounds, target_node_id);

        if merged {
            let guid = Guid::new(EntityType::Player, target_node_id as u16, 0);
            self.spatial_index.insert(guid, parent_bounds.center());
        }

        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subdivide_bounds() {
        let parent = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let children = subdivide_bounds(&parent);

        assert_eq!(children.len(), 8);

        // Check child 0: --- -> (0..50, 0..50, 0..50)
        assert_eq!(children[0], BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 50.0, 50.0)));
        // Check child 7: +++ -> (50..100, 50..100, 50..100)
        assert_eq!(children[7], BoundingBox3D::new((50.0, 50.0, 50.0), (100.0, 100.0, 100.0)));

        // Child centers
        assert_eq!(children[0].center(), (25.0, 25.0, 25.0));
        assert_eq!(children[7].center(), (75.0, 75.0, 75.0));

        // Dimension of each child is half parent's dimension
        for child in &children {
            assert_eq!(child.dimensions(), (50.0, 50.0, 50.0));
        }
    }

    #[test]
    fn test_octree_split_command_generation_when_density_crosses_threshold() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let coord = GlobalOctreeCoordinator::new(bounds);

        // Default threshold is 50
        assert_eq!(coord.split_threshold, DEFAULT_SPLIT_THRESHOLD);
        assert!(!coord.should_split(0));
        assert!(!coord.should_split(49));
        assert!(coord.should_split(50));
        assert!(coord.should_split(1000));

        // Evaluate heartbeat split recommendation
        assert!(!coord.evaluate_heartbeat(1, 40));
        assert!(coord.evaluate_heartbeat(1, 60));

        // Custom thresholds
        let custom_coord = GlobalOctreeCoordinator::with_thresholds(bounds, 30, 8);
        assert!(!custom_coord.should_split(29));
        assert!(custom_coord.should_split(30));
        assert!(custom_coord.should_merge(8));
        assert!(!custom_coord.should_merge(9));
    }

    #[test]
    fn test_handle_split_single_node() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node_id = registry.register_node("127.0.0.1:50051".to_string(), vec![bounds]);
        coord.assign_initial_node(node_id);

        assert_eq!(coord.find_node_for_point((10.0, 10.0, 10.0)), Some(node_id));

        // Trigger split with no explicit child bounds (coordinator generates them)
        let assigned_ids = coord.handle_split(node_id, bounds, vec![], &mut registry);
        assert_eq!(assigned_ids.len(), 8);
        for &id in &assigned_ids {
            assert_eq!(id, node_id);
        }

        // Registry should now have 8 child bounds and 0 for the original bounds
        let node = registry.get_node(node_id).unwrap();
        assert_eq!(node.assigned_bounds.len(), 8);
        assert!(!node.assigned_bounds.contains(&bounds));

        // Any point in bounds should still route to node_id
        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), Some(node_id));
        assert_eq!(coord.find_node_for_point((75.0, 75.0, 75.0)), Some(node_id));
    }

    #[test]
    fn test_handle_split_and_node_assignment_multi_node() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node("127.0.0.1:50051".to_string(), vec![bounds]);
        let node2 = registry.register_node("127.0.0.1:50052".to_string(), vec![]);
        coord.assign_initial_node(node1);

        // Before split, all points route to node1
        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), Some(node1));
        assert_eq!(coord.find_node_for_point((75.0, 25.0, 25.0)), Some(node1));

        // Split original bounds
        let assigned_ids = coord.handle_split(node1, bounds, vec![], &mut registry);
        assert_eq!(assigned_ids.len(), 8);

        // Child 0 retained by node1
        assert_eq!(assigned_ids[0], node1);
        // At least some children assigned to least loaded node2
        assert!(assigned_ids.contains(&node2));

        // Point inside child 0 (0..50, 0..50, 0..50) routes to node1
        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), Some(node1));

        // Point in child 1 (50..100, 0..50, 0..50) routes to assigned node
        let expected_node_child1 = assigned_ids[1];
        assert_eq!(coord.find_node_for_point((75.0, 25.0, 25.0)), Some(expected_node_child1));

        // Verify bounds in registry
        let reg_node1 = registry.get_node(node1).unwrap();
        let reg_node2 = registry.get_node(node2).unwrap();
        assert!(!reg_node1.assigned_bounds.is_empty());
        assert!(!reg_node2.assigned_bounds.is_empty());
    }

    #[test]
    fn test_handle_merge_collapses_branches() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node("127.0.0.1:50051".to_string(), vec![bounds]);
        let node2 = registry.register_node("127.0.0.1:50052".to_string(), vec![]);
        coord.assign_initial_node(node1);

        // Split
        coord.handle_split(node1, bounds, vec![], &mut registry);
        assert!(matches!(coord.root, GlobalOctreeNode::Branch { .. }));

        // Merge back into node2
        let merged = coord.handle_merge(bounds, node2, &mut registry);
        assert!(merged);

        // Root is now a leaf assigned to node2
        match &coord.root {
            GlobalOctreeNode::Leaf { assigned_node_id, bounds: b, .. } => {
                assert_eq!(*assigned_node_id, Some(node2));
                assert_eq!(*b, bounds);
            }
            _ => panic!("Root should have collapsed to Leaf"),
        }

        // Routing now points to node2 everywhere
        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), Some(node2));
        assert_eq!(coord.find_node_for_point((75.0, 75.0, 75.0)), Some(node2));

        // Registry bounds updated
        let reg_node2 = registry.get_node(node2).unwrap();
        assert!(reg_node2.assigned_bounds.contains(&bounds));
    }
}
