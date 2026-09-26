//! Authoritative MMO Simulation Node.
//!
//! Provides the fixed 20 Hz simulation game loop, entity store integration,
//! spatial octree partitioning, synchronous hook dispatches, and kinematic movement validation.

pub mod aoi;
pub mod kinematics;
pub mod network;
pub mod sim;
pub mod snapshotter;

pub use aoi::{AoiBroadcastJob, AoiManager, CachedEntityState, ObserverConfig, ObserverState};
pub use kinematics::{
    EntityKinematicState, KinematicValidator, KinematicViolation, KinematicViolationReason,
};
pub use network::{
    extract_initiator, make_remote_command, BroadcastJob, EntityVelocity, GhostBroadcaster,
    GhostRecord, GhostStore, NetworkError, NodeServiceImpl, RemoteCommandErrorCode,
};
pub use sim::TickSimulation;
pub use snapshotter::WriteBehindSnapshotter;
