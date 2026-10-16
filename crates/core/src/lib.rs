pub mod action;
pub mod attribute;
pub mod collision;
pub mod container;
pub mod entity;
pub mod guid;
pub mod hook;
pub mod octree;
pub mod plugin;
pub mod property;
pub mod script;
pub mod social;
pub mod spatial;
pub mod telemetry;
pub mod world;

pub use collision::{
    AabbCollisionWorld, CollisionWorld, EmptyCollisionWorld, RayHit, SpatialQueryFilter, SweepHit,
};

pub use action::{
    ActionCompletedEvent, ActionInterruptFlags, ActionInterruptedEvent, ActionProgressEvent,
    ActionQueue, ActionStartedEvent, ActionTickSystem, InterruptReason, QueuePolicy, TimedAction,
};
pub use attribute::{
    AttributeSet, AttributeTickSystem, AttributeValue, AuraExpiredEvent, ModifierType,
    TimedModifier,
};
pub use container::{
    ContainerAccessPolicy, ContainerError, ContainerId, ContainerOwner, ContainerSlot,
};
pub use entity::{ComponentStore, Entity, EntityKey, EntityStore};
pub use guid::{EntityType, Guid};
pub use hook::{Event, EventContext, HookDispatcher, HookHandler};
pub use octree::{BoundingBox3D, Octree};
pub use plugin::{
    CodecError, ComponentCodec, ComponentCodecRegistry, DomainEvent, EntityDespawnEvent,
    EventQueue, GameplayPlugin, ModularPacketHandler, ModularPacketRouter, PacketHandlerError,
    PluginContext, PluginManager, TickContext, TickSystem, TypedComponentCodec,
};
pub use property::{DynamicProperties, PropertyValue};
pub use script::{FuelMeteredExecutor, ScriptError, ScriptFuelConfig, ScriptValue};
pub use social::{SocialChannel, SocialEvent};
pub use spatial::{
    query_sphere_targets, ray_intersects_aabb, validate_cone_target, validate_point_range,
    SpatialQueryError, Vec3,
};
pub use telemetry::{TelemetryError, TelemetryServer};
pub use world::{
    world_object_flags, ObjectHealth, SectorCoord3D, SectorSpatialIndex, Transform3D, WorldObject,
    SECTOR_SIZE,
};

/// Awaits an asynchronous termination signal (Ctrl+C / SIGINT or Unix SIGTERM).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                eprintln!("[Core] Failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            eprintln!("[Core] Received SIGINT (Ctrl+C), starting graceful shutdown");
        },
        _ = terminate => {
            eprintln!("[Core] Received SIGTERM, starting graceful shutdown");
        },
    }
}
