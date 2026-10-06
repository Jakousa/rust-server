//! Authoritative MMO Simulation Node.
//!
//! Provides the fixed 20 Hz simulation game loop, entity store integration,
//! spatial octree partitioning, synchronous hook dispatches, and kinematic movement validation.

pub mod ability_dispatcher;
pub mod aoi;
pub mod instance;
pub mod kinematics;
pub mod network;
pub mod sector;
pub mod sim;
pub mod snapshotter;
pub mod world_snapshotter;

pub use ability_dispatcher::{
    cast_boundary_sphere_aoe, cast_cross_partition_ability, BoundaryAoeOutcome, CastOutcome,
    CrossNodeDispatcher, DEFAULT_DISPATCH_TIMEOUT,
};
pub use aoi::{AoiBroadcastJob, AoiManager, CachedEntityState, ObserverConfig, ObserverState};
pub use instance::{InstanceDomainManager, LocalInstanceDomain, NodeInstanceError};
pub use kinematics::{
    EntityKinematicState, KinematicValidator, KinematicViolation, KinematicViolationReason,
};

pub use network::{
    execute_batch_entity_migration, execute_entity_migration, execute_remote_ability,
    extract_initiator, make_remote_command, BatchMigrationReport, BroadcastJob, EntityVelocity,
    GhostBroadcaster, GhostRecord, GhostStore, MigrationError, NetworkError, NodeServiceImpl,
    RemoteCommandErrorCode,
};
pub use sector::{PlacementError, SectorManager, MAX_INTERACTION_DISTANCE};
pub use sim::{AbilityCompensation, ContainerTransferNotification, TickSimulation};
pub use snapshotter::WriteBehindSnapshotter;
pub use world_snapshotter::WorldObjectWriteBehindSnapshotter;
