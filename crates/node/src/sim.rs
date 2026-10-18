//! Simulation node tick loop and authoritative movement processing.
//!
//! Drives the fixed-frequency (20 Hz / 50ms) simulation game loop, entity iteration,
//! hook dispatches, and authoritative client movement validation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use mmo_core::{
    BoundingBox3D, ComponentCodecRegistry, EntityDespawnEvent, EntityStore, Event, EventQueue,
    GameplayPlugin, Guid, HookDispatcher, ModularPacketRouter, Octree, PluginManager, TickContext,
    TickSystem,
};

use crate::aoi::{AoiBroadcastJob, AoiManager, ObserverConfig, ObserverState};
use crate::kinematics::{EntityKinematicState, KinematicValidator};
use crate::network::{GhostBroadcaster, GhostStore, NodeServiceImpl};

/// Authoritative simulation node driving world ticks, spatial partitioning,
/// dynamic entity stores, and movement validation.
pub struct TickSimulation {
    pub node_id: u16,
    pub entity_store: EntityStore,
    pub octree: Octree,
    pub hooks: HookDispatcher,
    pub kinematics: KinematicValidator,
    pub collision_world: std::sync::Arc<std::sync::RwLock<dyn mmo_core::collision::CollisionWorld>>,
    pub tick_counter: u64,
    pub ghost_store: GhostStore,
    pub codecs: ComponentCodecRegistry,
    pub event_queue: EventQueue,
    pub packets: ModularPacketRouter,
    pub tick_systems: Vec<Box<dyn TickSystem>>,
    pub plugin_manager: PluginManager,
    pub aoi: AoiManager,
    pub sectors: crate::sector::SectorManager,
    pub instances: crate::instance::InstanceDomainManager,
    pub next_world_object_seq: u32,
    pub pending_broadcast_jobs: Vec<AoiBroadcastJob>,
    pub kinematic_states: HashMap<Guid, EntityKinematicState>,
    pub migrating_leases: HashMap<Guid, Instant>,
    pub last_migration_ticks: HashMap<Guid, u64>,
    pub session_epochs: HashMap<Guid, u64>,
    pub current_time: Instant,
    pub last_tick_duration: Duration,
    pub ack_sender: tokio::sync::mpsc::UnboundedSender<mmo_schema::client::ClientAckPacket>,
    pub ack_receiver: tokio::sync::mpsc::UnboundedReceiver<mmo_schema::client::ClientAckPacket>,
    pub input_sender: tokio::sync::mpsc::UnboundedSender<QueuedClientInput>,
    pub input_receiver: tokio::sync::mpsc::UnboundedReceiver<QueuedClientInput>,
    pub ability_compensation_sender: tokio::sync::mpsc::UnboundedSender<AbilityCompensation>,
    pub ability_compensation_receiver: tokio::sync::mpsc::UnboundedReceiver<AbilityCompensation>,
    pub container_transfer_sender:
        tokio::sync::mpsc::UnboundedSender<ContainerTransferNotification>,
    pub container_transfer_receiver:
        tokio::sync::mpsc::UnboundedReceiver<ContainerTransferNotification>,
    pub snapshot_sender: Option<tokio::sync::mpsc::Sender<crate::snapshotter::SnapshotPayload>>,
    pub snapshot_interval_ticks: u64,
    pub history: crate::history::SpatialHistoryBuffer,
    pub script_reloader: Option<Arc<dyn mmo_core::script::ScriptReloadable>>,
}

/// Ingress packet queued from a gateway client session, drained at tick boundaries.
#[derive(Debug, Clone)]
pub struct QueuedClientInput {
    pub session_id: u64,
    pub entity_guid: u64,
    pub session_epoch: u64,
    pub packet: mmo_schema::client::ClientPacket,
}

/// Compensation event for refunding caster resources and resetting cooldowns if a cross-node ability fails.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilityCompensation {
    pub caster_guid: Guid,
    pub ability_id: u32,
    pub resource_name: Option<String>,
    pub resource_amount: f32,
}

/// Notification event dispatched when a container transfer is finalized or aborted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerTransferNotification {
    pub tx_id: String,
    pub src_container_id: u64,
    pub dst_container_id: u64,
    pub item_instance_id: String,
    pub success: bool,
    pub error_message: Option<String>,
}

impl TickSimulation {
    /// Simulation tick rate: 20 Hz (20 ticks per second).
    pub const TICK_RATE_HZ: u32 = 20;
    /// Tick duration interval: 50 milliseconds.
    pub const TICK_INTERVAL: Duration = Duration::from_millis(50);
    /// Tick delta time in seconds: 0.05s.
    pub const TICK_DT_SEC: f32 = 0.05;

    /// Creates a new `TickSimulation` instance with default kinematic validation.
    pub fn new(node_id: u16, bounds: BoundingBox3D) -> Self {
        Self::with_kinematics(node_id, bounds, KinematicValidator::default())
    }

