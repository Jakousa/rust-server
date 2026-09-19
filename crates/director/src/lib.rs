//! MMO Director control plane service.
//!
//! Maintains cluster node registry, monitors node telemetry and heartbeats,
//! and commands dynamic Octree subdivision and merge topologies.

pub mod coordinator;
pub mod registry;
pub mod service;

pub use coordinator::{
    subdivide_bounds, GlobalOctreeCoordinator, GlobalOctreeNode, DEFAULT_MERGE_THRESHOLD,
    DEFAULT_SPLIT_THRESHOLD, DEFAULT_WORLD_BOUNDS,
};
pub use registry::{NodeRegistry, RegisteredNode};
pub use service::{
    bounding_box_3d_to_proto, proto_bbox_to_bounding_box_3d, DirectorServiceImpl,
};
