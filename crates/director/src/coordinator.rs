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

/// Default threshold of tick execution duration (in milliseconds) before triggering a split.
pub const DEFAULT_TICK_SATURATION_MS: f32 = 35.0;

/// Default cooldown duration between splits on a partition.
pub const DEFAULT_SPLIT_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

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
        BoundingBox3D::new(
            (bounds.min.0, bounds.min.1, bounds.min.2),
            (mid_x, mid_y, mid_z),
        ),
        // Index 1: +--
        BoundingBox3D::new(
            (mid_x, bounds.min.1, bounds.min.2),
            (bounds.max.0, mid_y, mid_z),
        ),
        // Index 2: -+-
        BoundingBox3D::new(
            (bounds.min.0, mid_y, bounds.min.2),
            (mid_x, bounds.max.1, mid_z),
        ),
        // Index 3: ++-
        BoundingBox3D::new(
            (mid_x, mid_y, bounds.min.2),
            (bounds.max.0, bounds.max.1, mid_z),
        ),
        // Index 4: --+
        BoundingBox3D::new(
            (bounds.min.0, bounds.min.1, mid_z),
            (mid_x, mid_y, bounds.max.2),
        ),
        // Index 5: +-+
        BoundingBox3D::new(
            (mid_x, bounds.min.1, mid_z),
            (bounds.max.0, mid_y, bounds.max.2),
        ),
        // Index 6: -++
        BoundingBox3D::new(
            (bounds.min.0, mid_y, mid_z),
            (mid_x, bounds.max.1, bounds.max.2),
        ),
        // Index 7: +++
        BoundingBox3D::new(
            (mid_x, mid_y, mid_z),
            (bounds.max.0, bounds.max.1, bounds.max.2),
        ),
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

    /// Recursively collects all leaves assigned to any of `dead_node_ids`.
    pub fn collect_orphaned_leaves(
        &self,
        dead_node_ids: &[u32],
        orphaned: &mut Vec<(BoundingBox3D, u32)>,
    ) {
        match self {
            GlobalOctreeNode::Leaf {
                bounds,
                assigned_node_id,
                ..
            } => {
                if let Some(nid) = *assigned_node_id {
                    if dead_node_ids.contains(&nid) {
                        orphaned.push((*bounds, nid));
                    }
                }
            }
            GlobalOctreeNode::Branch { children, .. } => {
                for child in children.iter() {
                    child.collect_orphaned_leaves(dead_node_ids, orphaned);
                }
            }
        }
    }

    /// Reassigns a specific leaf octant matching `target_bounds` to `new_node_id`.
    pub fn assign_leaf_node(
        &mut self,
        target_bounds: &BoundingBox3D,
        new_node_id: Option<u32>,
    ) -> bool {
        match self {
            GlobalOctreeNode::Leaf {
                bounds,
                assigned_node_id,
                ..
            } => {
                if bounds == target_bounds {
                    *assigned_node_id = new_node_id;
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
                    if child.assign_leaf_node(target_bounds, new_node_id) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Updates bounds of a leaf octant matching `old_bounds` to `new_bounds`.
    pub fn update_leaf_bounds(
        &mut self,
        old_bounds: &BoundingBox3D,
        new_bounds: BoundingBox3D,
    ) -> bool {
        match self {
            GlobalOctreeNode::Leaf { bounds, .. } => {
                if bounds == old_bounds {
                    *bounds = new_bounds;
                    true
                } else {
                    false
                }
            }
            GlobalOctreeNode::Branch { bounds, children } => {
                if !bounds.intersects(old_bounds) {
                    return false;
                }
                for child in children.iter_mut() {
                    if child.update_leaf_bounds(old_bounds, new_bounds) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// Record of an orphaned partition being reassigned to a surviving node.
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionRecoveryEvent {
    pub bounds: BoundingBox3D,
    pub previous_node_id: u32,
    pub new_node_id: u32,
    pub epoch: u64,
}

/// Global coordinator tracking cluster octree spatial partitioning and dynamic subdivision.
#[derive(Debug)]
pub struct GlobalOctreeCoordinator {
    pub split_threshold: u32,
    pub merge_threshold: u32,
    pub tick_saturation_ms: f32,
    pub split_cooldown: std::time::Duration,
    pub last_split_times: std::collections::HashMap<u32, std::time::Instant>,
    pub root: GlobalOctreeNode,
    pub spatial_index: Octree,
    pub topology_epoch: u64,
}

impl Default for GlobalOctreeCoordinator {
    fn default() -> Self {
        Self::new(DEFAULT_WORLD_BOUNDS)
    }
}

impl GlobalOctreeCoordinator {
    /// Creates a new `GlobalOctreeCoordinator` with the specified root bounds and default thresholds.
    pub fn new(root_bounds: BoundingBox3D) -> Self {
        Self::with_thresholds(
            root_bounds,
            DEFAULT_SPLIT_THRESHOLD,
            DEFAULT_MERGE_THRESHOLD,
        )
    }

    /// Creates a new `GlobalOctreeCoordinator` with custom split and merge thresholds.
    pub fn with_thresholds(
        root_bounds: BoundingBox3D,
        split_threshold: u32,
        merge_threshold: u32,
    ) -> Self {
        Self::with_full_thresholds(
            root_bounds,
            split_threshold,
            merge_threshold,
            DEFAULT_TICK_SATURATION_MS,
            DEFAULT_SPLIT_COOLDOWN,
        )
    }

    /// Creates a new `GlobalOctreeCoordinator` with custom thresholds, saturation time, and split cooldown.
    pub fn with_full_thresholds(
        root_bounds: BoundingBox3D,
        split_threshold: u32,
        merge_threshold: u32,
        tick_saturation_ms: f32,
        split_cooldown: std::time::Duration,
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
            tick_saturation_ms,
            split_cooldown,
            last_split_times: std::collections::HashMap::new(),
            root,
            spatial_index,
            topology_epoch: 1,
        }
    }

    /// Returns the current generational topology epoch.
    pub fn topology_epoch(&self) -> u64 {
        self.topology_epoch
    }

    /// Returns `true` if an octant's reported entity count meets or exceeds `split_threshold`.
    pub fn should_split(&self, entity_count: u32) -> bool {
        entity_count >= self.split_threshold
    }

    /// Returns `true` if a node should split based on entity count or tick duration, respecting the split cooldown.
    pub fn should_split_node(
        &self,
        node_id: u32,
        entity_count: u32,
        tick_duration_ms: f32,
    ) -> bool {
        if let Some(last_split) = self.last_split_times.get(&node_id) {
            if last_split.elapsed() < self.split_cooldown {
                return false;
            }
        }
        entity_count >= self.split_threshold || tick_duration_ms >= self.tick_saturation_ms
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
    pub fn evaluate_heartbeat(
        &self,
        node_id: u32,
        entity_count: u32,
        tick_duration_ms: f32,
    ) -> bool {
        self.should_split_node(node_id, entity_count, tick_duration_ms)
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

            // Sort other_nodes by composite load score
            other_nodes.sort_by(|&a, &b| {
                let na = registry.get_node(a);
                let nb = registry.get_node(b);
                match (na, nb) {
                    (Some(na), Some(nb)) => na
                        .load_score()
                        .partial_cmp(&nb.load_score())
                        .unwrap_or(std::cmp::Ordering::Equal)
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
        let mut assignments_arr: [(BoundingBox3D, u32); 8] =
            [(original_bounds, reporting_node_id); 8];
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
        for (i, (&child, &assigned_id)) in
            sub_bounds.iter().zip(assigned_node_ids.iter()).enumerate()
        {
            let guid = Guid::new(EntityType::Player, assigned_id as u16, i as u32);
            self.spatial_index.insert(guid, child.center());
        }

        self.last_split_times
            .insert(reporting_node_id, std::time::Instant::now());
        self.topology_epoch += 1;

        assigned_node_ids
    }

    /// Smooths the dividing boundary between two adjacent leaf nodes based on entity centroids.
    ///
    /// Clamps centroids within each node's assigned bounds.
    /// If entity density ratio exceeds 2.0x, calculates an elastic displacement:
    /// delta = min(25.0, 0.1 * ((count_a - count_b)/(count_a + count_b)) * distance)
    /// Damped by hysteresis (at least 1.0m shift required).
    /// Returns the adjusted bounding boxes if shifted.
    #[allow(clippy::too_many_arguments)]
    pub fn smooth_adjacent_boundaries(
        &mut self,
        node_a: u32,
        centroid_a: (f32, f32, f32),
        count_a: u32,
        node_b: u32,
        centroid_b: (f32, f32, f32),
        count_b: u32,
        registry: &mut NodeRegistry,
    ) -> Option<(BoundingBox3D, BoundingBox3D)> {
        let bounds_a_list = self.get_assigned_bounds_for_node(node_a);
        let bounds_b_list = self.get_assigned_bounds_for_node(node_b);

        for ba in &bounds_a_list {
            for bb in &bounds_b_list {
                // Check if ba and bb are adjacent along X axis (ba on left, bb on right)
                if (ba.max.0 - bb.min.0).abs() < 0.001
                    && (ba.min.1 - bb.min.1).abs() < 0.001
                    && (ba.max.1 - bb.max.1).abs() < 0.001
                    && (ba.min.2 - bb.min.2).abs() < 0.001
                    && (ba.max.2 - bb.max.2).abs() < 0.001
                {
                    // Strict anti-cheat coordinate clamping to assigned bounds
                    let ca_x = centroid_a.0.clamp(ba.min.0, ba.max.0);
                    let cb_x = centroid_b.0.clamp(bb.min.0, bb.max.0);

                    let na = count_a as f32;
                    let nb = count_b as f32;
                    if na + nb == 0.0 {
                        continue;
                    }

                    // Check density ratio >= 2.0x
                    if na >= 2.0 * nb || nb >= 2.0 * na {
                        let imbalance = (na - nb) / (na + nb);
                        let dist = (cb_x - ca_x).abs().max(10.0);
                        let shift = (imbalance * 0.1 * dist).clamp(-25.0, 25.0);

                        if shift.abs() >= 1.0 {
                            let new_split_x = ba.max.0 + shift;
                            // Ensure bounds do not collapse
                            if new_split_x > ba.min.0 + 10.0 && new_split_x < bb.max.0 - 10.0 {
                                let new_ba =
                                    BoundingBox3D::new(ba.min, (new_split_x, ba.max.1, ba.max.2));
                                let new_bb =
                                    BoundingBox3D::new((new_split_x, bb.min.1, bb.min.2), bb.max);

                                self.root.update_leaf_bounds(ba, new_ba);
                                self.root.update_leaf_bounds(bb, new_bb);

                                registry.remove_assigned_bounds(node_a, ba);
                                registry.add_assigned_bounds(node_a, new_ba);
                                registry.remove_assigned_bounds(node_b, bb);
                                registry.add_assigned_bounds(node_b, new_bb);

                                self.topology_epoch += 1;
                                metrics::counter!("mmo_boundary_adjustments_total").increment(1);
                                return Some((new_ba, new_bb));
                            }
                        }
                    }
                }
            }
        }
        None
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
            self.topology_epoch += 1;
        }

        merged
    }

    /// Scans the topology tree for leaf octants assigned to evicted/dead simulation nodes,
    /// reassigning each orphaned partition to active surviving nodes balancing cluster load.
    ///
    /// Returns a list of `PartitionRecoveryEvent`s describing the reassignments.
    /// If no active nodes remain, the partitions are set to unassigned (`None`).
    pub fn recover_orphaned_partitions(
        &mut self,
        dead_node_ids: &[u32],
        registry: &mut NodeRegistry,
    ) -> Vec<PartitionRecoveryEvent> {
        if dead_node_ids.is_empty() {
            return Vec::new();
        }

        let mut orphaned = Vec::new();
        self.root
            .collect_orphaned_leaves(dead_node_ids, &mut orphaned);

        if orphaned.is_empty() {
            return Vec::new();
        }

        self.topology_epoch += 1;
        let current_epoch = self.topology_epoch;
        let mut events = Vec::with_capacity(orphaned.len());

        for (bounds, prev_node_id) in orphaned {
            let active_ids = registry.active_node_ids();
            let target_node_id = if active_ids.is_empty() {
                None
            } else {
                registry.find_least_loaded_node(&[])
            };

            self.root.assign_leaf_node(&bounds, target_node_id);

            // Clean up spatial index entry for the previous dead node
            let center = bounds.center();
            let query = self.spatial_index.query_radius(center, 0.1);
            for guid in query {
                if guid.node_id() as u32 == prev_node_id {
                    self.spatial_index.remove(guid, center);
                }
            }

            if let Some(new_node_id) = target_node_id {
                registry.add_assigned_bounds(new_node_id, bounds);
                let guid = Guid::new(EntityType::Player, new_node_id as u16, 0);
                self.spatial_index.insert(guid, center);

                events.push(PartitionRecoveryEvent {
                    bounds,
                    previous_node_id: prev_node_id,
                    new_node_id,
                    epoch: current_epoch,
                });
            }
        }

        events
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
        assert_eq!(
            children[0],
            BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 50.0, 50.0))
        );
        // Check child 7: +++ -> (50..100, 50..100, 50..100)
        assert_eq!(
            children[7],
            BoundingBox3D::new((50.0, 50.0, 50.0), (100.0, 100.0, 100.0))
        );

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

        // Evaluate heartbeat split recommendation (entity count or tick duration saturation)
        assert!(!coord.evaluate_heartbeat(1, 40, 15.0));
        assert!(coord.evaluate_heartbeat(1, 60, 15.0));
        assert!(coord.evaluate_heartbeat(1, 10, 36.0));

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
        assert_eq!(
            coord.find_node_for_point((75.0, 25.0, 25.0)),
            Some(expected_node_child1)
        );

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
            GlobalOctreeNode::Leaf {
                assigned_node_id,
                bounds: b,
                ..
            } => {
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

    #[test]
    fn test_recover_orphaned_partitions_reassigns_leaves_and_increments_epoch() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node("127.0.0.1:50051".to_string(), vec![bounds]);
        let node2 = registry.register_node("127.0.0.1:50052".to_string(), vec![]);
        coord.assign_initial_node(node1);

        assert_eq!(coord.topology_epoch(), 1);

        // Split into 8 leaves (assigns some to node1 and some to node2)
        let _ = coord.handle_split(node1, bounds, vec![], &mut registry);
        assert_eq!(coord.topology_epoch(), 2);

        // Simulate node1 dying: evict from registry
        let _ = registry.deregister_node(node1);
        assert_eq!(registry.active_node_ids(), vec![node2]);

        // Recover orphaned partitions belonging to node1
        let events = coord.recover_orphaned_partitions(&[node1], &mut registry);
        assert!(!events.is_empty());
        assert_eq!(coord.topology_epoch(), 3);

        for event in &events {
            assert_eq!(event.previous_node_id, node1);
            assert_eq!(event.new_node_id, node2);
            assert_eq!(event.epoch, 3);
        }

        // Verify no point inside bounds points to dead node1
        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), Some(node2));
        assert_eq!(coord.find_node_for_point((75.0, 75.0, 75.0)), Some(node2));

        // Registry for node2 should now have all 8 bounds
        let reg_node2 = registry.get_node(node2).unwrap();
        assert_eq!(reg_node2.assigned_bounds.len(), 8);
    }

    #[test]
    fn test_recover_orphaned_partitions_total_cluster_blackout() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node("127.0.0.1:50051".to_string(), vec![bounds]);
        coord.assign_initial_node(node1);

        // All nodes die
        let _ = registry.deregister_node(node1);
        assert!(registry.active_node_ids().is_empty());

        let events = coord.recover_orphaned_partitions(&[node1], &mut registry);
        // Since no active node exists, events is empty (unassigned rather than reassigned)
        assert!(events.is_empty());
        assert_eq!(coord.topology_epoch(), 2);

        // Partition is now unassigned
        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), None);
    }

    #[test]
    fn test_saturation_split_and_cooldown() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node("127.0.0.1:50051".to_string(), vec![bounds]);
        coord.assign_initial_node(node1);

        // Before split: split recommendation fires on saturation
        assert!(coord.should_split_node(node1, 10, 40.0));

        // Perform split
        let _ = coord.handle_split(node1, bounds, vec![], &mut registry);

        // Immediately after split: cooldown prevents split even under high saturation
        assert!(!coord.should_split_node(node1, 1000, 50.0));
    }

    #[test]
    fn test_smooth_adjacent_boundaries() {
        let root_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (200.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(root_bounds);
        let mut registry = NodeRegistry::new();

        let bound_a = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let bound_b = BoundingBox3D::new((100.0, 0.0, 0.0), (200.0, 100.0, 100.0));

        let node_a = registry.register_node("127.0.0.1:50051".to_string(), vec![bound_a]);
        let node_b = registry.register_node("127.0.0.1:50052".to_string(), vec![bound_b]);

        // Manually setup coordinator root with these two leaf octants as a branch or leaf assignments
        let mut leaves_arr = [(bound_a, node_a); 8];
        leaves_arr[0] = (bound_a, node_a);
        leaves_arr[1] = (bound_b, node_b);
        for item in leaves_arr.iter_mut().skip(2) {
            *item = (bound_b, node_b);
        }
        coord.root = GlobalOctreeNode::Branch {
            bounds: root_bounds,
            children: Box::new([
                GlobalOctreeNode::Leaf {
                    bounds: bound_a,
                    assigned_node_id: Some(node_a),
                    entity_count: 300,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 50,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_b,
                    assigned_node_id: Some(node_b),
                    entity_count: 0,
                },
            ]),
        };

        let initial_epoch = coord.topology_epoch();

        // Node A has 300 entities (hotspot), Node B has 50 entities (ratio 6:1 > 2.0x)
        // With extreme spoofed coordinate (99999.0), it should be clamped inside bound_a
        let smoothed = coord.smooth_adjacent_boundaries(
            node_a,
            (99999.0, 50.0, 50.0), // spoofed coordinate clamped to 100.0
            300,
            node_b,
            (150.0, 50.0, 50.0),
            50,
            &mut registry,
        );

        assert!(smoothed.is_some());
        let (new_ba, new_bb) = smoothed.unwrap();
        // Boundary should shift to the right (> 100.0) towards Node B
        assert!(new_ba.max.0 > 100.0);
        assert_eq!(new_ba.max.0, new_bb.min.0);
        assert_eq!(coord.topology_epoch(), initial_epoch + 1);

        // Registry should also reflect updated bounds
        let reg_a = registry.get_node(node_a).unwrap();
        assert_eq!(reg_a.assigned_bounds, vec![new_ba]);
        let reg_b = registry.get_node(node_b).unwrap();
        assert_eq!(reg_b.assigned_bounds, vec![new_bb]);
    }
}