    /// Creates a new `TickSimulation` instance with a custom `KinematicValidator`.
    pub fn with_kinematics(
        node_id: u16,
        bounds: BoundingBox3D,
        kinematics: KinematicValidator,
    ) -> Self {
        let mut codecs = ComponentCodecRegistry::new();
        codecs.register::<mmo_core::ActionQueue>("action_queue");
        codecs.register::<mmo_core::AttributeSet>("attribute_set");
        codecs.register::<mmo_core::action::CooldownTracker>("cooldown_tracker");

        let tick_systems: Vec<Box<dyn mmo_core::TickSystem>> = vec![
            Box::new(mmo_core::ActionTickSystem),
            Box::new(mmo_core::AttributeTickSystem),
            Box::new(mmo_core::action::ActionCooldownSystem),
        ];

        let (ack_sender, ack_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (input_sender, input_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (ability_compensation_sender, ability_compensation_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let (container_transfer_sender, container_transfer_receiver) =
            tokio::sync::mpsc::unbounded_channel();

        Self {
            node_id,
            entity_store: EntityStore::new(),
            octree: Octree::new(bounds),
            hooks: HookDispatcher::new(),
            kinematics,
            collision_world: std::sync::Arc::new(std::sync::RwLock::new(
                mmo_core::collision::AabbCollisionWorld::new(),
            )),
            tick_counter: 0,
            ghost_store: GhostStore::new(),
            codecs,
            event_queue: EventQueue::new(),
            packets: ModularPacketRouter::new(),
            tick_systems,
            plugin_manager: PluginManager::new(),
            aoi: AoiManager::new(),
            sectors: crate::sector::SectorManager::new(),
            instances: crate::instance::InstanceDomainManager::new(),
            next_world_object_seq: 1,
            pending_broadcast_jobs: Vec::new(),
            kinematic_states: HashMap::new(),
            migrating_leases: HashMap::new(),
            last_migration_ticks: HashMap::new(),
            session_epochs: HashMap::new(),
            current_time: Instant::now(),
            last_tick_duration: Duration::from_millis(0),
            ack_sender,
            ack_receiver,
            input_sender,
            input_receiver,
            ability_compensation_sender,
            ability_compensation_receiver,
            container_transfer_sender,
            container_transfer_receiver,
            snapshot_sender: None,
            snapshot_interval_ticks: 20,
            history: crate::history::SpatialHistoryBuffer::default(),
            script_reloader: None,
        }
    }

    /// Returns a cloned sender for pushing client input packets into the simulation ingress queue.
    pub fn input_sender(&self) -> tokio::sync::mpsc::UnboundedSender<QueuedClientInput> {
        self.input_sender.clone()
    }

    /// Enqueues an inbound client packet for processing on the next simulation tick.
    #[allow(clippy::result_large_err)]
    pub fn queue_client_input(
        &self,
        input: QueuedClientInput,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<QueuedClientInput>> {
        self.input_sender.send(input)
    }

    /// Drains and processes all queued client input packets.
    pub fn drain_client_inputs(&mut self) {
        while let Ok(input) = self.input_receiver.try_recv() {
            let guid = Guid::from_raw(input.entity_guid);
            if !self.is_valid_upstream_epoch(guid, input.session_epoch) {
                metrics::counter!("mmo_node_stale_epoch_packets_dropped_total").increment(1);
                continue;
            }
            if !self.entity_store.contains_guid(guid) {
                continue;
            }
            if self.is_migrating(guid) {
                continue;
            }

            match input.packet {
                mmo_schema::client::ClientPacket::ClientAck(ack) => {
                    self.aoi.process_ack(
                        ack.session_id,
                        ack.observed_tick,
                        ack.ack_mask,
                        self.tick_counter,
                    );
                }
                mmo_schema::client::ClientPacket::Movement(m) => {
                    if input.entity_guid != 0 && m.entity_id != input.entity_guid {
                        tracing::warn!("Spoofed movement entity_id rejected");
                        metrics::counter!("mmo_node_spoofed_packets_dropped_total").increment(1);
                        continue;
                    }
                    match self.process_client_move_with_seq(
                        guid,
                        (m.position.x, m.position.y, m.position.z),
                        m.sequence,
                    ) {
                        Ok(_) => {
                            metrics::counter!("mmo_node_client_movements_processed_total")
                                .increment(1);
                        }
                        Err(snap_back) => {
                            let correction = mmo_schema::client::MovementCorrectionPacket {
                                entity_id: guid.raw(),
                                corrected_position: mmo_schema::client::Vec3::new(
                                    snap_back.0,
                                    snap_back.1,
                                    snap_back.2,
                                ),
                                corrected_velocity: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                                sequence: m.sequence,
                                reason:
                                    mmo_schema::client::MovementCorrectionReason::KinematicViolation
                                        .into(),
                            };
                            self.pending_broadcast_jobs
                                .push(crate::aoi::AoiBroadcastJob {
                                    session_id: input.session_id,
                                    player_guid: guid,
                                    packets: vec![
                                        mmo_schema::client::ClientPacket::MovementCorrection(
                                            correction,
                                        ),
                                    ],
                                });
                            metrics::counter!("mmo_node_movement_corrections_total").increment(1);
                        }
                    }
                }
                mmo_schema::client::ClientPacket::PlaceObject(po) => {
                    let _ = self.handle_place_object(guid, &po);
                }
                mmo_schema::client::ClientPacket::InteractObject(io) => {
                    let _ = self.handle_interact_object(guid, &io);
                }
                mmo_schema::client::ClientPacket::WorldObjectResync(resync) => {
                    if let Some(spawn) = self.handle_world_object_resync(guid, &resync) {
                        self.pending_broadcast_jobs
                            .push(crate::aoi::AoiBroadcastJob {
                                session_id: input.session_id,
                                player_guid: guid,
                                packets: vec![mmo_schema::client::ClientPacket::WorldObjectSpawn(
                                    spawn,
                                )],
                            });
                    }
                }
                mmo_schema::client::ClientPacket::Interaction(interact) => {
                    let (module_id, sub_opcode) = if interact.action_id > 0xFFFF {
                        (
                            (interact.action_id >> 16) as u16,
                            (interact.action_id & 0xFFFF) as u16,
                        )
                    } else {
                        (
                            interact.action_id as u16,
                            match interact.interaction_type {
                                mmo_schema::client::InteractionType::Custom(sub) => sub as u16,
                                _ => 0,
                            },
                        )
                    };
                    if self.packets.has_module(module_id) {
                        if let Ok(Some(payload)) = self.packets.dispatch(
                            module_id,
                            sub_opcode,
                            guid,
                            &interact.payload,
                            &mut self.entity_store,
                        ) {
                            self.pending_broadcast_jobs
                                .push(crate::aoi::AoiBroadcastJob {
                                    session_id: input.session_id,
                                    player_guid: guid,
                                    packets: vec![
                                        mmo_schema::client::ClientPacket::InteractionResult(
                                            mmo_schema::client::InteractionResultPacket {
                                                action_id: interact.action_id,
                                                success: true,
                                                error_code: 0,
                                                payload,
                                            },
                                        ),
                                    ],
                                });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Configures an external or custom `CollisionWorld` implementation for authoritative simulation.
    pub fn with_collision_world(
        mut self,
        collision_world: std::sync::Arc<std::sync::RwLock<dyn mmo_core::collision::CollisionWorld>>,
    ) -> Self {
        self.collision_world = collision_world;
        self
    }

    /// Authoritatively registers a world object and synchronizes its bounds with the collision world.
    pub fn insert_world_object(
        &mut self,
        obj: mmo_core::world::WorldObject,
    ) -> Option<mmo_core::world::WorldObject> {
        let id = obj.object_id;
        let bounds = obj.bounds;
        let prev = self.sectors.insert_object(obj);
        if let Ok(mut collision) = self.collision_world.write() {
            collision.register_obstacle(id, bounds);
        }
        prev
    }

    /// Authoritatively removes a world object and synchronizes its bounds with the collision world.
    pub fn remove_world_object(&mut self, object_id: Guid) -> Option<mmo_core::world::WorldObject> {
        let prev = self.sectors.remove_object(object_id);
        if prev.is_some() {
            if let Ok(mut collision) = self.collision_world.write() {
                collision.unregister_obstacle(object_id);
            }
        }
        prev
    }

    /// Validates whether an incoming upstream packet from a gateway carries a valid (non-stale) session epoch.
    pub fn is_valid_upstream_epoch(&self, guid: Guid, packet_epoch: u64) -> bool {
        let current_epoch = *self.session_epochs.get(&guid).unwrap_or(&0);
        packet_epoch >= current_epoch
    }

    /// Returns a cloned sender for pushing client ACKs into the non-blocking channel.
    pub fn ack_sender(
        &self,
    ) -> tokio::sync::mpsc::UnboundedSender<mmo_schema::client::ClientAckPacket> {
        self.ack_sender.clone()
    }

    /// Returns a cloned sender for container transfer notifications.
    pub fn container_transfer_sender(
        &self,
    ) -> tokio::sync::mpsc::UnboundedSender<ContainerTransferNotification> {
        self.container_transfer_sender.clone()
    }

    /// Enqueues a container transfer notification into the simulation node's non-blocking channel.
    pub fn queue_container_transfer(&self, notif: ContainerTransferNotification) {
        let _ = self.container_transfer_sender.send(notif);
    }

    /// Queues a client ACK into the simulation's non-blocking channel.
    pub fn queue_client_ack(&self, ack: mmo_schema::client::ClientAckPacket) {
        let _ = self.ack_sender.send(ack);
    }

    /// Returns a cloned sender for pushing ability compensation events into the non-blocking channel.
    pub fn ability_compensation_sender(
        &self,
    ) -> tokio::sync::mpsc::UnboundedSender<AbilityCompensation> {
        self.ability_compensation_sender.clone()
    }

    /// Queues an ability compensation event into the simulation's non-blocking channel.
    pub fn queue_ability_compensation(&self, comp: AbilityCompensation) {
        let _ = self.ability_compensation_sender.send(comp);
    }

    /// Sets or attaches the asynchronous snapshot sender pipeline.
    pub fn set_snapshot_sender(
        &mut self,
        sender: tokio::sync::mpsc::Sender<crate::snapshotter::SnapshotPayload>,
    ) {
        self.snapshot_sender = Some(sender);
    }

    /// Sets the interval (in simulation ticks) between periodic entity state snapshots.
    pub fn set_snapshot_interval_ticks(&mut self, ticks: u64) {
        self.snapshot_interval_ticks = ticks.max(1);
    }

    /// Returns the current simulation monotonic timestamp.
    pub fn current_time(&self) -> Instant {
        self.current_time
    }

    /// Overrides the current simulation monotonic timestamp.
    pub fn set_current_time(&mut self, time: Instant) {
        self.current_time = time;
    }

    /// Advances the simulation monotonic timestamp by the given duration.
    pub fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
    }

    /// Returns an immutable reference to the ghost store.
    pub fn ghost_store(&self) -> &GhostStore {
        &self.ghost_store
    }

    /// Returns a mutable reference to the ghost store.
    pub fn ghost_store_mut(&mut self) -> &mut GhostStore {
        &mut self.ghost_store
    }

    /// Wraps this simulation instance in a shared asynchronous `RwLock`.
    pub fn into_shared(self) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(self))
    }

    /// Creates a [`NodeServiceImpl`] backed by a shared simulation instance.
    pub fn create_service(shared_sim: Arc<RwLock<Self>>) -> NodeServiceImpl {
        NodeServiceImpl::new(shared_sim)
    }

    /// Creates a [`GhostBroadcaster`] configured for this simulation node.
    pub fn create_ghost_broadcaster(&self, boundary_margin: f32) -> GhostBroadcaster {
        GhostBroadcaster::new(self.node_id as u32, boundary_margin)
    }

    /// Checks whether an entity currently holds an active migration lease.
    pub fn is_migrating(&self, guid: Guid) -> bool {
        self.migrating_leases.contains_key(&guid)
    }

    /// Acquires a migration lease for an entity, freezing its movement and mutations.
    ///
    /// Returns `true` if the lease was acquired, or `false` if already migrating or entity not found.
    pub fn acquire_migration_lease(&mut self, guid: Guid) -> bool {
        if !self.entity_store.contains_guid(guid) || self.is_migrating(guid) {
            return false;
        }
        self.migrating_leases.insert(guid, self.current_time);
        true
    }

    /// Releases a migration lease for an entity.
    pub fn release_migration_lease(&mut self, guid: Guid) -> bool {
        self.migrating_leases.remove(&guid).is_some()
    }

    /// Sets the last migration tick for an entity to enforce cooldowns.
    pub fn set_last_migration_tick(&mut self, guid: Guid, tick: u64) {
        self.last_migration_ticks.insert(guid, tick);
    }

    /// Returns the last migration tick for an entity, if recorded.
    pub fn get_last_migration_tick(&self, guid: Guid) -> Option<u64> {
        self.last_migration_ticks.get(&guid).copied()
    }

    /// Checks whether an entity is currently in migration cooldown.
    pub fn is_in_migration_cooldown(&self, guid: Guid, cooldown_ticks: u64) -> bool {
        if let Some(&last_tick) = self.last_migration_ticks.get(&guid) {
            self.tick_counter < last_tick + cooldown_ticks
        } else {
            false
        }
    }

    /// Evaluates whether a proposed movement qualifies for boundary migration across partitions.
    ///
    /// Returns `true` if `new_pos` is outside local bounds by at least `margin` world units,
    /// the entity is not currently migrating, and migration cooldown has elapsed.
    pub fn check_boundary_migration(
        &self,
        guid: Guid,
        new_pos: (f32, f32, f32),
        margin: f32,
        cooldown_ticks: u64,
    ) -> bool {
        // Child entities attached to a parent platform never trigger independent boundary migration
        if let Some(entity) = self.entity_store.get_by_guid(guid) {
            if entity.parent_guid.is_some() {
                return false;
            }
        }
        if self.is_migrating(guid) || self.is_in_migration_cooldown(guid, cooldown_ticks) {
            return false;
        }
        self.octree.bounds.distance_to_point(new_pos) >= margin
    }

    /// Collects an entity and all its transitive hierarchy descendants as an atomic migration group.
    pub fn collect_migration_group(&self, root_guid: Guid) -> Vec<Guid> {
        self.entity_store.collect_hierarchy_group(root_guid)
    }

    /// Collects all entity GUIDs currently residing inside the specified bounding box volume.
    pub fn find_entities_in_bounds(&self, bounds: &BoundingBox3D) -> Vec<Guid> {
        self.octree.query_range(bounds)
    }

    /// Spawns an entity into both `EntityStore` and `Octree`, dispatching `OnSpawn`.
    ///
    /// Returns `true` if the entity was successfully spawned within octree boundaries.
    pub fn spawn_entity(&mut self, guid: Guid, position: (f32, f32, f32)) -> bool {
        if !self.octree.bounds.contains(position) {
            return false;
        }

        self.entity_store.spawn(guid, position);
        self.octree.insert(guid, position);
        self.hooks.dispatch(&Event::OnSpawn {
            entity: guid,
            position,
        });
        true
    }

    /// Authoritatively handles a client `PlaceObject` request: validates rate limit, range,
    /// line of sight and non-penetration, then spawns the object into the sector index.
    ///
    /// Returns the new object's GUID on success.
    pub fn handle_place_object(
        &mut self,
        player: Guid,
        packet: &mmo_schema::client::PlaceObjectPacket,
    ) -> Result<Guid, crate::sector::PlacementError> {
        use mmo_core::container::ContainerOwner;
        use mmo_core::world::{Transform3D, WorldObject};

        let player_pos = self
            .entity_store
            .get_by_guid(player)
            .map(|e| e.position)
            .unwrap_or((0.0, 0.0, 0.0));
        let pos = (packet.pos.x, packet.pos.y, packet.pos.z);
        if !pos.0.is_finite() || !pos.1.is_finite() || !pos.2.is_finite() {
            return Err(crate::sector::PlacementError::InvalidTransform(
                "Position coordinates must be finite".to_string(),
            ));
        }

        let rot = packet.rot;
        if !rot.0.is_finite() || !rot.1.is_finite() || !rot.2.is_finite() || !rot.3.is_finite() {
            return Err(crate::sector::PlacementError::InvalidTransform(
                "Rotation quaternion components must be finite".to_string(),
            ));
        }
        let rot_len_sq = rot.0 * rot.0 + rot.1 * rot.1 + rot.2 * rot.2 + rot.3 * rot.3;
        if (rot_len_sq - 1.0).abs() > 0.05 {
            return Err(crate::sector::PlacementError::InvalidTransform(
                "Rotation quaternion must be normalized".to_string(),
            ));
        }

        let scale_raw = (packet.scale.x, packet.scale.y, packet.scale.z);
        if !scale_raw.0.is_finite() || !scale_raw.1.is_finite() || !scale_raw.2.is_finite() {
            return Err(crate::sector::PlacementError::InvalidTransform(
                "Scale dimensions must be finite".to_string(),
            ));
        }
        // Clamp scale dimensions to safe bounds [0.01, 100.0] to prevent spatial index poisoning
        let scale = (
            scale_raw.0.abs().clamp(0.01, 100.0),
            scale_raw.1.abs().clamp(0.01, 100.0),
            scale_raw.2.abs().clamp(0.01, 100.0),
        );

        // Prefab footprint is plugin-defined; core uses the scaled unit cube centered on pos.
        let half = (0.5 * scale.0, 0.5 * scale.1, 0.5 * scale.2);
        let bounds = BoundingBox3D::new(
            (pos.0 - half.0, pos.1 - half.1, pos.2 - half.2),
            (pos.0 + half.0, pos.1 + half.1, pos.2 + half.2),
        );
        self.sectors
            .validate_placement(player, player_pos, pos, &bounds)?;

        let seq = self.next_world_object_seq;
        self.next_world_object_seq = self.next_world_object_seq.wrapping_add(1);
        let object_id = Guid::new(mmo_core::EntityType::DynamicObject, self.node_id, seq);
        let mut obj = WorldObject::new(
            object_id,
            packet.prefab_id,
            Transform3D::new(pos, rot, scale),
            bounds,
            ContainerOwner::Character(player.raw()),
        );
        obj.properties = packet.properties.clone();
        self.sectors.insert_object(obj);
        if let Ok(mut collision) = self.collision_world.write() {
            collision.register_obstacle(object_id, bounds);
        }
        Ok(object_id)
    }

    /// Handles a client `InteractObject` request, validating range, interactivity, and plugin hooks.
    /// Returns `true` if the interaction is permitted and not vetoed; plugins react via hooks/events.
    pub fn handle_interact_object(
        &mut self,
        player: Guid,
        packet: &mmo_schema::client::InteractObjectPacket,
    ) -> bool {
        use mmo_core::world::world_object_flags;
        let Some(ent) = self.entity_store.get_by_guid(player) else {
            return false;
        };
        let target_guid = Guid::from_raw(packet.object_id);
        let Some(obj) = self.sectors.get_object(target_guid) else {
            return false;
        };
        if !obj.has_flag(world_object_flags::INTERACTIVE)
            || obj.has_flag(world_object_flags::LOCKED)
        {
            return false;
        }
        if obj.bounds.distance_to_point(ent.position) > crate::sector::MAX_INTERACTION_DISTANCE {
            return false;
        }

        let ctx = self.hooks.dispatch_with_entities(
            &Event::BeforeInteract {
                initiator: player,
                target: target_guid,
                action_id: packet.action_type,
            },
            &mut self.entity_store,
        );
        if ctx.is_canceled() {
            return false;
        }

        self.hooks.dispatch_with_entities(
            &Event::OnInteract {
                initiator: player,
                target: target_guid,
                action_id: packet.action_type,
            },
            &mut self.entity_store,
        );

        true
    }

    /// Authoritatively handles a client `WorldObjectResync` request with token-bucket rate limiting,
    /// protecting the server against state-serialization DOS attacks.
    pub fn handle_world_object_resync(
        &mut self,
        player: Guid,
        packet: &mmo_schema::client::WorldObjectResyncPacket,
    ) -> Option<mmo_schema::client::WorldObjectSpawnPacket> {
        if !self.sectors.check_rate_limit(player) {
            return None;
        }
        let obj = self.sectors.get_object(Guid::from_raw(packet.object_id))?;
        Some(crate::aoi::build_spawn_packet(obj))
    }

    /// Despawns an entity by GUID from both `EntityStore` and `Octree`, dispatching `OnDespawn`.
    ///
    /// Returns `true` if the entity existed and was removed.
    pub fn despawn_entity(&mut self, guid: Guid) -> bool {
        self.kinematic_states.remove(&guid);
        self.migrating_leases.remove(&guid);
        self.last_migration_ticks.remove(&guid);
        self.session_epochs.remove(&guid);
        self.aoi.unregister_by_guid(guid);
        self.sectors.clear_rate_limit(guid);
        self.instances.remove_entity(guid);
        if let Some(entity) = self.entity_store.despawn_by_guid(guid) {
            if !self.octree.remove(guid, entity.position) {
                self.octree.remove_by_guid(guid);
            }
            self.hooks.dispatch(&Event::OnDespawn { entity: guid });
            self.event_queue.push(EntityDespawnEvent { guid });
            true
        } else {
            false
        }
    }

    /// Executes a single simulation step.
    ///
    /// Registers a gameplay plugin to be initialized in this simulation node.
    pub fn add_plugin(&mut self, plugin: Box<dyn GameplayPlugin>) {
        self.plugin_manager.add_plugin(plugin);
    }

    /// Initializes and builds all registered plugins.
    pub fn init_plugins(&mut self) {
        self.plugin_manager.build_all(
            &mut self.hooks,
            &mut self.codecs,
            &mut self.event_queue,
            &mut self.packets,
            &mut self.tick_systems,
        );
    }

    /// Attaches a script hot-reloader coordinator to this simulation node.
    pub fn set_script_reloader(&mut self, reloader: Arc<dyn mmo_core::script::ScriptReloadable>) {
        self.script_reloader = Some(reloader);
    }

    /// Atomically reloads a WASM script module by ID at runtime.
    pub fn reload_script(
        &self,
        module_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<(), mmo_core::ScriptError> {
        if let Some(reloader) = &self.script_reloader {
            reloader
                .reload_module(module_id, wasm_bytes)
                .map(|_| ())
                .map_err(mmo_core::ScriptError::CompilationError)
        } else {
            Err(mmo_core::ScriptError::ModuleNotFound(
                "No script reloader attached to simulation node".to_string(),
            ))
        }
    }

    /// Returns a reference to the active script reloader, if attached.
    pub fn script_reloader(&self) -> Option<&Arc<dyn mmo_core::script::ScriptReloadable>> {
        self.script_reloader.as_ref()
    }

    /// Increments the `tick_counter`, runs batch tick systems, iterates through active entities
    /// in `EntityStore`, executes tick updates, and triggers synchronous hook dispatches (`Event::OnTick`).
    pub fn step(&mut self, dt_sec: f32) {
        let start = std::time::Instant::now();
        self.tick_counter += 1;
        self.advance_time(Duration::from_secs_f32(dt_sec));

        // 0. Drain queued inbound client packets
        self.drain_client_inputs();

        // 0.1 Drain pending client baseline ACKs in batch without lock contention
        while let Ok(ack) = self.ack_receiver.try_recv() {
            self.aoi.process_ack(
                ack.session_id,
                ack.observed_tick,
                ack.ack_mask,
                self.tick_counter,
            );
        }

        // 0.1 Drain pending cross-partition ability compensation refunds
        while let Ok(comp) = self.ability_compensation_receiver.try_recv() {
            if let Some(entity) = self.entity_store.get_mut_by_guid(comp.caster_guid) {
                if let Some(cd) = entity.get_component_mut::<mmo_core::action::CooldownTracker>() {
                    cd.reset_cooldown(comp.ability_id);
                    cd.reset_gcd();
                }
                if let Some(ref res_name) = comp.resource_name {
                    if let Some(attrs) = entity.get_component_mut::<mmo_core::AttributeSet>() {
                        let _ = attrs.modify_current(res_name, comp.resource_amount);
                    }
                }
            }
        }

        // 0.2 Drain pending container transfer notifications
        while let Ok(_notif) = self.container_transfer_receiver.try_recv() {}

        // 0.3 Reap idle instance domains and evict remaining non-player entities
        let reaped_instances = self.instances.reap_idle_instances();
        for (reaped_inst_id, evicted_guids) in reaped_instances {
            metrics::counter!("mmo_node_idle_instances_reaped_total").increment(1);
            for g in evicted_guids {
                self.despawn_entity(g);
            }
            eprintln!(
                "[Node {}] Reaped idle instance domain {}",
                self.node_id, reaped_inst_id
            );
        }

        // 1. Run batch tick systems registered by plugins
        let mut tick_ctx = TickContext {
            entities: &mut self.entity_store,
            octree: &mut self.octree,
            events: &mut self.event_queue,
            dt_sec,
            tick: self.tick_counter,
        };
        for system in &mut self.tick_systems {
            system.update(&mut tick_ctx);
        }

        // 2. Snapshot entity GUIDs to avoid borrowing conflicts during hook dispatch
        let guids: Vec<Guid> = self
            .entity_store
            .iter()
            .map(|(_, ent)| ent.guid)
            .filter(|g| !self.migrating_leases.contains_key(g))
            .collect();

        for guid in guids {
            self.hooks.dispatch(&Event::OnTick {
                entity: guid,
                tick: self.tick_counter,
                dt_sec,
            });
        }

        // 3. Update AoI observers and queue broadcast jobs
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut aoi_jobs =
            self.aoi
                .update_observers(&self.entity_store, &self.octree, self.tick_counter, now_ms);
        self.pending_broadcast_jobs.append(&mut aoi_jobs);

        let mut world_obj_jobs = self
            .aoi
            .update_world_object_observers(&self.entity_store, &self.sectors);
        self.pending_broadcast_jobs.append(&mut world_obj_jobs);

        // 3.5. Periodic asynchronous write-behind snapshot dispatch
        if let Some(ref sender) = self.snapshot_sender {
            if self
                .tick_counter
                .is_multiple_of(self.snapshot_interval_ticks)
            {
                let now_sec = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                for (_key, entity) in self.entity_store.iter() {
                    // Skip entities holding active migration leases
                    if self.migrating_leases.contains_key(&entity.guid) {
                        continue;
                    }

                    let attributes = self.codecs.serialize_all(&entity.components);
                    let state_blob = self.codecs.serialize_all_binary(&entity.components).ok();
                    let payload = crate::snapshotter::SnapshotPayload {
                        guid: entity.guid,
                        position: entity.position,
                        rotation: entity.rotation,
                        attributes,
                        state_blob,
                        version: self.tick_counter,
                        timestamp: now_sec,
                    };

                    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                        sender.try_send(payload)
                    {
                        metrics::counter!("mmo_node_snapshot_dropped_channel_full").increment(1);
                    }
                }
            }
        }

        // 3.6. Record historical spatial tick frame in zero-allocation ring buffer
        let Self {
            ref entity_store,
            ref migrating_leases,
            ref mut history,
            tick_counter,
            current_time,
            ..
        } = *self;

        let entity_records = entity_store.iter().filter_map(|(_key, entity)| {
            if migrating_leases.contains_key(&entity.guid) {
                return None;
            }
            let half = 0.5f32;
            let bounds = BoundingBox3D::new(
                (
                    entity.position.0 - half,
                    entity.position.1 - half,
                    entity.position.2 - half,
                ),
                (
                    entity.position.0 + half,
                    entity.position.1 + half,
                    entity.position.2 + half,
                ),
            );
            Some(crate::history::HistoricalEntityRecord {
                guid: entity.guid,
                position: entity.position,
                bounds,
                velocity: (0.0, 0.0, 0.0),
                yaw: entity.rotation,
                pitch: 0.0,
            })
        });
        history.push_frame(tick_counter, current_time, now_ms, entity_records);

        // 4. Record simulation loop metrics
        let duration = start.elapsed();
        self.last_tick_duration = duration;
        let duration_sec = duration.as_secs_f64();
        let node_id_str = self.node_id.to_string();

        metrics::histogram!("mmo_node_tick_duration_seconds", "node_id" => node_id_str.clone())
            .record(duration_sec);
        if duration > Self::TICK_INTERVAL {
            metrics::counter!("mmo_node_tick_overruns_total", "node_id" => node_id_str.clone())
                .increment(1);
        }
        metrics::gauge!("mmo_node_active_entities", "node_id" => node_id_str.clone())
            .set(self.entity_store.count() as f64);
        metrics::gauge!("mmo_node_aoi_observers", "node_id" => node_id_str)
            .set(self.aoi.observer_count() as f64);
    }

    /// Returns the execution duration of the last tick in milliseconds.
    pub fn last_tick_duration_ms(&self) -> f32 {
        self.last_tick_duration.as_secs_f32() * 1000.0
    }

    /// Computes spatial centroid of all active local entities, clamping each entity's
    /// coordinates to the node's assigned bounding box to prevent malicious centroid spoofing.
    pub fn compute_clamped_centroid(&self, bounds: &BoundingBox3D) -> (f32, f32, f32) {
        if self.entity_store.is_empty() {
            return bounds.center();
        }

        let mut sum_x = 0.0f32;
        let mut sum_y = 0.0f32;
        let mut sum_z = 0.0f32;
        let count = self.entity_store.count() as f32;

        for (_key, entity) in self.entity_store.iter() {
            let cx = entity.position.0.clamp(bounds.min.0, bounds.max.0);
            let cy = entity.position.1.clamp(bounds.min.1, bounds.max.1);
            let cz = entity.position.2.clamp(bounds.min.2, bounds.max.2);
            sum_x += cx;
            sum_y += cy;
            sum_z += cz;
        }

        (sum_x / count, sum_y / count, sum_z / count)
    }

    /// Drains and returns all queued AoI broadcast jobs accumulated during ticks.
    pub fn drain_broadcast_jobs(&mut self) -> Vec<AoiBroadcastJob> {
        std::mem::take(&mut self.pending_broadcast_jobs)
    }

    /// Registers a player session as an AoI observer.
    pub fn register_aoi_observer(
        &mut self,
        session_id: u64,
        player_guid: Guid,
        config: ObserverConfig,
    ) {
        self.aoi.register_observer(session_id, player_guid, config);
    }

    /// Unregisters an AoI observer by session ID.
    pub fn unregister_aoi_observer(&mut self, session_id: u64) -> Option<ObserverState> {
        self.aoi.unregister_observer(session_id)
    }

    /// Convenience helper executing a standard 20 Hz (50ms) simulation tick step.
    pub fn step_tick(&mut self) {
        self.step(Self::TICK_DT_SEC);
    }

    /// Performs an authoritative lag-compensated raycast against historical entity bounding boxes.
    pub fn raycast_historical(
        &self,
        origin: (f32, f32, f32),
        direction: (f32, f32, f32),
        max_distance: f32,
        target_time: Instant,
        exclude_guid: Option<Guid>,
    ) -> Option<crate::history::HistoricalHitResult> {
        self.history.raycast_historical(
            origin,
            direction,
            max_distance,
            target_time,
            self.current_time,
            exclude_guid,
        )
    }

    /// Performs a lag-compensated raycast using millisecond timestamps.
    pub fn raycast_historical_ms(
        &self,
        origin: (f32, f32, f32),
        direction: (f32, f32, f32),
        max_distance: f32,
        target_ms: u64,
        current_ms: u64,
        exclude_guid: Option<Guid>,
    ) -> Option<crate::history::HistoricalHitResult> {
        self.history.raycast_historical_ms(
            origin,
            direction,
            max_distance,
            target_ms,
            current_ms,
            exclude_guid,
        )
    }

    /// Authoritatively validates whether a target entity was within range at historical timestamp `target_time`.
    pub fn validate_historical_range(
        &self,
        attacker_pos: (f32, f32, f32),
        target_guid: Guid,
        target_time: Instant,
        max_range: f32,
    ) -> Result<f32, crate::history::HistoricalQueryError> {
        self.history.validate_historical_range(
            attacker_pos,
            target_guid,
            target_time,
            self.current_time,
            max_range,
        )
    }

    /// Maximum number of consecutive catch-up ticks executed per cycle before yielding to Tokio.
    pub const MAX_CATCHUP_TICKS: usize = 3;

    /// Runs a fixed number of 20 Hz simulation ticks asynchronously using a clamped tick accumulator.
    ///
    /// Executes at most [`Self::MAX_CATCHUP_TICKS`] catch-up ticks per cycle to prevent the "spiral of death"
    /// and starvation of Tokio asynchronous I/O tasks.
    pub async fn run_ticks(&mut self, count: usize) -> u64 {
        let tick_duration = Self::TICK_INTERVAL;
        let mut next_tick = tokio::time::Instant::now() + tick_duration;
        let mut completed = 0;

        while completed < count {
            let now = tokio::time::Instant::now();
            if now < next_tick {
                tokio::time::sleep_until(next_tick).await;
            }

            let mut catchup = 0;
            while tokio::time::Instant::now() >= next_tick
                && completed < count
                && catchup < Self::MAX_CATCHUP_TICKS
            {
                self.step_tick();
                completed += 1;
                catchup += 1;
                next_tick += tick_duration;
            }

            // Clamp residual lag to prevent spiral of death
            let now_after = tokio::time::Instant::now();
            if now_after >= next_tick {
                next_tick = now_after + tick_duration;
            }
        }

        self.tick_counter
    }

    /// Calculates the server-authoritative delta time for an entity movement.
    ///
    /// If the entity is moving for the first time on this node, returns nominal [`Self::TICK_DT_SEC`].
    /// If the elapsed time since last move exceeds 1.0 second (idle time gap), clamps to
    /// [`Self::TICK_DT_SEC`] to prevent time-banking speed exploits.
    /// For sub-microsecond intervals (such as synchronous test calls), floors to nominal [`Self::TICK_DT_SEC`].
    pub fn compute_server_dt(&mut self, guid: Guid) -> f32 {
        self.compute_server_dt_with_seq(guid, 1)
    }

    /// Calculates server-authoritative delta time with jitter tolerance based on sequence delta.
    pub fn compute_server_dt_with_seq(&mut self, guid: Guid, seq_delta: u32) -> f32 {
        let now = self.current_time;
        let state = self
            .kinematic_states
            .entry(guid)
            .or_insert_with(|| EntityKinematicState::new(now));

        if !state.initialized {
            state.initialized = true;
            state.last_move_time = now;
            return Self::TICK_DT_SEC;
        }

        let elapsed = now.duration_since(state.last_move_time).as_secs_f32();
        state.last_move_time = now;

        if elapsed > 1.0 {
            // Idle gap clamping: prevent banking idle time
            Self::TICK_DT_SEC
        } else if elapsed < Self::TICK_DT_SEC {
            // Jitter tolerance: packets arriving compressed by network buffer flushes
            // floor to nominal tick duration per sequence increment, capped at 0.2s
            (Self::TICK_DT_SEC * (seq_delta.max(1) as f32)).min(0.2)
        } else {
            elapsed
        }
    }

    /// Validates and processes a client-reported movement packet with sequence monotonicity checking.
    ///
    /// The server strictly computes elapsed delta time from internal monotonic clocks and ignores
    /// any client-provided delta time, protecting against speed hacks and spoofed teleports.
    pub fn process_client_move_with_seq(
        &mut self,
        guid: Guid,
        new_pos: (f32, f32, f32),
        sequence: u32,
    ) -> Result<(f32, f32, f32), (f32, f32, f32)> {
        // 0. Migration lease check: movement is strictly frozen while migrating
        if self.is_migrating(guid) {
            return match self.entity_store.get_by_guid(guid) {
                Some(e) => Err(e.position),
                None => Err(new_pos),
            };
        }

        // 1. Fetch current entity coordinates
        let current_pos = match self.entity_store.get_by_guid(guid) {
            Some(entity) => entity.position,
            None => return Err(new_pos),
        };

        // 2. Monotonic sequence check (if sequence is specified and entity was already initialized)
        let seq_delta = if sequence > 0 {
            if let Some(state) = self.kinematic_states.get(&guid) {
                if state.initialized && sequence <= state.last_sequence {
                    return Err(current_pos);
                }
                if state.initialized && state.last_sequence > 0 {
                    sequence - state.last_sequence
                } else {
                    1
                }
            } else {
                1
            }
        } else {
            1
        };

        // 3. Compute server-authoritative delta time with sequence jitter tolerance
        let dt_sec = self.compute_server_dt_with_seq(guid, seq_delta);

        // 4. Kinematic & Collision validation (speed caps, teleport threshold, NaN/Inf, dt bounds, obstacle collision)
        let kin_state = self.kinematic_states.get_mut(&guid);
        if let Ok(collision_guard) = self.collision_world.read() {
            if let Err(violation) = self.kinematics.validate_move_with_collision(
                current_pos,
                new_pos,
                dt_sec,
                &*collision_guard,
                Some(guid),
                kin_state,
                self.tick_counter,
            ) {
                return Err(violation.snap_back);
            }
        } else if let Err(violation) = self.kinematics.validate_move(current_pos, new_pos, dt_sec) {
            return Err(violation.snap_back);
        }

        // 5. Spatial world boundary check
        if !self.octree.bounds.contains(new_pos) {
            return Err(current_pos);
        }

        // 5.5. Entity state check (incapacitated or movement locked)
        if let Some(entity) = self.entity_store.get_by_guid(guid) {
            if entity.is_incapacitated() || entity.is_movement_locked() {
                return Err(current_pos);
            }
        }

        // 6. Synchronous BeforeMove hook dispatch (veto / cancel support)
        let ctx = self.hooks.dispatch_with_entities(
            &Event::BeforeMove {
                entity: guid,
                from: current_pos,
                to: new_pos,
            },
            &mut self.entity_store,
        );
        if ctx.is_canceled() {
            return Err(current_pos);
        }

        // Synchronous Action Interruption on movement
        let dx = new_pos.0 - current_pos.0;
        let dy = new_pos.1 - current_pos.1;
        let dz = new_pos.2 - current_pos.2;
        let moved = (dx * dx + dy * dy + dz * dz) > 1e-4;
        if moved {
            if let Some(entity) = self.entity_store.get_mut_by_guid(guid) {
                if let Some(queue) = entity.get_component_mut::<mmo_core::ActionQueue>() {
                    let should_interrupt = queue
                        .current()
                        .map(|a| a.interrupt_flags.cancel_on_movement)
                        .unwrap_or(false);
                    if should_interrupt {
                        queue.interrupt(
                            guid,
                            mmo_core::InterruptReason::Movement,
                            &mut self.event_queue,
                        );
                    }
                }
            }
        }

        // 7. Update EntityStore position
        if let Some(entity) = self.entity_store.get_mut_by_guid(guid) {
            entity.position = new_pos;
        }

        // 8. Update Octree spatial index
        if !self.octree.remove(guid, current_pos) {
            self.octree.remove_by_guid(guid);
        }
        self.octree.insert(guid, new_pos);

        // 9. Update kinematic sequence
        if let Some(state) = self.kinematic_states.get_mut(&guid) {
            if sequence > 0 {
                state.last_sequence = sequence;
            }
        }

        // 9.5. Update parent-child hierarchy
        let parent_transform = self
            .entity_store
            .get_by_guid(guid)
            .and_then(|c| c.parent_guid)
            .and_then(|p_guid| {
                self.entity_store
                    .get_by_guid(p_guid)
                    .map(|p| (p.position, p.rotation))
            });
        if let Some((parent_pos, parent_rot)) = parent_transform {
            if let Some(child) = self.entity_store.get_mut_by_guid(guid) {
                child.local_position =
                    mmo_core::entity::world_to_local(parent_pos, parent_rot, new_pos);
            }
        }

        let has_children = self
            .entity_store
            .get_by_guid(guid)
            .map(|e| !e.children.is_empty())
            .unwrap_or(false);
        if has_children {
            let moved_children = self.entity_store.propagate_hierarchy_transforms(guid);
            for (child_guid, old_pos, child_new_pos) in moved_children {
                if !self.octree.remove(child_guid, old_pos) {
                    self.octree.remove_by_guid(child_guid);
                }
                self.octree.insert(child_guid, child_new_pos);
                if let Some(child_k_state) = self.kinematic_states.get_mut(&child_guid) {
                    child_k_state.last_position = child_new_pos;
                }
            }
        }

        // 10. Dispatch OnMove hook
        self.hooks.dispatch(&Event::OnMove {
            entity: guid,
            position: new_pos,
        });

        Ok(new_pos)
    }

    /// Processes a client-reported movement packet authoritatively.
    ///
    /// Accepts `_client_dt_sec` for interface compatibility, but calculates delta time
    /// strictly on the server to prevent client spoofing exploits.
    pub fn process_client_move(
        &mut self,
        guid: Guid,
        new_pos: (f32, f32, f32),
        _client_dt_sec: f32,
    ) -> Result<(f32, f32, f32), (f32, f32, f32)> {
        self.process_client_move_with_seq(guid, new_pos, 0)
    }

    /// Explicit deterministic move validation bypassing server monotonic clocks.
    /// Used for synthetic kinematic test suites and regression verifications.
    pub fn process_client_move_deterministic(
        &mut self,
        guid: Guid,
        new_pos: (f32, f32, f32),
        dt_sec: f32,
    ) -> Result<(f32, f32, f32), (f32, f32, f32)> {
        let current_pos = match self.entity_store.get_by_guid(guid) {
            Some(entity) => entity.position,
            None => return Err(new_pos),
        };

        let kin_state = self.kinematic_states.get_mut(&guid);
        if let Ok(collision_guard) = self.collision_world.read() {
            if let Err(violation) = self.kinematics.validate_move_with_collision(
                current_pos,
                new_pos,
                dt_sec,
                &*collision_guard,
                Some(guid),
                kin_state,
                self.tick_counter,
            ) {
                return Err(violation.snap_back);
            }
        } else if let Err(violation) = self.kinematics.validate_move(current_pos, new_pos, dt_sec) {
            return Err(violation.snap_back);
        }

        if !self.octree.bounds.contains(new_pos) {
            return Err(current_pos);
        }

        if let Some(entity) = self.entity_store.get_by_guid(guid) {
            if entity.is_incapacitated() || entity.is_movement_locked() {
                return Err(current_pos);
            }
        }

        let ctx = self.hooks.dispatch_with_entities(
            &Event::BeforeMove {
                entity: guid,
                from: current_pos,
                to: new_pos,
            },
            &mut self.entity_store,
        );
        if ctx.is_canceled() {
            return Err(current_pos);
        }

        if let Some(entity) = self.entity_store.get_mut_by_guid(guid) {
            entity.position = new_pos;
        }

        if !self.octree.remove(guid, current_pos) {
            self.octree.remove_by_guid(guid);
        }
        self.octree.insert(guid, new_pos);

        // Update parent-child hierarchy
        let parent_transform = self
            .entity_store
            .get_by_guid(guid)
            .and_then(|c| c.parent_guid)
            .and_then(|p_guid| {
                self.entity_store
                    .get_by_guid(p_guid)
                    .map(|p| (p.position, p.rotation))
            });
        if let Some((parent_pos, parent_rot)) = parent_transform {
            if let Some(child) = self.entity_store.get_mut_by_guid(guid) {
                child.local_position =
                    mmo_core::entity::world_to_local(parent_pos, parent_rot, new_pos);
            }
        }

        let has_children = self
            .entity_store
            .get_by_guid(guid)
            .map(|e| !e.children.is_empty())
            .unwrap_or(false);
        if has_children {
            let moved_children = self.entity_store.propagate_hierarchy_transforms(guid);
            for (child_guid, old_pos, child_new_pos) in moved_children {
                if !self.octree.remove(child_guid, old_pos) {
                    self.octree.remove_by_guid(child_guid);
                }
                self.octree.insert(child_guid, child_new_pos);
                if let Some(child_k_state) = self.kinematic_states.get_mut(&child_guid) {
                    child_k_state.last_position = child_new_pos;
                }
            }
        }

        self.hooks.dispatch(&Event::OnMove {
            entity: guid,
            position: new_pos,
        });

        Ok(new_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    fn test_bounds() -> BoundingBox3D {
        BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0))
    }

    #[test]
    fn test_tick_simulation_initialization() {
        let sim = TickSimulation::new(1, test_bounds());
        assert_eq!(sim.node_id, 1);
        assert_eq!(sim.tick_counter, 0);
        assert_eq!(sim.entity_store.count(), 0);
        assert_eq!(sim.octree.len(), 0);
        assert!(sim.octree.is_empty());
        assert_eq!(sim.ghost_store.len(), 0);
        assert!(sim.ghost_store.is_empty());
    }

    #[test]
    fn test_process_client_move_normal_pass() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 100);
        let start_pos = (0.0, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Move 0.4 units over dt = 0.05s (speed = 8.0 units/s <= max 10.0)
        let target_pos = (0.4, 0.0, 0.0);
        let result = sim.process_client_move(guid, target_pos, 0.05);

        assert_eq!(result, Ok(target_pos));

        // EntityStore updated
        let entity = sim
            .entity_store
            .get_by_guid(guid)
            .expect("entity must exist");
        assert_eq!(entity.position, target_pos);

        // Octree updated
        let nearby = sim.octree.query_radius(target_pos, 0.1);
        assert_eq!(nearby, vec![guid]);
    }

    #[test]
    fn test_process_client_move_speed_hack_rejection_and_snapback() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 200);
        let start_pos = (5.0, 5.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Attempt to move 5 units in 0.05s (speed = 100.0 units/s > max 10.0)
        let hack_pos = (10.0, 5.0, 0.0);
        let result = sim.process_client_move(guid, hack_pos, 0.05);

        // Expect rejection with snap-back to start_pos
        assert_eq!(result, Err(start_pos));

        // Authoritative entity store remains unchanged
        let entity = sim
            .entity_store
            .get_by_guid(guid)
            .expect("entity must exist");
        assert_eq!(entity.position, start_pos);

        // Octree index still reflects original position
        let old_pos_query = sim.octree.query_radius(start_pos, 0.1);
        assert_eq!(old_pos_query, vec![guid]);

        let hack_pos_query = sim.octree.query_radius(hack_pos, 0.1);
        assert!(hack_pos_query.is_empty());
    }

    #[test]
    fn test_process_client_move_out_of_bounds_snap_back() {
        let bounds = BoundingBox3D::new((-10.0, -10.0, -10.0), (10.0, 10.0, 10.0));
        let mut sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 300);
        let start_pos = (9.8, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Move 0.3 units in 0.05s -> reaches 10.1 (outside bounds [-10, 10])
        let out_pos = (10.1, 0.0, 0.0);
        let result = sim.process_client_move(guid, out_pos, 0.05);

        assert_eq!(result, Err(start_pos));
        let entity = sim.entity_store.get_by_guid(guid).unwrap();
        assert_eq!(entity.position, start_pos);
    }

    #[test]
    fn test_process_client_move_hook_veto_snap_back() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 400);
        let start_pos = (0.0, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        // Register a hook that vetos any movement (e.g. stunned / rooted debuff)
        sim.hooks.register_fn(|event, ctx| {
            if let Event::BeforeMove { entity, .. } = event {
                ctx.cancel(format!("Entity {entity} is rooted"));
            }
        });

        let target_pos = (0.2, 0.0, 0.0);
        let result = sim.process_client_move(guid, target_pos, 0.05);

        assert_eq!(result, Err(start_pos));
        let entity = sim.entity_store.get_by_guid(guid).unwrap();
        assert_eq!(entity.position, start_pos);
    }

    #[test]
    fn test_movement_hooks_dispatched() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 500);
        let start_pos = (0.0, 0.0, 0.0);
        assert!(sim.spawn_entity(guid, start_pos));

        let before_count = Arc::new(AtomicUsize::new(0));
        let on_move_count = Arc::new(AtomicUsize::new(0));

        let b_clone = before_count.clone();
        let m_clone = on_move_count.clone();

        sim.hooks.register_fn(move |event, _| match event {
            Event::BeforeMove { .. } => {
                b_clone.fetch_add(1, Ordering::SeqCst);
            }
            Event::OnMove { .. } => {
                m_clone.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        });

        let target_pos = (0.3, 0.0, 0.0);
        let res = sim.process_client_move(guid, target_pos, 0.05);
        assert!(res.is_ok());

        assert_eq!(before_count.load(Ordering::SeqCst), 1);
        assert_eq!(on_move_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_step_iterates_entities_and_dispatches_tick_hooks() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let g1 = Guid::new(EntityType::Player, 1, 601);
        let g2 = Guid::new(EntityType::Npc, 1, 602);
        sim.spawn_entity(g1, (1.0, 1.0, 1.0));
        sim.spawn_entity(g2, (2.0, 2.0, 2.0));

        let tick_events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let t_clone = tick_events.clone();

        sim.hooks.register_fn(move |event, _| {
            if let Event::OnTick {
                entity,
                tick,
                dt_sec,
            } = event
            {
                t_clone.lock().unwrap().push((*entity, *tick, *dt_sec));
            }
        });

        assert_eq!(sim.tick_counter, 0);

        // Execute step 1 (50ms = 0.05s)
        sim.step(0.05);
        assert_eq!(sim.tick_counter, 1);

        {
            let events = tick_events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(events.contains(&(g1, 1, 0.05)));
            assert!(events.contains(&(g2, 1, 0.05)));
        }

        // Execute step 2
        sim.step(0.05);
        assert_eq!(sim.tick_counter, 2);

        {
            let events = tick_events.lock().unwrap();
            assert_eq!(events.len(), 4);
            assert!(events.contains(&(g1, 2, 0.05)));
            assert!(events.contains(&(g2, 2, 0.05)));
        }
    }

    #[tokio::test]
    async fn test_20hz_timed_tick_execution() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 700);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));

        let start = Instant::now();
        // Run 4 ticks at 20 Hz (50ms each = ~200ms total)
        let total_ticks = sim.run_ticks(4).await;
        let elapsed = start.elapsed();

        assert_eq!(total_ticks, 4);
        assert_eq!(sim.tick_counter, 4);
        // Expect at least 150ms elapsed given 50ms intervals
        assert!(
            elapsed >= Duration::from_millis(150),
            "Expected elapsed >= 150ms for 4 ticks, got {:?}",
            elapsed
        );
    }

    #[test]
    fn test_tick_simulation_aoi_broadcast_integration() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let player = Guid::new(EntityType::Player, 1, 100);
        let npc = Guid::new(EntityType::Npc, 1, 200);

        sim.spawn_entity(player, (10.0, 10.0, 10.0));
        sim.register_aoi_observer(1, player, ObserverConfig::default());

        // Step 1: Only player present, no other entities -> no broadcast jobs
        sim.step_tick();
        assert!(sim.drain_broadcast_jobs().is_empty());

        // Spawn NPC within AoI radius
        sim.spawn_entity(npc, (15.0, 10.0, 10.0));

        // Step 2: NPC enters AoI -> broadcast job generated
        sim.step_tick();
        let jobs = sim.drain_broadcast_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].session_id, 1);
        assert_eq!(jobs[0].player_guid, player);
        assert_eq!(jobs[0].packets.len(), 1);

        // Queue drained
        assert!(sim.drain_broadcast_jobs().is_empty());

        // Step 3: Despawn NPC
        sim.despawn_entity(npc);
        sim.step_tick();
        let jobs = sim.drain_broadcast_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].packets.len(), 1);
        match &jobs[0].packets[0] {
            mmo_schema::client::ClientPacket::EntityDespawn(d) => {
                assert_eq!(d.entity_id, npc.raw());
            }
            other => panic!("Expected EntityDespawn, got {:?}", other),
        }
    }

    #[test]
    fn test_process_client_move_spoofed_dt_rejected_and_snapback() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 991);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));

        // Malicious client tries to teleport 40.0 units while claiming dt_sec = 100.0
        // (40.0 / 100.0 = 0.4 <= 10.0 max speed).
        // The server MUST ignore the client's dt_sec and calculate elapsed time internally.
        let spoof_pos = (40.0, 0.0, 0.0);
        let result = sim.process_client_move(guid, spoof_pos, 100.0);

        // Verify speed hack is caught and snap back to origin is returned
        assert_eq!(result, Err((0.0, 0.0, 0.0)));
        assert_eq!(
            sim.entity_store.get_by_guid(guid).unwrap().position,
            (0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn test_idle_entity_time_banking_prevented() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 992);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));

        // Initial valid small move establishing baseline
        let init_pos = (0.2, 0.0, 0.0);
        assert_eq!(sim.process_client_move(guid, init_pos, 0.05), Ok(init_pos));

        // Simulate entity standing idle for 10 seconds without moving
        sim.advance_time(Duration::from_secs(10));

        // Client attempts to "bank" 10 seconds of idle time and move 40 units at once
        let dash_pos = (40.2, 0.0, 0.0);
        let result = sim.process_client_move(guid, dash_pos, 10.0);

        // Server clamps idle gap > 1.0s to 0.05s nominal tick duration, catching the exploit!
        assert_eq!(result, Err(init_pos));
        assert_eq!(
            sim.entity_store.get_by_guid(guid).unwrap().position,
            init_pos
        );
    }

    #[test]
    fn test_process_client_move_sequence_monotonicity() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 993);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));

        // Packet 1: sequence = 1, valid displacement (0.2 units)
        let pos1 = (0.2, 0.0, 0.0);
        assert_eq!(sim.process_client_move_with_seq(guid, pos1, 1), Ok(pos1));

        // Advance 50ms
        sim.advance_time(Duration::from_millis(50));

        // Replay of sequence = 1 should be rejected
        let replay_pos = (0.4, 0.0, 0.0);
        assert_eq!(
            sim.process_client_move_with_seq(guid, replay_pos, 1),
            Err(pos1)
        );

        // Out-of-order sequence = 0 (or smaller than 1) should be rejected
        assert_eq!(
            sim.process_client_move_with_seq(guid, replay_pos, 0),
            Ok(replay_pos)
        );

        // Advance 50ms and send monotonic sequence = 2
        sim.advance_time(Duration::from_millis(50));
        let pos2 = (0.6, 0.0, 0.0);
        assert_eq!(sim.process_client_move_with_seq(guid, pos2, 2), Ok(pos2));

        // Stale sequence = 2 sent again should be rejected
        sim.advance_time(Duration::from_millis(50));
        let pos3 = (0.8, 0.0, 0.0);
        assert_eq!(sim.process_client_move_with_seq(guid, pos3, 2), Err(pos2));
    }

    #[test]
    fn test_step_tick_telemetry_metrics() {
        let server = mmo_core::TelemetryServer::init().unwrap();
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 999);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));
        sim.step_tick();

        let render = server.render_metrics();
        assert!(render.contains("mmo_node_active_entities"));
        assert!(render.contains("mmo_node_tick_duration_seconds"));
    }

    #[test]
    fn test_migration_lease_lifecycle_and_freeze() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let guid = Guid::new(EntityType::Player, 1, 101);
        let start_pos = (5.0, 5.0, 5.0);
        assert!(sim.spawn_entity(guid, start_pos));

        assert!(!sim.is_migrating(guid));

        // Acquire lease
        assert!(sim.acquire_migration_lease(guid));
        assert!(sim.is_migrating(guid));
        // Double acquisition fails
        assert!(!sim.acquire_migration_lease(guid));

        // Movement is strictly frozen while under lease
        let move_pos = (5.2, 5.0, 5.0);
        let move_res = sim.process_client_move(guid, move_pos, 0.05);
        assert_eq!(move_res, Err(start_pos));
        assert_eq!(
            sim.entity_store.get_by_guid(guid).unwrap().position,
            start_pos
        );

        // Step tick does not crash or dispatch mutations to migrating entity
        sim.step_tick();

        // Release lease
        assert!(sim.release_migration_lease(guid));
        assert!(!sim.is_migrating(guid));

        // Movement passes after lease release
        let valid_res = sim.process_client_move(guid, move_pos, 0.05);
        assert_eq!(valid_res, Ok(move_pos));
    }

    #[test]
    fn test_boundary_migration_hysteresis_and_cooldown() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (10.0, 10.0, 10.0));
        let mut sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 102);
        assert!(sim.spawn_entity(guid, (9.5, 5.0, 5.0)));

        const MARGIN: f32 = 1.0;
        const COOLDOWN: u64 = 20;

        // Position slightly outside boundary (10.3) < margin (1.0) -> false
        let shallow_outside = (10.3, 5.0, 5.0);
        assert!(!sim.check_boundary_migration(guid, shallow_outside, MARGIN, COOLDOWN));

        // Position deep outside boundary (11.5) >= margin (1.0) -> true
        let deep_outside = (11.5, 5.0, 5.0);
        assert!(sim.check_boundary_migration(guid, deep_outside, MARGIN, COOLDOWN));

        // Set cooldown tick
        sim.set_last_migration_tick(guid, sim.tick_counter);
        assert!(sim.is_in_migration_cooldown(guid, COOLDOWN));

        // During cooldown, boundary migration is suppressed
        assert!(!sim.check_boundary_migration(guid, deep_outside, MARGIN, COOLDOWN));

        // Advance simulation ticks past cooldown
        for _ in 0..COOLDOWN + 1 {
            sim.step_tick();
        }
        assert!(!sim.is_in_migration_cooldown(guid, COOLDOWN));
        assert!(sim.check_boundary_migration(guid, deep_outside, MARGIN, COOLDOWN));
    }

    #[test]
    fn test_process_client_move_compressed_jitter_burst_accepted() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 999);
        assert!(sim.spawn_entity(guid, (50.0, 50.0, 0.0)));

        // Initial move at seq=1
        let res1 = sim.process_client_move_with_seq(guid, (50.5, 50.0, 0.0), 1);
        assert_eq!(res1, Ok((50.5, 50.0, 0.0)));

        // Second packet arrives in rapid jitter burst (e.g. 1ms elapsed) with seq=2
        // advancing another 0.5 units (within normal 10.0 m/s speed over nominal 50ms)
        sim.advance_time(std::time::Duration::from_millis(1));
        let res2 = sim.process_client_move_with_seq(guid, (51.0, 50.0, 0.0), 2);
        assert_eq!(res2, Ok((51.0, 50.0, 0.0)));
    }

    #[test]
    fn test_process_client_move_interrupts_active_action() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 888);
        assert!(sim.spawn_entity(guid, (50.0, 50.0, 0.0)));

        // Attach an active ActionQueue with cancel_on_movement: true
        let mut queue = mmo_core::ActionQueue::new();
        let action = mmo_core::TimedAction::new(
            999,
            5.0,
            mmo_core::ActionInterruptFlags {
                cancel_on_movement: true,
                cancel_on_damage: true,
                cancel_on_stun: true,
                cancel_on_target_lost: true,
            },
            None,
        );
        let mut events = mmo_core::EventQueue::new();
        queue
            .start_action(guid, action, mmo_core::QueuePolicy::Replace, &mut events)
            .unwrap();
        sim.entity_store
            .get_mut_by_guid(guid)
            .unwrap()
            .add_component(queue);

        // Move entity within valid speed cap (0.3 units <= 0.5 max displacement per nominal tick)
        let move_res = sim.process_client_move_with_seq(guid, (50.3, 50.0, 0.0), 1);
        assert_eq!(move_res, Ok((50.3, 50.0, 0.0)));

        // ActionQueue should now be empty (aborted)
        let ent = sim.entity_store.get_by_guid(guid).unwrap();
        let q = ent.get_component::<mmo_core::ActionQueue>().unwrap();
        assert!(!q.is_busy());

        // EventQueue should contain ActionInterruptedEvent
        let interrupted = sim.event_queue.drain::<mmo_core::ActionInterruptedEvent>();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].action_id, 999);
        assert_eq!(interrupted[0].reason, mmo_core::InterruptReason::Movement);
    }

    #[test]
    fn test_compute_clamped_centroid() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut sim = TickSimulation::new(1, bounds);

        // Empty store returns bounds center
        assert_eq!(sim.compute_clamped_centroid(&bounds), (50.0, 50.0, 50.0));

        let g1 = Guid::new(EntityType::Player, 1, 101);
        let g2 = Guid::new(EntityType::Player, 1, 102);

        sim.spawn_entity(g1, (20.0, 30.0, 40.0));
        // Directly spawn into entity_store with spoofed coordinates outside bounds (e.g. 500.0)
        sim.entity_store.spawn(g2, (500.0, 100.0, 0.0));

        // Coordinate (500.0) should be clamped to bounds.max.0 (100.0)
        // Average X: (20.0 + 100.0) / 2 = 60.0
        // Average Y: (30.0 + 100.0) / 2 = 65.0
        // Average Z: (40.0 + 0.0) / 2 = 20.0
        let centroid = sim.compute_clamped_centroid(&bounds);
        assert!((centroid.0 - 60.0).abs() < 0.001);
        assert!((centroid.1 - 65.0).abs() < 0.001);
        assert!((centroid.2 - 20.0).abs() < 0.001);

        // Step updates last_tick_duration
        sim.step(0.05);
        assert!(sim.last_tick_duration_ms() >= 0.0);
    }

    #[test]
    fn test_place_object_anti_cheat_float_and_quaternion_validation() {
        use mmo_schema::client::{PlaceObjectPacket, Vec3};
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (200.0, 200.0, 200.0));
        let mut sim = TickSimulation::new(1, bounds);
        let player = Guid::new(EntityType::Player, 1, 777);
        sim.spawn_entity(player, (10.0, 10.0, 10.0));
        let sector = mmo_core::world::SectorCoord3D::from_world_pos((10.0, 10.0, 10.0));
        sim.sectors.mark_sector_loaded(sector);

        // 1. NaN position rejected
        let nan_packet = PlaceObjectPacket {
            prefab_id: 1,
            pos: Vec3::new(f32::NAN, 10.0, 10.0),
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            properties: std::collections::HashMap::new(),
        };
        assert!(matches!(
            sim.handle_place_object(player, &nan_packet),
            Err(crate::sector::PlacementError::InvalidTransform(_))
        ));

        // 2. Unnormalized quaternion rejected
        let bad_quat_packet = PlaceObjectPacket {
            prefab_id: 1,
            pos: Vec3::new(12.0, 10.0, 10.0),
            rot: (5.0, 5.0, 5.0, 5.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            properties: std::collections::HashMap::new(),
        };
        assert!(matches!(
            sim.handle_place_object(player, &bad_quat_packet),
            Err(crate::sector::PlacementError::InvalidTransform(_))
        ));

        // 3. Valid placement succeeds and clamps massive scale
        let valid_packet = PlaceObjectPacket {
            prefab_id: 1,
            pos: Vec3::new(12.0, 10.0, 10.0),
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3::new(500.0, 0.0001, 1.0),
            properties: std::collections::HashMap::new(),
        };
        let obj_id = sim.handle_place_object(player, &valid_packet).unwrap();
        let obj = sim.sectors.get_object(obj_id).unwrap();
        assert_eq!(obj.transform.scale.0, 100.0); // clamped to MAX_SCALE
        assert_eq!(obj.transform.scale.1, 0.01); // clamped to MIN_SCALE
    }

    #[test]
    fn test_world_object_resync_rate_limiting() {
        use mmo_schema::client::{PlaceObjectPacket, Vec3, WorldObjectResyncPacket};
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (200.0, 200.0, 200.0));
        let mut sim = TickSimulation::new(1, bounds);
        let player = Guid::new(EntityType::Player, 1, 555);
        sim.spawn_entity(player, (10.0, 10.0, 10.0));
        let sector = mmo_core::world::SectorCoord3D::from_world_pos((10.0, 10.0, 10.0));
        sim.sectors.mark_sector_loaded(sector);

        let place_pkt = PlaceObjectPacket {
            prefab_id: 10,
            pos: Vec3::new(11.0, 10.0, 10.0),
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            properties: std::collections::HashMap::new(),
        };
        let obj_id = sim.handle_place_object(player, &place_pkt).unwrap();

        let resync_pkt = WorldObjectResyncPacket {
            object_id: obj_id.raw(),
        };

        // First resync succeeds
        let res1 = sim.handle_world_object_resync(player, &resync_pkt);
        assert!(res1.is_some());
        assert_eq!(res1.unwrap().object_id, obj_id.raw());

        // Exhaust token bucket
        for _ in 0..15 {
            sim.handle_world_object_resync(player, &resync_pkt);
        }

        // Subsequent resync is rate-limited and rejected
        let res_exhausted = sim.handle_world_object_resync(player, &resync_pkt);
        assert!(
            res_exhausted.is_none(),
            "Resync should be rejected when token bucket exhausted"
        );
    }

    #[test]
    fn test_sim_movement_blocked_by_placed_world_object() {
        use mmo_schema::client::{PlaceObjectPacket, Vec3};

        let mut sim = TickSimulation::new(
            1,
            BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0)),
        );
        let player = Guid::new(EntityType::Player, 1, 1);
        sim.spawn_entity(player, (5.0, 10.0, 10.0));

        let sector = mmo_core::world::SectorCoord3D::from_world_pos((10.0, 10.0, 10.0));
        sim.sectors.mark_sector_loaded(sector);

        // Place object at (7.0, 10.0, 10.0) with scale 1.0
        let place_pkt = PlaceObjectPacket {
            prefab_id: 10,
            pos: Vec3::new(7.0, 10.0, 10.0),
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            properties: std::collections::HashMap::new(),
        };
        let obj_id = sim.handle_place_object(player, &place_pkt).unwrap();

        // Player attempts to move through the placed object from 5.0 to 9.0
        let res = sim.process_client_move_deterministic(player, (9.0, 10.0, 10.0), 1.0);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), (5.0, 10.0, 10.0));

        // Now remove the object
        let removed = sim.remove_world_object(obj_id);
        assert!(removed.is_some());

        // Now unobstructed move succeeds
        let res2 = sim.process_client_move_deterministic(player, (6.0, 10.0, 10.0), 0.5);
        assert!(res2.is_ok());
    }

    #[test]
    fn test_sim_tick_history_recording_and_raycast() {
        let mut sim = TickSimulation::new(
            1,
            BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0)),
        );
        let target = Guid::new(EntityType::Player, 1, 10);
        let shooter = Guid::new(EntityType::Player, 1, 20);
        sim.spawn_entity(target, (10.0, 0.0, 0.0));
        sim.spawn_entity(shooter, (0.0, 0.0, 0.0));

        // Step 1
        sim.step_tick();
        assert_eq!(sim.history.len(), 1);
        let t0 = sim.current_time();

        // Move target to (20.0, 0.0, 0.0)
        let _ = sim.process_client_move_deterministic(target, (20.0, 0.0, 0.0), 1.0);
        sim.step_tick();
        assert_eq!(sim.history.len(), 2);
        let _t1 = sim.current_time();

        // Historical raycast directed at target's position at t0 (10.0, 0.0, 0.0)
        let hit = sim.raycast_historical((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), 50.0, t0, Some(shooter));
        assert!(hit.is_some());
        let hit_res = hit.unwrap();
        assert_eq!(hit_res.target_guid, target);
        assert!((hit_res.distance - 9.5).abs() < 1e-3);

        // Historical range validation
        let range_res = sim.validate_historical_range((0.0, 0.0, 0.0), target, t0, 15.0);
        assert!(range_res.is_ok());
        assert!((range_res.unwrap() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn test_sim_script_hot_reload_and_interact_lifecycle() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (1000.0, 1000.0, 1000.0));
        let mut sim = TickSimulation::new(1, bounds);

        let engine = Arc::new(mmo_gameplay::WasmScriptEngine::new().unwrap());
        let runtime = Arc::new(mmo_gameplay::WasmRuntime::new(engine).unwrap());
        let reloader = Arc::new(mmo_gameplay::ScriptHotReloader::new(runtime.clone()));

        sim.set_script_reloader(reloader);
        sim.add_plugin(Box::new(mmo_gameplay::WasmScriptPlugin::new(runtime)));
        sim.init_plugins();

        let e1 = Guid::new(mmo_core::EntityType::Player, 1, 10);
        let e2 = Guid::new(mmo_core::EntityType::Player, 1, 20);
        sim.spawn_entity(e1, (10.0, 10.0, 10.0));
        sim.spawn_entity(e2, (20.0, 20.0, 20.0));

        // Version 1 of the script: vetoes interaction
        let wat_v1 = r#"
            (module
                (func (export "on_before_interact") (param i64 i64 i32) (result i32)
                    i32.const 1
                )
            )
        "#;
        assert!(sim.reload_script("interact", wat_v1.as_bytes()).is_ok());

        let ctx = sim.hooks.dispatch_with_entities(
            &Event::BeforeInteract {
                initiator: e1,
                target: e2,
                action_id: 10,
            },
            &mut sim.entity_store,
        );
        assert!(ctx.is_canceled());
        assert_eq!(ctx.cancel_reason(), Some("script_veto"));

        // Version 2 of the script: allows interaction
        let wat_v2 = r#"
            (module
                (func (export "on_before_interact") (param i64 i64 i32) (result i32)
                    i32.const 0
                )
            )
        "#;
        assert!(sim.reload_script("interact", wat_v2.as_bytes()).is_ok());

        let ctx2 = sim.hooks.dispatch_with_entities(
            &Event::BeforeInteract {
                initiator: e1,
                target: e2,
                action_id: 10,
            },
            &mut sim.entity_store,
        );
        assert!(!ctx2.is_canceled());

        // Version 3: runaway loop fails closed
        let wat_v3 = r#"
            (module
                (func (export "on_before_interact") (param i64 i64 i32) (result i32)
                    (loop (br 0))
                    i32.const 0
                )
            )
        "#;
        assert!(sim.reload_script("interact", wat_v3.as_bytes()).is_ok());

        let ctx3 = sim.hooks.dispatch_with_entities(
            &Event::BeforeInteract {
                initiator: e1,
                target: e2,
                action_id: 10,
            },
            &mut sim.entity_store,
        );
        assert!(ctx3.is_canceled());
        assert!(ctx3
            .cancel_reason()
            .unwrap()
            .contains("script_error: out of fuel"));

        // Simulation step continues unharmed
        sim.step(0.05);
        assert_eq!(sim.tick_counter, 1);
    }

    #[test]
    fn test_tick_simulation_world_object_aoi_streaming() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let mut sim = TickSimulation::new(1, bounds);

        let player = Guid::new(mmo_core::EntityType::Player, 1, 10);
        sim.spawn_entity(player, (10.0, 0.0, 10.0));
        sim.aoi.register_observer(
            1,
            player,
            ObserverConfig {
                base_radius: 100.0,
                ..Default::default()
            },
        );

        // Insert a world object in the player's vicinity
        let obj_guid = Guid::new(mmo_core::EntityType::DynamicObject, 1, 999);
        let obj = mmo_core::world::WorldObject::new(
            obj_guid,
            1,
            mmo_core::world::Transform3D::new(
                (15.0, 0.0, 15.0),
                (0.0, 0.0, 0.0, 1.0),
                (1.0, 1.0, 1.0),
            ),
            BoundingBox3D::new((14.0, -1.0, 14.0), (16.0, 1.0, 16.0)),
            mmo_core::container::ContainerOwner::Character(10),
        );
        sim.insert_world_object(obj);

        assert!(sim.pending_broadcast_jobs.is_empty());

        // Step simulation tick: world object AoI jobs should be queued!
        sim.step(0.05);

        assert!(!sim.pending_broadcast_jobs.is_empty());
        let has_world_obj_spawn = sim.pending_broadcast_jobs.iter().any(|job| {
            job.packets
                .iter()
                .any(|pkt| matches!(pkt, mmo_schema::client::ClientPacket::WorldObjectSpawn(_)))
        });
        assert!(
            has_world_obj_spawn,
            "WorldObjectSpawn packet must be queued in AoI broadcast jobs"
        );
    }

    #[test]
    fn test_despawn_entity_cleans_up_all_tracking_stores() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let mut sim = TickSimulation::new(1, bounds);

        let guid = Guid::new(mmo_core::EntityType::Player, 1, 55);
        sim.spawn_entity(guid, (0.0, 0.0, 0.0));
        sim.aoi
            .register_observer(100, guid, ObserverConfig::default());
        sim.last_migration_ticks.insert(guid, 10);
        sim.session_epochs.insert(guid, 3);
        sim.sectors.check_rate_limit(guid);

        assert!(sim.entity_store.contains_guid(guid));
        assert!(sim.aoi.get_observer(100).is_some());
        assert!(sim.session_epochs.contains_key(&guid));
        assert!(sim.last_migration_ticks.contains_key(&guid));

        // Despawn entity
        assert!(sim.despawn_entity(guid));

        assert!(!sim.entity_store.contains_guid(guid));
        assert!(
            sim.aoi.get_observer(100).is_none(),
            "AoI observer must be unregistered"
        );
        assert!(
            !sim.session_epochs.contains_key(&guid),
            "session epoch must be removed"
        );
        assert!(
            !sim.last_migration_ticks.contains_key(&guid),
            "last migration tick must be removed"
        );
    }

    #[test]
    fn test_handle_interact_object_hooks_and_veto() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let mut sim = TickSimulation::new(1, bounds);

        let player = Guid::new(mmo_core::EntityType::Player, 1, 10);
        sim.spawn_entity(player, (0.0, 0.0, 0.0));

        let obj_guid = Guid::new(mmo_core::EntityType::DynamicObject, 1, 200);
        let mut obj = mmo_core::world::WorldObject::new(
            obj_guid,
            1,
            mmo_core::world::Transform3D::new(
                (2.0, 0.0, 0.0),
                (0.0, 0.0, 0.0, 1.0),
                (1.0, 1.0, 1.0),
            ),
            BoundingBox3D::new((1.5, -0.5, -0.5), (2.5, 0.5, 0.5)),
            mmo_core::container::ContainerOwner::Character(10),
        );
        obj.state_flags |= mmo_core::world::world_object_flags::INTERACTIVE;
        sim.insert_world_object(obj);

        let pkt = mmo_schema::client::InteractObjectPacket {
            object_id: obj_guid.raw(),
            action_type: 1,
        };

        // Without veto, interaction succeeds
        assert!(sim.handle_interact_object(player, &pkt));

        // Register a veto hook
        sim.hooks.register_fn(|event, ctx| {
            if let mmo_core::Event::BeforeInteract { .. } = event {
                ctx.cancel("forbidden");
            }
        });

        // Now interaction is vetoed
        assert!(!sim.handle_interact_object(player, &pkt));
    }

    #[test]
    fn test_passenger_walking_on_fast_moving_ship_and_atomic_migration_group() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let mut sim = TickSimulation::new(1, bounds);
        sim.kinematics = KinematicValidator::new(7.0); // 7.0 m/s base speed cap

        let ship_guid = Guid::new(mmo_core::EntityType::DynamicObject, 1, 100);
        let passenger_guid = Guid::new(mmo_core::EntityType::Player, 1, 101);

        sim.spawn_entity(ship_guid, (0.0, 0.0, 0.0));
        sim.spawn_entity(passenger_guid, (0.0, 0.0, 0.0));

        let now = std::time::Instant::now();
        sim.kinematic_states.insert(
            ship_guid,
            EntityKinematicState::new(now).with_position((0.0, 0.0, 0.0)),
        );
        sim.kinematic_states.insert(
            passenger_guid,
            EntityKinematicState::new(now).with_position((0.0, 0.0, 0.0)),
        );

        // Attach passenger to ship at local position (1.0, 0.0, 0.0)
        assert!(sim
            .entity_store
            .attach_child(passenger_guid, ship_guid, (1.0, 0.0, 0.0), 0.0));
        sim.kinematic_states
            .get_mut(&passenger_guid)
            .unwrap()
            .last_position = (1.0, 0.0, 0.0);

        // Grant impulse allowance to the ship for high-speed travel (50 m/s)
        sim.kinematics.grant_impulse(
            sim.kinematic_states.get_mut(&ship_guid).unwrap(),
            50.0,
            10,
            sim.tick_counter,
        );

        // Ship advances 2.5m in a 50ms tick (50 m/s)
        let ship_new_pos = (2.5, 0.0, 0.0);
        let res_ship = sim.process_client_move_deterministic(ship_guid, ship_new_pos, 0.05);
        assert!(
            res_ship.is_ok(),
            "Ship movement within impulse allowance must pass"
        );

        // Hierarchy transform propagation should have automatically updated passenger world pos and kinematic last_position
        let passenger = sim.entity_store.get_by_guid(passenger_guid).unwrap();
        assert_eq!(passenger.position, (3.5, 0.0, 0.0)); // 2.5 + 1.0 = 3.5
        let passenger_k_state = sim.kinematic_states.get(&passenger_guid).unwrap();
        assert_eq!(passenger_k_state.last_position, (3.5, 0.0, 0.0));

        // Passenger walks locally 0.3m on deck in 50ms (from local 1.0 to 1.3 -> world 3.5 to 3.8)
        // Local speed: 0.3 / 0.05 = 6.0 m/s <= 7.0 m/s
        let passenger_new_world_pos = (3.8, 0.0, 0.0);
        let res_passenger =
            sim.process_client_move_deterministic(passenger_guid, passenger_new_world_pos, 0.05);
        assert!(
            res_passenger.is_ok(),
            "Passenger walking 0.3m locally on moving ship must pass kinematic validation"
        );

        let passenger = sim.entity_store.get_by_guid(passenger_guid).unwrap();
        assert!((passenger.local_position.0 - 1.3).abs() < 1e-4);

        // Boundary Migration: passenger must NEVER migrate independently
        assert!(
            !sim.check_boundary_migration(passenger_guid, (600.0, 0.0, 0.0), 1.0, 20),
            "Child entity must not trigger independent boundary migration"
        );

        // Ship crossing boundary triggers migration and collects atomic group [ship, passenger]
        assert!(
            sim.check_boundary_migration(ship_guid, (600.0, 0.0, 0.0), 1.0, 20),
            "Parent ship qualifies for boundary migration"
        );
        let group = sim.collect_migration_group(ship_guid);
        assert_eq!(group, vec![ship_guid, passenger_guid]);
    }
}
