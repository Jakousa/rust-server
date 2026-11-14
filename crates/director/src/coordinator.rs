//! Global Octree Coordinator for distributed spatial topology.
//!
//! Tracks cluster-wide octree partition bounds, evaluates node density thresholds,
//! issues split and merge commands, and assigns sub-octants to simulation nodes.

use mmo_core::BoundingBox3D;

use crate::registry::NodeRegistry;

/// Default threshold of entities per octant before triggering a split.
pub const DEFAULT_SPLIT_THRESHOLD: u32 = 50;

/// Default threshold of total entities in an octant's subtree before merging.
pub const DEFAULT_MERGE_THRESHOLD: u32 = 10;

/// Default threshold of tick execution duration (in milliseconds) before triggering a split.
pub const DEFAULT_TICK_SATURATION_MS: f32 = 35.0;

/// Default cooldown duration between splits on a partition.
pub const DEFAULT_SPLIT_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

/// Minimum dimensional extent (in world units) along every axis required for a partition to split.
pub const MIN_SPLIT_EXTENT: f32 = 25.0;

/// Default global bounding box covering the entire world.
pub const DEFAULT_WORLD_BOUNDS: BoundingBox3D = BoundingBox3D {
    min: (-1000.0, -1000.0, -1000.0),
    max: (1000.0, 1000.0, 1000.0),
};

/// Subdivides a 3D bounding box into 8 equal child octants.
pub fn subdivide_bounds(bounds: &BoundingBox3D) -> [BoundingBox3D; 8] {
    bounds.subdivide()
}

const BOUNDS_EPSILON: f32 = 1e-3;

fn bounds_approx_eq(a: &BoundingBox3D, b: &BoundingBox3D) -> bool {
    (a.min.0 - b.min.0).abs() < BOUNDS_EPSILON
        && (a.min.1 - b.min.1).abs() < BOUNDS_EPSILON
        && (a.min.2 - b.min.2).abs() < BOUNDS_EPSILON
        && (a.max.0 - b.max.0).abs() < BOUNDS_EPSILON
        && (a.max.1 - b.max.1).abs() < BOUNDS_EPSILON
        && (a.max.2 - b.max.2).abs() < BOUNDS_EPSILON
}

fn bounds_contains_box(outer: &BoundingBox3D, inner: &BoundingBox3D) -> bool {
    inner.min.0 >= outer.min.0 - BOUNDS_EPSILON
        && inner.min.1 >= outer.min.1 - BOUNDS_EPSILON
        && inner.min.2 >= outer.min.2 - BOUNDS_EPSILON
        && inner.max.0 <= outer.max.0 + BOUNDS_EPSILON
        && inner.max.1 <= outer.max.1 + BOUNDS_EPSILON
        && inner.max.2 <= outer.max.2 + BOUNDS_EPSILON
}

fn meets_min_split_extent(bounds: &BoundingBox3D) -> bool {
    let (dx, dy, dz) = bounds.dimensions();
    dx >= MIN_SPLIT_EXTENT && dy >= MIN_SPLIT_EXTENT && dz >= MIN_SPLIT_EXTENT
}

fn strip_http_scheme(s: &str) -> &str {
    if s.get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
    {
        &s[7..]
    } else if s
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
    {
        &s[8..]
    } else {
        s
    }
}

fn is_octree_descendant(outer: &BoundingBox3D, target: &BoundingBox3D, depth_left: usize) -> bool {
    if bounds_approx_eq(outer, target) {
        return true;
    }
    if depth_left == 0 || !bounds_contains_box(outer, target) {
        return false;
    }
    for child in subdivide_bounds(outer) {
        if bounds_contains_box(&child, target)
            && is_octree_descendant(&child, target, depth_left - 1)
        {
            return true;
        }
    }
    false
}

fn empty_slot_node() -> GlobalOctreeNode {
    GlobalOctreeNode::Leaf {
        bounds: BoundingBox3D::new((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)),
        assigned_node_id: None,
        entity_count: 0,
    }
}

fn is_empty_slot(node: &GlobalOctreeNode) -> bool {
    matches!(
        node,
        GlobalOctreeNode::Leaf {
            bounds,
            assigned_node_id: None,
            ..
        } if bounds.min == bounds.max
    )
}

/// Computes the union bounding box that tightly encloses both `a` and `b`.
pub fn union_bounds(a: &BoundingBox3D, b: &BoundingBox3D) -> BoundingBox3D {
    BoundingBox3D::new(
        (
            a.min.0.min(b.min.0),
            a.min.1.min(b.min.1),
            a.min.2.min(b.min.2),
        ),
        (
            a.max.0.max(b.max.0),
            a.max.1.max(b.max.1),
            a.max.2.max(b.max.2),
        ),
    )
}

