//! MMO Director control plane service.
//!
//! Maintains cluster node registry, monitors node telemetry and heartbeats,
//! and commands dynamic Octree subdivision and merge topologies.

pub mod coordinator;
pub mod federation;
pub mod instance;
pub mod registry;
pub mod service;

pub use coordinator::{
    fallback_node_addr, subdivide_bounds, GlobalOctreeCoordinator, GlobalOctreeNode,
    NodeSmoothingContext, PartitionRecoveryEvent, DEFAULT_MERGE_THRESHOLD, DEFAULT_SPLIT_THRESHOLD,
    DEFAULT_WORLD_BOUNDS, MIN_SPLIT_EXTENT,
};
pub use federation::{FederationError, RealmFederationAuthority, RealmTransferClaim};
pub use instance::{validate_instance_bounds, InstanceDescriptor, InstanceError, InstanceRegistry};
pub use registry::{
    GatewayDescriptor, GatewayRegistry, GatewayStatus, NodeRegistry, RegisteredNode,
};
pub use service::{bounding_box_3d_to_proto, proto_bbox_to_bounding_box_3d, DirectorServiceImpl};