/// Resolves a node's network listen address from registry metadata, normalizing empty,
/// missing, or unspecified bind addresses (`0.0.0.0`, `::`, `[::]`) to the Kubernetes
/// StatefulSet DNS convention `mmo-node-{id-1}.mmo-node:{port}` (defaulting port to `50052`).
pub fn fallback_node_addr(node_id: u32, registered: Option<&str>) -> String {
    let default_addr =
        |port: u16| -> String { format!("mmo-node-{}.mmo-node:{port}", node_id.saturating_sub(1)) };

    let Some(raw) = registered else {
        return default_addr(50052);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default_addr(50052);
    }

    let without_scheme = strip_http_scheme(trimmed);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    if authority.is_empty() {
        return default_addr(50052);
    }

    if let Ok(sock) = authority.parse::<std::net::SocketAddr>() {
        if sock.ip().is_unspecified() {
            let port = if sock.port() == 0 { 50052 } else { sock.port() };
            return default_addr(port);
        }
        return trimmed.to_string();
    }

    let bare = authority
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(authority);
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if ip.is_unspecified() {
            return default_addr(50052);
        }
        return trimmed.to_string();
    }

    if let Some((host, port_str)) = authority.rsplit_once(':') {
        let host_bare = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        if host_bare == "0.0.0.0"
            || host_bare == "::"
            || host_bare
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_unspecified())
        {
            let port = port_str
                .parse::<u16>()
                .ok()
                .filter(|&p| p > 0)
                .unwrap_or(50052);
            return default_addr(port);
        }
    }

    trimmed.to_string()
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
                if bounds_approx_eq(bounds, target_bounds) {
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
                        bounds: *bounds,
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
                    if !is_empty_slot(child) && child.split_at(target_bounds, assignments) {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn first_assigned_node(&self) -> Option<u32> {
        match self {
            GlobalOctreeNode::Leaf {
                assigned_node_id, ..
            } => *assigned_node_id,
            GlobalOctreeNode::Branch { children, .. } => children
                .iter()
                .filter(|c| !is_empty_slot(c))
                .find_map(|c| c.first_assigned_node()),
        }
    }

    fn find_split_assignments(&self, target: &BoundingBox3D) -> Option<Vec<u32>> {
        match self {
            GlobalOctreeNode::Leaf { .. } => None,
            GlobalOctreeNode::Branch { bounds, children } => {
                if bounds_approx_eq(bounds, target) {
                    let non_empty_count = children.iter().filter(|c| !is_empty_slot(c)).count();
                    if non_empty_count == 8 {
                        let ids = children
                            .iter()
                            .map(|c| match c {
                                GlobalOctreeNode::Leaf {
                                    assigned_node_id, ..
                                } => assigned_node_id.unwrap_or(0),
                                GlobalOctreeNode::Branch { .. } => {
                                    c.first_assigned_node().unwrap_or(0)
                                }
                            })
                            .collect();
                        return Some(ids);
                    }
                    return None;
                }
                for child in children.iter() {
                    if !is_empty_slot(child) && bounds_contains_box(child.bounds(), target) {
                        if let Some(ids) = child.find_split_assignments(target) {
                            return Some(ids);
                        }
                    }
                }
                None
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
                for child in children.iter_mut() {
                    if child.update_leaf_bounds(old_bounds, new_bounds) {
                        let mut b = *children[0].bounds();
                        for c in &children[1..] {
                            b = union_bounds(&b, c.bounds());
                        }
                        *bounds = b;
                        return true;
                    }
                }
                false
            }
        }
    }

    fn map_bounds(
        &mut self,
        node_id: u32,
        target: BoundingBox3D,
        registry: &mut Option<&mut NodeRegistry>,
    ) -> (bool, bool) {
        match self {
            GlobalOctreeNode::Leaf {
                bounds,
                assigned_node_id,
                ..
            } => {
                if bounds_approx_eq(bounds, &target) {
                    if let (Some(prev), Some(reg)) = (*assigned_node_id, registry.as_deref_mut()) {
                        if prev != node_id && reg.get_node(prev).is_some() {
                            if let Some(node) = reg.get_node_mut(node_id) {
                                node.assigned_bounds
                                    .retain(|b| !bounds_approx_eq(b, &target));
                            }
                            return (true, false);
                        }
                        if prev != node_id {
                            reg.remove_assigned_bounds(prev, bounds);
                        }
                    }
                    let changed = *assigned_node_id != Some(node_id);
                    *assigned_node_id = Some(node_id);
                    (true, changed)
                } else if bounds_contains_box(bounds, &target)
                    && is_octree_descendant(bounds, &target, 6)
                {
                    let existing_owner = *assigned_node_id;
                    let old_bounds = *bounds;
                    let sub = subdivide_bounds(&old_bounds);
                    if let (Some(prev), Some(reg)) = (existing_owner, registry.as_deref_mut()) {
                        reg.remove_assigned_bounds(prev, &old_bounds);
                        for b in &sub {
                            reg.add_assigned_bounds(prev, *b);
                        }
                    }
                    let children_vec: Vec<GlobalOctreeNode> = sub
                        .into_iter()
                        .map(|cb| GlobalOctreeNode::Leaf {
                            bounds: cb,
                            assigned_node_id: existing_owner,
                            entity_count: 0,
                        })
                        .collect();
                    let children: Box<[GlobalOctreeNode; 8]> = children_vec
                        .into_boxed_slice()
                        .try_into()
                        .expect("subdivide_bounds always produces 8 octants");
                    *self = GlobalOctreeNode::Branch {
                        bounds: old_bounds,
                        children,
                    };
                    let (mapped, _) = self.map_bounds(node_id, target, registry);
                    (mapped, true)
                } else if assigned_node_id.is_none() && bounds_contains_box(bounds, &target) {
                    let children = Box::new([
                        GlobalOctreeNode::Leaf {
                            bounds: target,
                            assigned_node_id: Some(node_id),
                            entity_count: 0,
                        },
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                    ]);
                    *self = GlobalOctreeNode::Branch {
                        bounds: *bounds,
                        children,
                    };
                    (true, true)
                } else {
                    (false, false)
                }
            }
            GlobalOctreeNode::Branch { bounds, children } => {
                if bounds_approx_eq(bounds, &target) {
                    if let Some(reg) = registry.as_deref_mut() {
                        let mut owned_leaves = Vec::new();
                        for child in children.iter() {
                            child.collect_assigned_bounds(node_id, &mut owned_leaves);
                        }
                        if let Some(node) = reg.get_node_mut(node_id) {
                            node.assigned_bounds
                                .retain(|b| !bounds_approx_eq(b, &target));
                            if node.assigned_bounds.is_empty() {
                                node.assigned_bounds.extend(owned_leaves);
                            }
                        }
                    }
                    return (true, false);
                }
                for child in children.iter_mut() {
                    if !is_empty_slot(child) && bounds_contains_box(child.bounds(), &target) {
                        let (mapped, changed) = child.map_bounds(node_id, target, registry);
                        if mapped {
                            return (true, changed);
                        }
                    }
                }
                for child in children.iter_mut() {
                    if is_empty_slot(child) {
                        *child = GlobalOctreeNode::Leaf {
                            bounds: target,
                            assigned_node_id: Some(node_id),
                            entity_count: 0,
                        };
                        *bounds = union_bounds(bounds, &target);
                        return (true, true);
                    }
                }
                let prev = std::mem::replace(&mut children[7], empty_slot_node());
                let branch_bounds = union_bounds(prev.bounds(), &target);
                children[7] = GlobalOctreeNode::Branch {
                    bounds: branch_bounds,
                    children: Box::new([
                        prev,
                        GlobalOctreeNode::Leaf {
                            bounds: target,
                            assigned_node_id: Some(node_id),
                            entity_count: 0,
                        },
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                        empty_slot_node(),
                    ]),
                };
                *bounds = union_bounds(bounds, &target);
                (true, true)
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
    pub previous_node_address: String,
    pub new_node_address: String,
    pub epoch: u64,
}

/// Context for an adjacent node participating in spatial boundary smoothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSmoothingContext {
    pub node_id: u32,
    pub centroid: (f32, f32, f32),
    pub entity_count: u32,
}

impl NodeSmoothingContext {
    pub fn new(node_id: u32, centroid: (f32, f32, f32), entity_count: u32) -> Self {
        Self {
            node_id,
            centroid,
            entity_count,
        }
    }
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

        Self {
            split_threshold,
            merge_threshold,
            tick_saturation_ms,
            split_cooldown,
            last_split_times: std::collections::HashMap::new(),
            root,
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

    /// Returns `true` if a node should split based on entity count or tick duration,
    /// respecting the split cooldown and minimum partition extent (`MIN_SPLIT_EXTENT`).
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
        if entity_count < self.split_threshold && tick_duration_ms < self.tick_saturation_ms {
            return false;
        }
        let leaves = self.leaf_octants();
        if leaves.iter().any(|(_, owner)| owner.is_some()) {
            leaves
                .iter()
                .any(|(bounds, owner)| *owner == Some(node_id) && meets_min_split_extent(bounds))
        } else {
            meets_min_split_extent(self.root.bounds())
        }
    }

    /// Returns `true` if an octant's entity count drops to or below `merge_threshold`.
    pub fn should_merge(&self, entity_count: u32) -> bool {
        entity_count <= self.merge_threshold
    }

    /// Assigns the root octant to an initial node if it is currently unassigned.
    pub fn assign_initial_node(&mut self, node_id: u32) {
        if let GlobalOctreeNode::Leaf {
            assigned_node_id, ..
        } = &mut self.root
        {
            if assigned_node_id.is_none() {
                *assigned_node_id = Some(node_id);
            }
        }
    }

    /// Maps or subdivides `bounds` in the global octree and assigns it to `node_id`.
    ///
    /// Supports initial root assignment, octree sub-octant mapping during multi-node startup
    /// or Director restart re-registration, and arbitrary sub-box placement.
    pub fn assign_or_map_node_bounds(&mut self, node_id: u32, bounds: BoundingBox3D) -> bool {
        self.assign_or_map_node_bounds_with_registry(node_id, bounds, None)
    }

    /// Maps or subdivides `bounds` in the global octree and assigns it to `node_id`,
    /// keeping `registry` bounds synchronized if an existing leaf is subdivided.
    pub fn assign_or_map_node_bounds_with_registry(
        &mut self,
        node_id: u32,
        bounds: BoundingBox3D,
        mut registry: Option<&mut NodeRegistry>,
    ) -> bool {
        if !bounds.min.0.is_finite()
            || !bounds.min.1.is_finite()
            || !bounds.min.2.is_finite()
            || !bounds.max.0.is_finite()
            || !bounds.max.1.is_finite()
            || !bounds.max.2.is_finite()
            || bounds.min.0 >= bounds.max.0
            || bounds.min.1 >= bounds.max.1
            || bounds.min.2 >= bounds.max.2
        {
            return false;
        }

        let was_unassigned_root = matches!(
            self.root,
            GlobalOctreeNode::Leaf {
                assigned_node_id: None,
                ..
            }
        );

        let (mapped, mut changed) = self.root.map_bounds(node_id, bounds, &mut registry);
        if !mapped {
            let old_root = std::mem::replace(&mut self.root, empty_slot_node());
            let new_root_bounds = union_bounds(old_root.bounds(), &bounds);
            self.root = GlobalOctreeNode::Branch {
                bounds: new_root_bounds,
                children: Box::new([
                    old_root,
                    GlobalOctreeNode::Leaf {
                        bounds,
                        assigned_node_id: Some(node_id),
                        entity_count: 0,
                    },
                    empty_slot_node(),
                    empty_slot_node(),
                    empty_slot_node(),
                    empty_slot_node(),
                    empty_slot_node(),
                    empty_slot_node(),
                ]),
            };
            changed = true;
        }

        if changed && !was_unassigned_root {
            self.topology_epoch += 1;
        }

        true
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

    /// Returns the existing 8 direct child node IDs if `target` is already split into 8 children.
    pub fn find_split_assignments(&self, target: &BoundingBox3D) -> Option<Vec<u32>> {
        self.root.find_split_assignments(target)
    }

    /// Collects all non-empty leaf octants with an assigned owner as `(bounds, node_id, resolved_addr)`.
    pub fn collect_partitions(&self, registry: &NodeRegistry) -> Vec<(BoundingBox3D, u32, String)> {
        let mut leaves = Vec::new();
        self.root.collect_leaves(&mut leaves);
        leaves
            .into_iter()
            .filter_map(|(bounds, owner)| {
                let node_id = owner?;
                if bounds.min.0 >= bounds.max.0
                    || bounds.min.1 >= bounds.max.1
                    || bounds.min.2 >= bounds.max.2
                {
                    return None;
                }
                let addr = fallback_node_addr(
                    node_id,
                    registry.get_node(node_id).map(|n| n.listen_addr.as_str()),
                );
                Some((*bounds, node_id, addr))
            })
            .collect()
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
        if !meets_min_split_extent(&original_bounds) {
            return Vec::new();
        }

        if let Some(existing_ids) = self.root.find_split_assignments(&original_bounds) {
            let mut leaves = Vec::new();
            self.root.collect_leaves(&mut leaves);
            let mut nodes_to_sync: Vec<u32> =
                existing_ids.iter().copied().filter(|&id| id != 0).collect();
            if reporting_node_id != 0 && !nodes_to_sync.contains(&reporting_node_id) {
                nodes_to_sync.push(reporting_node_id);
            }
            for nid in nodes_to_sync {
                if let Some(reg_node) = registry.get_node_mut(nid) {
                    reg_node
                        .assigned_bounds
                        .retain(|b| !bounds_approx_eq(b, &original_bounds));
                    for &(leaf_bounds, owner) in &leaves {
                        if owner == Some(nid)
                            && !reg_node
                                .assigned_bounds
                                .iter()
                                .any(|b| bounds_approx_eq(b, leaf_bounds))
                        {
                            reg_node.assigned_bounds.push(*leaf_bounds);
                        }
                    }
                }
            }
            return existing_ids;
        }

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
            // Distribute evenly across all active nodes including reporting_node_id
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

            let effective_partition_count =
                |id: u32, newly_assigned: &std::collections::HashMap<u32, usize>| -> usize {
                    let existing = registry
                        .get_node(id)
                        .map(|n| {
                            n.assigned_bounds
                                .iter()
                                .filter(|b| {
                                    !(id == reporting_node_id
                                        && bounds_approx_eq(b, &original_bounds))
                                })
                                .count()
                        })
                        .unwrap_or(0);
                    existing + newly_assigned.get(&id).copied().unwrap_or(0)
                };

            let effective_load_score =
                |id: u32, newly_assigned: &std::collections::HashMap<u32, usize>| -> f32 {
                    let new_count = newly_assigned.get(&id).copied().unwrap_or(0) as f32;
                    let base = registry
                        .get_node(id)
                        .map(|n| {
                            let removed_orig = if id == reporting_node_id {
                                n.assigned_bounds
                                    .iter()
                                    .filter(|b| bounds_approx_eq(b, &original_bounds))
                                    .count() as f32
                            } else {
                                0.0
                            };
                            (n.load_score() - removed_orig * 10.0).max(0.0)
                        })
                        .unwrap_or(0.0);
                    base + new_count * 10.0
                };

            let mut newly_assigned: std::collections::HashMap<u32, usize> =
                std::collections::HashMap::new();

            for i in 0..8 {
                let chosen = if i == 0 {
                    // Retain reporting node on child 0 for spatial continuity
                    reporting_node_id
                } else if i == 1 {
                    other_nodes.first().copied().unwrap_or(reporting_node_id)
                } else {
                    active_nodes
                        .iter()
                        .copied()
                        .min_by(|&a, &b| {
                            effective_partition_count(a, &newly_assigned)
                                .cmp(&effective_partition_count(b, &newly_assigned))
                                .then_with(|| {
                                    effective_load_score(a, &newly_assigned)
                                        .partial_cmp(&effective_load_score(b, &newly_assigned))
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .then_with(|| a.cmp(&b))
                        })
                        .unwrap_or(reporting_node_id)
                };
                *newly_assigned.entry(chosen).or_insert(0) += 1;
                assigned_node_ids.push(chosen);
            }
        }

        // 3. Update coordinator topology tree first. If split fails (octant not found),
        // abort without corrupting registry or topology epoch.
        let assignments_arr: [(BoundingBox3D, u32); 8] =
            std::array::from_fn(|i| (sub_bounds[i], assigned_node_ids[i]));
        if !self.root.split_at(&original_bounds, &assignments_arr) {
            return Vec::new();
        }

        // 4. Update NodeRegistry assigned bounds on successful split
        if let Some(node) = registry.get_node_mut(reporting_node_id) {
            node.assigned_bounds
                .retain(|b| !bounds_approx_eq(b, &original_bounds));
        }
        for (&child, &assigned_id) in sub_bounds.iter().zip(assigned_node_ids.iter()) {
            registry.add_assigned_bounds(assigned_id, child);
        }

        self.last_split_times
            .insert(reporting_node_id, std::time::Instant::now());
        self.topology_epoch += 1;

        assigned_node_ids
    }

    /// Smooths the dividing boundary between two adjacent leaf nodes based on entity centroids.
    ///
    /// Evaluates adjacency along all three axes (X, Y, Z) and both neighbor orientations.
    /// Clamps centroids within each node's assigned bounds.
    /// If entity density ratio exceeds 2.0x, calculates an elastic displacement:
    /// delta = min(25.0, 0.1 * ((count_a - count_b)/(count_a + count_b)) * distance)
    /// Damped by hysteresis (at least 1.0m shift required).
    /// Returns the adjusted bounding boxes if shifted.
    pub fn smooth_adjacent_boundaries(
        &mut self,
        primary: NodeSmoothingContext,
        neighbor: NodeSmoothingContext,
        registry: &mut NodeRegistry,
    ) -> Option<(BoundingBox3D, BoundingBox3D)> {
        let bounds_a_list = self.get_assigned_bounds_for_node(primary.node_id);
        let bounds_b_list = self.get_assigned_bounds_for_node(neighbor.node_id);

        let na = primary.entity_count as f32;
        let nb = neighbor.entity_count as f32;
        if na + nb == 0.0 || (na < 2.0 * nb && nb < 2.0 * na) {
            return None;
        }

        for ba in &bounds_a_list {
            for bb in &bounds_b_list {
                // Check all 3 axes: 0 = X, 1 = Y, 2 = Z
                for axis in 0..3 {
                    let (ax1_match, ax2_match) = match axis {
                        0 => (
                            (ba.min.1 - bb.min.1).abs() < 0.001
                                && (ba.max.1 - bb.max.1).abs() < 0.001,
                            (ba.min.2 - bb.min.2).abs() < 0.001
                                && (ba.max.2 - bb.max.2).abs() < 0.001,
                        ),
                        1 => (
                            (ba.min.0 - bb.min.0).abs() < 0.001
                                && (ba.max.0 - bb.max.0).abs() < 0.001,
                            (ba.min.2 - bb.min.2).abs() < 0.001
                                && (ba.max.2 - bb.max.2).abs() < 0.001,
                        ),
                        _ => (
                            (ba.min.0 - bb.min.0).abs() < 0.001
                                && (ba.max.0 - bb.max.0).abs() < 0.001,
                            (ba.min.1 - bb.min.1).abs() < 0.001
                                && (ba.max.1 - bb.max.1).abs() < 0.001,
                        ),
                    };

                    if !ax1_match || !ax2_match {
                        continue;
                    }

                    let (ba_min_ax, ba_max_ax) = match axis {
                        0 => (ba.min.0, ba.max.0),
                        1 => (ba.min.1, ba.max.1),
                        _ => (ba.min.2, ba.max.2),
                    };
                    let (bb_min_ax, bb_max_ax) = match axis {
                        0 => (bb.min.0, bb.max.0),
                        1 => (bb.min.1, bb.max.1),
                        _ => (bb.min.2, bb.max.2),
                    };

                    let is_ba_lower = if (ba_max_ax - bb_min_ax).abs() < 0.001 {
                        true
                    } else if (bb_max_ax - ba_min_ax).abs() < 0.001 {
                        false
                    } else {
                        continue;
                    };

                    let (ca_ax, cb_ax) = match axis {
                        0 => (
                            primary.centroid.0.clamp(ba.min.0, ba.max.0),
                            neighbor.centroid.0.clamp(bb.min.0, bb.max.0),
                        ),
                        1 => (
                            primary.centroid.1.clamp(ba.min.1, ba.max.1),
                            neighbor.centroid.1.clamp(bb.min.1, bb.max.1),
                        ),
                        _ => (
                            primary.centroid.2.clamp(ba.min.2, ba.max.2),
                            neighbor.centroid.2.clamp(bb.min.2, bb.max.2),
                        ),
                    };

                    let (lower_min, split, upper_max, lower_load, upper_load, lower_c, upper_c) =
                        if is_ba_lower {
                            (ba_min_ax, ba_max_ax, bb_max_ax, na, nb, ca_ax, cb_ax)
                        } else {
                            (bb_min_ax, bb_max_ax, ba_max_ax, nb, na, cb_ax, ca_ax)
                        };

                    let imbalance = (lower_load - upper_load) / (lower_load + upper_load);
                    let dist = (upper_c - lower_c).abs().max(10.0);
                    let shift = (imbalance * 0.1 * dist).clamp(-25.0, 25.0);

                    if shift.abs() < 1.0 {
                        continue;
                    }

                    let new_split = split + shift;
                    if new_split <= lower_min + 10.0 || new_split >= upper_max - 10.0 {
                        continue;
                    }

                    let (new_lower_bounds, new_upper_bounds) = match axis {
                        0 => {
                            let (l_min, l_max) = if is_ba_lower {
                                (ba.min, ba.max)
                            } else {
                                (bb.min, bb.max)
                            };
                            let (u_min, u_max) = if is_ba_lower {
                                (bb.min, bb.max)
                            } else {
                                (ba.min, ba.max)
                            };
                            (
                                BoundingBox3D::new(l_min, (new_split, l_max.1, l_max.2)),
                                BoundingBox3D::new((new_split, u_min.1, u_min.2), u_max),
                            )
                        }
                        1 => {
                            let (l_min, l_max) = if is_ba_lower {
                                (ba.min, ba.max)
                            } else {
                                (bb.min, bb.max)
                            };
                            let (u_min, u_max) = if is_ba_lower {
                                (bb.min, bb.max)
                            } else {
                                (ba.min, ba.max)
                            };
                            (
                                BoundingBox3D::new(l_min, (l_max.0, new_split, l_max.2)),
                                BoundingBox3D::new((u_min.0, new_split, u_min.2), u_max),
                            )
                        }
                        _ => {
                            let (l_min, l_max) = if is_ba_lower {
                                (ba.min, ba.max)
                            } else {
                                (bb.min, bb.max)
                            };
                            let (u_min, u_max) = if is_ba_lower {
                                (bb.min, bb.max)
                            } else {
                                (ba.min, ba.max)
                            };
                            (
                                BoundingBox3D::new(l_min, (l_max.0, l_max.1, new_split)),
                                BoundingBox3D::new((u_min.0, u_min.1, new_split), u_max),
                            )
                        }
                    };

                    let (new_ba, new_bb) = if is_ba_lower {
                        (new_lower_bounds, new_upper_bounds)
                    } else {
                        (new_upper_bounds, new_lower_bounds)
                    };

                    self.root.update_leaf_bounds(ba, new_ba);
                    self.root.update_leaf_bounds(bb, new_bb);

                    registry.remove_assigned_bounds(primary.node_id, ba);
                    registry.add_assigned_bounds(primary.node_id, new_ba);
                    registry.remove_assigned_bounds(neighbor.node_id, bb);
                    registry.add_assigned_bounds(neighbor.node_id, new_bb);

                    self.topology_epoch += 1;
                    metrics::counter!("mmo_boundary_adjustments_total").increment(1);
                    return Some((new_ba, new_bb));
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
            }
        }

        // Add parent bounds to the target node
        registry.add_assigned_bounds(target_node_id, parent_bounds);

        // Collapse branch in topology
        let merged = self.root.merge_at(&parent_bounds, target_node_id);

        if merged {
            self.topology_epoch += 1;
        }

        merged
    }

    /// Scans the topology tree for leaf octants assigned to evicted/dead simulation nodes,
    /// reassigning each orphaned partition to active surviving nodes balancing cluster load.
    ///
    /// Resolves departed node addresses from `dead_node_addrs` and recipient node addresses
    /// from `registry`, falling back to standard cluster DNS if empty.
    /// Returns a list of `PartitionRecoveryEvent`s describing the reassignments.
    /// If no active nodes remain, the partitions are set to unassigned (`None`).
    pub fn recover_orphaned_partitions(
        &mut self,
        dead_node_ids: &[u32],
        dead_node_addrs: &std::collections::HashMap<u32, String>,
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

            if let Some(new_node_id) = target_node_id {
                registry.add_assigned_bounds(new_node_id, bounds);

                let prev_addr = fallback_node_addr(
                    prev_node_id,
                    dead_node_addrs.get(&prev_node_id).map(|s| s.as_str()),
                );
                let new_addr = fallback_node_addr(
                    new_node_id,
                    registry
                        .get_node(new_node_id)
                        .map(|n| n.listen_addr.as_str()),
                );

                events.push(PartitionRecoveryEvent {
                    bounds,
                    previous_node_id: prev_node_id,
                    new_node_id,
                    previous_node_address: prev_addr,
                    new_node_address: new_addr,
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
        let mut dead_addrs = std::collections::HashMap::new();
        dead_addrs.insert(node1, "127.0.0.1:50051".to_string());
        let events = coord.recover_orphaned_partitions(&[node1], &dead_addrs, &mut registry);
        assert!(!events.is_empty());
        assert_eq!(coord.topology_epoch(), 3);

        for event in &events {
            assert_eq!(event.previous_node_id, node1);
            assert_eq!(event.new_node_id, node2);
            assert_eq!(event.previous_node_address, "127.0.0.1:50051");
            assert_eq!(event.new_node_address, "127.0.0.1:50052");
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

        let events = coord.recover_orphaned_partitions(
            &[node1],
            &std::collections::HashMap::new(),
            &mut registry,
        );
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
        let dummy_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (-50.0, -50.0, -50.0));
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
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
            ]),
        };

        let initial_epoch = coord.topology_epoch();

        // Node A has 300 entities (hotspot), Node B has 50 entities (ratio 6:1 > 2.0x)
        // With extreme spoofed coordinate (99999.0), it should be clamped inside bound_a
        let smoothed = coord.smooth_adjacent_boundaries(
            NodeSmoothingContext::new(node_a, (99999.0, 50.0, 50.0), 300),
            NodeSmoothingContext::new(node_b, (150.0, 50.0, 50.0), 50),
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

        // Point inside the newly shifted strip (e.g. 101.0, 25.0, 25.0) routes to expanded node A
        assert_eq!(coord.find_node_for_point((101.0, 25.0, 25.0)), Some(node_a));
    }

    #[test]
    fn test_smooth_adjacent_boundaries_y_axis_and_routing() {
        let root_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 200.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(root_bounds);
        let mut registry = NodeRegistry::new();

        let bound_a = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let bound_b = BoundingBox3D::new((0.0, 100.0, 0.0), (100.0, 200.0, 100.0));

        let node_a = registry.register_node("127.0.0.1:50051".to_string(), vec![bound_a]);
        let node_b = registry.register_node("127.0.0.1:50052".to_string(), vec![bound_b]);

        let dummy_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (-50.0, -50.0, -50.0));
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
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
            ]),
        };

        let smoothed = coord.smooth_adjacent_boundaries(
            NodeSmoothingContext::new(node_a, (50.0, 50.0, 50.0), 300),
            NodeSmoothingContext::new(node_b, (50.0, 150.0, 50.0), 50),
            &mut registry,
        );

        assert!(smoothed.is_some());
        let (new_ba, new_bb) = smoothed.unwrap();
        assert!(new_ba.max.1 > 100.0);
        assert_eq!(new_ba.max.1, new_bb.min.1);
        assert_eq!(coord.find_node_for_point((25.0, 101.0, 25.0)), Some(node_a));
    }

    #[test]
    fn test_smooth_adjacent_boundaries_z_axis_and_reverse_order() {
        let root_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 200.0));
        let mut coord = GlobalOctreeCoordinator::new(root_bounds);
        let mut registry = NodeRegistry::new();

        let bound_lower = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let bound_upper = BoundingBox3D::new((0.0, 0.0, 100.0), (100.0, 100.0, 200.0));

        let node_lower = registry.register_node("127.0.0.1:50051".to_string(), vec![bound_lower]);
        let node_upper = registry.register_node("127.0.0.1:50052".to_string(), vec![bound_upper]);

        let dummy_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (-50.0, -50.0, -50.0));
        coord.root = GlobalOctreeNode::Branch {
            bounds: root_bounds,
            children: Box::new([
                GlobalOctreeNode::Leaf {
                    bounds: bound_lower,
                    assigned_node_id: Some(node_lower),
                    entity_count: 50,
                },
                GlobalOctreeNode::Leaf {
                    bounds: bound_upper,
                    assigned_node_id: Some(node_upper),
                    entity_count: 300,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
                GlobalOctreeNode::Leaf {
                    bounds: dummy_bounds,
                    assigned_node_id: None,
                    entity_count: 0,
                },
            ]),
        };

        // Primary is node_upper (reverse order: primary has higher coordinate along Z axis)
        let smoothed = coord.smooth_adjacent_boundaries(
            NodeSmoothingContext::new(node_upper, (50.0, 50.0, 150.0), 300),
            NodeSmoothingContext::new(node_lower, (50.0, 50.0, 50.0), 50),
            &mut registry,
        );

        assert!(smoothed.is_some());
        let (new_upper, new_lower) = smoothed.unwrap();
        // Shift should move split to the left (< 100.0) expanding upper partition
        assert!(new_upper.min.2 < 100.0);
        assert_eq!(new_lower.max.2, new_upper.min.2);
        assert_eq!(
            coord.find_node_for_point((25.0, 25.0, 99.0)),
            Some(node_upper)
        );
    }

    #[test]
    fn test_assign_or_map_node_bounds_multi_node_octants() {
        let root_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(root_bounds);
        let octants = subdivide_bounds(&root_bounds);

        assert!(coord.assign_or_map_node_bounds(1, octants[0]));
        assert!(coord.assign_or_map_node_bounds(2, octants[1]));
        assert!(coord.assign_or_map_node_bounds(3, octants[7]));

        assert_eq!(coord.find_node_for_point((25.0, 25.0, 25.0)), Some(1));
        assert_eq!(coord.find_node_for_point((75.0, 25.0, 25.0)), Some(2));
        assert_eq!(coord.find_node_for_point((75.0, 75.0, 75.0)), Some(3));
        assert_eq!(coord.find_node_for_point((25.0, 75.0, 75.0)), None);

        assert_eq!(coord.get_assigned_bounds_for_node(1), vec![octants[0]]);
        assert_eq!(coord.get_assigned_bounds_for_node(2), vec![octants[1]]);
        assert_eq!(coord.get_assigned_bounds_for_node(3), vec![octants[7]]);
    }

    #[test]
    fn test_assign_or_map_node_bounds_non_octant_default_world() {
        let mut coord = GlobalOctreeCoordinator::default();
        let b1 = BoundingBox3D::new((-100.0, -100.0, -100.0), (0.0, 100.0, 100.0));
        let b2 = BoundingBox3D::new((0.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        assert!(coord.assign_or_map_node_bounds(1, b1));
        assert!(coord.assign_or_map_node_bounds(2, b2));

        assert_eq!(coord.find_node_for_point((-50.0, 0.0, 0.0)), Some(1));
        assert_eq!(coord.find_node_for_point((50.0, 0.0, 0.0)), Some(2));
        assert_eq!(coord.find_node_for_point((500.0, 500.0, 500.0)), None);
    }

    #[test]
    fn test_recover_orphaned_partitions_distributes_across_surviving_nodes() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let octants = subdivide_bounds(&bounds);
        // Node 1 owns 6 partitions; Nodes 2 and 3 each own 1 partition
        let node1 = registry.register_node("127.0.0.1:50051".to_string(), octants[0..6].to_vec());
        let node2 = registry.register_node("127.0.0.1:50052".to_string(), vec![octants[6]]);
        let node3 = registry.register_node("127.0.0.1:50053".to_string(), vec![octants[7]]);

        for &b in &octants[0..6] {
            assert!(coord.assign_or_map_node_bounds(node1, b));
        }
        assert!(coord.assign_or_map_node_bounds(node2, octants[6]));
        assert!(coord.assign_or_map_node_bounds(node3, octants[7]));

        let _ = registry.deregister_node(node1);
        let mut dead_addrs = std::collections::HashMap::new();
        dead_addrs.insert(node1, "127.0.0.1:50051".to_string());

        let events = coord.recover_orphaned_partitions(&[node1], &dead_addrs, &mut registry);
        assert_eq!(events.len(), 6);

        let n2_count = events.iter().filter(|e| e.new_node_id == node2).count();
        let n3_count = events.iter().filter(|e| e.new_node_id == node3).count();
        assert_eq!(
            n2_count, 3,
            "node2 should receive 3 of 6 orphaned partitions"
        );
        assert_eq!(
            n3_count, 3,
            "node3 should receive 3 of 6 orphaned partitions"
        );
        assert_eq!(registry.get_node(node2).unwrap().assigned_bounds.len(), 4);
        assert_eq!(registry.get_node(node3).unwrap().assigned_bounds.len(), 4);
    }

    #[test]
    fn test_fallback_node_addr_normalization() {
        assert_eq!(
            fallback_node_addr(1, Some("0.0.0.0:50052")),
            "mmo-node-0.mmo-node:50052"
        );
        assert_eq!(
            fallback_node_addr(2, Some("0.0.0.0:50053")),
            "mmo-node-1.mmo-node:50053"
        );
        assert_eq!(
            fallback_node_addr(1, Some("[::]:50052")),
            "mmo-node-0.mmo-node:50052"
        );
        assert_eq!(
            fallback_node_addr(3, Some("[::]:50060")),
            "mmo-node-2.mmo-node:50060"
        );
        assert_eq!(
            fallback_node_addr(1, Some("::")),
            "mmo-node-0.mmo-node:50052"
        );
        assert_eq!(
            fallback_node_addr(1, Some("[::]")),
            "mmo-node-0.mmo-node:50052"
        );
        assert_eq!(
            fallback_node_addr(1, Some("0.0.0.0")),
            "mmo-node-0.mmo-node:50052"
        );
        assert_eq!(
            fallback_node_addr(1, Some("http://0.0.0.0:50054")),
            "mmo-node-0.mmo-node:50054"
        );
        assert_eq!(
            fallback_node_addr(2, Some("https://[::]:50055")),
            "mmo-node-1.mmo-node:50055"
        );
        assert_eq!(fallback_node_addr(1, Some("")), "mmo-node-0.mmo-node:50052");
        assert_eq!(
            fallback_node_addr(2, Some("   ")),
            "mmo-node-1.mmo-node:50052"
        );
        assert_eq!(fallback_node_addr(1, None), "mmo-node-0.mmo-node:50052");
        assert_eq!(
            fallback_node_addr(1, Some("127.0.0.1:50051")),
            "127.0.0.1:50051"
        );
        assert_eq!(
            fallback_node_addr(2, Some("10.0.1.5:50052")),
            "10.0.1.5:50052"
        );
        assert_eq!(
            fallback_node_addr(1, Some("mmo-node-0.mmo-node:50052")),
            "mmo-node-0.mmo-node:50052"
        );
    }

    #[test]
    fn test_standby_node_does_not_steal_active_root_partition() {
        let world_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(world_bounds);
        let mut registry = NodeRegistry::new();

        let node1 =
            registry.register_node_with_id(1, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(
            node1,
            world_bounds,
            Some(&mut registry)
        ));
        assert_eq!(coord.topology_epoch(), 1);
        assert_eq!(coord.find_node_for_point((0.0, 0.0, 0.0)), Some(node1));

        // Standby node 2 registers with the same default world bounds while node 1 is active
        let node2 = registry.register_node_with_id(2, "[::]:50052".to_string(), vec![world_bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(
            node2,
            world_bounds,
            Some(&mut registry)
        ));

        // Node 1 must retain ownership, epoch must not bump, and node 2 must have empty bounds
        assert_eq!(coord.find_node_for_point((0.0, 0.0, 0.0)), Some(node1));
        assert_eq!(coord.topology_epoch(), 1);
        assert!(registry.get_node(node2).unwrap().assigned_bounds.is_empty());
        assert_eq!(
            registry.get_node(node1).unwrap().assigned_bounds,
            vec![world_bounds]
        );

        // Also test when coordinator uses DEFAULT_WORLD_BOUNDS and nodes register (-100..100)^3
        let mut default_coord = GlobalOctreeCoordinator::default();
        let mut default_reg = NodeRegistry::new();
        let n1 =
            default_reg.register_node_with_id(1, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(default_coord.assign_or_map_node_bounds_with_registry(
            n1,
            world_bounds,
            Some(&mut default_reg)
        ));
        let epoch_before = default_coord.topology_epoch();
        let n2 =
            default_reg.register_node_with_id(2, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(default_coord.assign_or_map_node_bounds_with_registry(
            n2,
            world_bounds,
            Some(&mut default_reg)
        ));
        assert_eq!(default_coord.find_node_for_point((0.0, 0.0, 0.0)), Some(n1));
        assert_eq!(default_coord.topology_epoch(), epoch_before);
        assert!(default_reg.get_node(n2).unwrap().assigned_bounds.is_empty());

        // If node 1 is no longer active in registry, node 2 CAN take over on re-registration
        default_reg.deregister_node(n1);
        let n2_takeover =
            default_reg.register_node_with_id(2, "10.0.0.2:50052".to_string(), vec![world_bounds]);
        assert!(default_coord.assign_or_map_node_bounds_with_registry(
            n2_takeover,
            world_bounds,
            Some(&mut default_reg)
        ));
        assert_eq!(
            default_coord.find_node_for_point((0.0, 0.0, 0.0)),
            Some(n2_takeover)
        );
    }

    #[test]
    fn test_repeated_register_on_already_split_bounds_preserves_branch() {
        let world_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(world_bounds);
        let mut registry = NodeRegistry::new();

        let node1 =
            registry.register_node_with_id(1, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(
            node1,
            world_bounds,
            Some(&mut registry)
        ));
        let node2 =
            registry.register_node_with_id(2, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(
            node2,
            world_bounds,
            Some(&mut registry)
        ));

        let assigned = coord.handle_split(node1, world_bounds, vec![], &mut registry);
        assert_eq!(assigned, vec![1, 2, 1, 2, 1, 2, 1, 2]);
        let epoch_after_split = coord.topology_epoch();
        assert_eq!(epoch_after_split, 2);

        // Node 1 restarts and re-registers with world_bounds: branch must NOT flatten or duplicate
        registry.register_node_with_id(1, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(
            1,
            world_bounds,
            Some(&mut registry)
        ));
        assert!(matches!(coord.root, GlobalOctreeNode::Branch { .. }));
        assert_eq!(coord.leaf_octants().len(), 8);
        assert_eq!(coord.topology_epoch(), epoch_after_split);

        let reg_n1_bounds = &registry.get_node(1).unwrap().assigned_bounds;
        assert_eq!(reg_n1_bounds.len(), 4);
        assert!(!reg_n1_bounds.contains(&world_bounds));

        // New standby node 3 registers with world_bounds: gets empty assigned_bounds
        let node3 =
            registry.register_node_with_id(3, "0.0.0.0:50052".to_string(), vec![world_bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(
            node3,
            world_bounds,
            Some(&mut registry)
        ));
        assert_eq!(coord.leaf_octants().len(), 8);
        assert_eq!(coord.topology_epoch(), epoch_after_split);
        assert!(registry.get_node(node3).unwrap().assigned_bounds.is_empty());
    }

    #[test]
    fn test_should_split_node_rejects_standby_and_sub_minimum_bounds() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node_with_id(1, "127.0.0.1:50051".to_string(), vec![bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(node1, bounds, Some(&mut registry)));
        let node2 = registry.register_node_with_id(2, "127.0.0.1:50052".to_string(), vec![bounds]);
        assert!(coord.assign_or_map_node_bounds_with_registry(node2, bounds, Some(&mut registry)));

        // Standby node 2 owns no leaves -> should_split_node must return false
        assert!(!coord.should_split_node(node2, 1000, 100.0));
        // Active node 1 owns 100^3 >= MIN_SPLIT_EXTENT (25.0) -> true
        assert!(coord.should_split_node(node1, 100, 10.0));

        // 25^3 meets MIN_SPLIT_EXTENT and can split into 12.5^3
        let min_splittable = BoundingBox3D::new((0.0, 0.0, 0.0), (25.0, 25.0, 25.0));
        let mut min_coord = GlobalOctreeCoordinator::new(min_splittable);
        let mut min_reg = NodeRegistry::new();
        let mn =
            min_reg.register_node_with_id(1, "127.0.0.1:50051".to_string(), vec![min_splittable]);
        min_coord.assign_initial_node(mn);
        assert!(min_coord.should_split_node(mn, 100, 10.0));
        let sub_assigned = min_coord.handle_split(mn, min_splittable, vec![], &mut min_reg);
        assert_eq!(sub_assigned.len(), 8);

        // After splitting 25^3 -> 12.5^3, all owned leaves are < MIN_SPLIT_EXTENT
        min_coord.last_split_times.clear();
        assert!(!min_coord.should_split_node(mn, 1000, 100.0));
        let sub_leaf = BoundingBox3D::new((0.0, 0.0, 0.0), (12.5, 12.5, 12.5));
        assert!(min_coord
            .handle_split(mn, sub_leaf, vec![], &mut min_reg)
            .is_empty());

        // Asymmetric box with one axis < MIN_SPLIT_EXTENT (24.0 < 25.0) must also be rejected
        let thin_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 24.0));
        let mut thin_coord = GlobalOctreeCoordinator::new(thin_bounds);
        let mut thin_reg = NodeRegistry::new();
        let tn =
            thin_reg.register_node_with_id(1, "127.0.0.1:50051".to_string(), vec![thin_bounds]);
        thin_coord.assign_initial_node(tn);
        assert!(!thin_coord.should_split_node(tn, 1000, 100.0));
        assert!(thin_coord
            .handle_split(tn, thin_bounds, vec![], &mut thin_reg)
            .is_empty());
    }

    #[test]
    fn test_handle_split_balanced_and_idempotent() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut coord = GlobalOctreeCoordinator::new(bounds);
        let mut registry = NodeRegistry::new();

        let node1 = registry.register_node_with_id(1, "0.0.0.0:50052".to_string(), vec![bounds]);
        let node2 = registry.register_node_with_id(2, "0.0.0.0:50052".to_string(), vec![]);
        coord.assign_initial_node(node1);

        let first = coord.handle_split(node1, bounds, vec![], &mut registry);
        assert_eq!(first, vec![1, 2, 1, 2, 1, 2, 1, 2]);
        assert_eq!(coord.topology_epoch(), 2);
        assert_eq!(registry.get_node(node1).unwrap().assigned_bounds.len(), 4);
        assert_eq!(registry.get_node(node2).unwrap().assigned_bounds.len(), 4);

        // Second split call on already-split branch must be idempotent and not bump epoch
        let second = coord.handle_split(node1, bounds, vec![], &mut registry);
        assert_eq!(second, vec![1, 2, 1, 2, 1, 2, 1, 2]);
        assert_eq!(coord.topology_epoch(), 2);
        assert_eq!(registry.get_node(node1).unwrap().assigned_bounds.len(), 4);
        assert_eq!(registry.get_node(node2).unwrap().assigned_bounds.len(), 4);

        // Even if registry lost sync, idempotent handle_split restores leaf bounds without bumping epoch
        registry.get_node_mut(node1).unwrap().assigned_bounds = vec![bounds];
        let third = coord.handle_split(node1, bounds, vec![], &mut registry);
        assert_eq!(third, vec![1, 2, 1, 2, 1, 2, 1, 2]);
        assert_eq!(coord.topology_epoch(), 2);
        assert_eq!(registry.get_node(node1).unwrap().assigned_bounds.len(), 4);
        assert!(!registry
            .get_node(node1)
            .unwrap()
            .assigned_bounds
            .contains(&bounds));
    }
}
