use std::sync::Arc;

use mmo_core::guid::Guid;
use mmo_core::Event;
use mmo_schema::cluster::node_service_server::{NodeService, NodeServiceServer};
use mmo_schema::cluster::{
    AbortContainerTransferRequest, AbortContainerTransferResponse, BatchMigrateEntityRequest,
    BatchMigrateEntityResponse, BatchRemoteAbilityRequest, BatchRemoteAbilityResponse,
    CommitContainerTransferRequest, CommitContainerTransferResponse, ForwardClientPacketRequest,
    ForwardClientPacketResponse, GhostSyncRequest, GhostSyncResponse, MigrateEntityRequest,
    MigrateEntityResponse, RemoteAbilityErrorCode, RemoteAbilityRequest, RemoteAbilityResponse,
    RemoteCommandRequest, RemoteCommandResponse, ReserveContainerSlotRequest,
    ReserveContainerSlotResponse, TakeoverSessionRouteRequest, TakeoverSessionRouteResponse,
    Vector3,
};

use crate::network::{NetworkError, RemoteCommandErrorCode};
use crate::sim::{ContainerTransferNotification, TickSimulation};

/// Helper function extracting the initiator GUID from a [`RemoteCommandRequest`].
///
/// If payload contains at least 8 bytes, decodes the first 8 bytes as a little-endian GUID.
/// Returns an error if the payload is less than 8 bytes or decodes to an invalid (zero) GUID.
pub fn extract_initiator(req: &RemoteCommandRequest) -> Result<Guid, NetworkError> {
    if req.payload.len() < 8 {
        return Err(NetworkError::MalformedPayload(
            "Remote command payload missing initiator GUID (minimum 8 bytes required)".to_string(),
        ));
    }
    let raw = u64::from_le_bytes(
        req.payload[0..8]
            .try_into()
            .map_err(|_| NetworkError::MalformedPayload("Invalid payload slice".to_string()))?,
    );
    if raw == 0 {
        return Err(NetworkError::MalformedPayload(
            "Remote command initiator GUID cannot be zero".to_string(),
        ));
    }
    Ok(Guid::from_raw(raw))
}

/// Constructs a [`RemoteCommandRequest`] encoding the initiator GUID into the payload.
pub fn make_remote_command(
    initiator: Guid,
    target: Guid,
    action_id: u32,
    extra_payload: &[u8],
    source_node_id: u32,
    target_node_id: u32,
) -> RemoteCommandRequest {
    let mut payload = initiator.raw().to_le_bytes().to_vec();
    payload.extend_from_slice(extra_payload);
    RemoteCommandRequest {
        guid: target.raw(),
        action_id,
        payload,
        source_node_id,
        target_node_id,
    }
}

/// gRPC implementation of [`NodeService`] for inter-node command forwarding and ghost synchronization.
#[derive(Clone)]
pub struct NodeServiceImpl {
    pub sim: Arc<tokio::sync::RwLock<TickSimulation>>,
    pub transaction_coord: Option<Arc<mmo_storage::TransactionCoordinator>>,
    /// Bounded ingress queue into the simulation; lets packet forwarding avoid the `sim` lock.
    input_tx: tokio::sync::mpsc::Sender<crate::sim::QueuedClientInput>,
}

/// Grabs the simulation's ingress sender once at service construction.
fn ingress_sender(
    sim: &Arc<tokio::sync::RwLock<TickSimulation>>,
) -> tokio::sync::mpsc::Sender<crate::sim::QueuedClientInput> {
    sim.try_read()
        .expect("simulation must not be write-locked while constructing NodeServiceImpl")
        .input_sender()
}

impl NodeServiceImpl {
    /// Creates a new `NodeServiceImpl` sharing the simulation state.
    pub fn new(sim: Arc<tokio::sync::RwLock<TickSimulation>>) -> Self {
        let input_tx = ingress_sender(&sim);
        Self {
            sim,
            transaction_coord: None,
            input_tx,
        }
    }

    /// Creates a new `NodeServiceImpl` configured with a distributed transaction coordinator.
    pub fn with_coordinator(
        sim: Arc<tokio::sync::RwLock<TickSimulation>>,
        coord: Arc<mmo_storage::TransactionCoordinator>,
    ) -> Self {
        let input_tx = ingress_sender(&sim);
        Self {
            sim,
            transaction_coord: Some(coord),
            input_tx,
        }
    }

    /// Creates a Tonic gRPC [`NodeServiceServer`] wrapping this implementation.
    pub fn into_server(self) -> NodeServiceServer<Self> {
        NodeServiceServer::new(self)
    }

    /// Enqueues an incoming client ACK packet into the simulation node's non-blocking channel.
    pub async fn queue_client_ack(&self, ack: mmo_schema::client::ClientAckPacket) {
        let sim = self.sim.read().await;
        sim.queue_client_ack(ack);
    }
}

#[tonic::async_trait]
impl NodeService for NodeServiceImpl {
    /// Processes a forwarded command targeted at an entity residing on this authoritative node.
    async fn remote_command(
        &self,
        request: tonic::Request<RemoteCommandRequest>,
    ) -> Result<tonic::Response<RemoteCommandResponse>, tonic::Status> {
        let req = request.into_inner();
        let target = Guid::from_raw(req.guid);
        let initiator = match extract_initiator(&req) {
            Ok(init) => init,
            Err(e) => {
                return Ok(tonic::Response::new(RemoteCommandResponse {
                    success: false,
                    error_code: RemoteCommandErrorCode::MalformedPayload as u32,
                    error_message: e.to_string(),
                    result: Vec::new(),
                }));
            }
        };

        let sim = self.sim.read().await;

        // 1. Target entity validation
        if !sim.entity_store.contains_guid(target) {
            return Ok(tonic::Response::new(RemoteCommandResponse {
                success: false,
                error_code: RemoteCommandErrorCode::EntityNotFound as u32,
                error_message: format!(
                    "Target entity {} not found on node {}",
                    target, sim.node_id
                ),
                result: Vec::new(),
            }));
        }

        // 1.5. Migration lease check (strict freeze)
        if sim.is_migrating(target) {
            return Ok(tonic::Response::new(RemoteCommandResponse {
                success: false,
                error_code: RemoteCommandErrorCode::Vetoed as u32,
                error_message: format!("Target entity {} is currently migrating", target),
                result: Vec::new(),
            }));
        }

        // 2. Dispatch BeforeInteract hook (check for vetoes)
        let before_ctx = sim.hooks.dispatch(&Event::BeforeInteract {
            initiator,
            target,
            action_id: req.action_id,
        });

        if before_ctx.is_canceled() {
            let reason = before_ctx
                .cancel_reason()
                .unwrap_or("Interaction vetoed by hook")
                .to_string();

            return Ok(tonic::Response::new(RemoteCommandResponse {
                success: false,
                error_code: RemoteCommandErrorCode::Vetoed as u32,
                error_message: reason,
                result: Vec::new(),
            }));
        }

        // 3. Dispatch OnInteract hook upon successful execution
        sim.hooks.dispatch(&Event::OnInteract {
            initiator,
            target,
            action_id: req.action_id,
        });

        Ok(tonic::Response::new(RemoteCommandResponse {
            success: true,
            error_code: RemoteCommandErrorCode::Success as u32,
            error_message: String::new(),
            result: Vec::new(),
        }))
    }

    /// Synchronizes boundary ghost replicas received from a neighboring simulation node.
    async fn sync_ghosts(
        &self,
        request: tonic::Request<GhostSyncRequest>,
    ) -> Result<tonic::Response<GhostSyncResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut sim = self.sim.write().await;
        let synced_count = sim.ghost_store.update_from_request(&req);

        Ok(tonic::Response::new(GhostSyncResponse {
            acknowledged: true,
            synced_count: synced_count as u32,
        }))
    }

    /// Receives and adopts an entity migrated live from a donor simulation node.
    async fn migrate_entity(
        &self,
        request: tonic::Request<MigrateEntityRequest>,
    ) -> Result<tonic::Response<MigrateEntityResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut sim = self.sim.write().await;
        match crate::network::migration::ingest_migrated_entity(&mut sim, &req) {
            Ok(()) => {
                metrics::counter!("mmo_migration_inbound_total", "status" => "success")
                    .increment(1);
                Ok(tonic::Response::new(MigrateEntityResponse {
                    success: true,
                    error_code: 0,
                    error_message: String::new(),
                    acknowledged_guid: req.guid,
                }))
            }
            Err((code, msg)) => Ok(tonic::Response::new(MigrateEntityResponse {
                success: false,
                error_code: code as u32,
                error_message: msg,
                acknowledged_guid: req.guid,
            })),
        }
    }

    /// Receives and adopts a batch of entities migrated live from a donor simulation node.
    /// Strictly all-or-nothing: validates all entities within recipient partition bounds before inserting.
    async fn migrate_batch_entities(
        &self,
        request: tonic::Request<BatchMigrateEntityRequest>,
    ) -> Result<tonic::Response<BatchMigrateEntityResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut sim = self.sim.write().await;

        // 1. Validation pass: check all entities have valid positions inside recipient bounds
        for entity_req in &req.entities {
            let target_pos = match entity_req.position {
                Some(ref p) => (p.x, p.y, p.z),
                None => {
                    return Ok(tonic::Response::new(BatchMigrateEntityResponse {
                        success: false,
                        migrated_count: 0,
                        error_message: format!(
                            "Entity {} missing position in batch",
                            entity_req.guid
                        ),
                    }));
                }
            };

            if !sim.octree.bounds.contains(target_pos) {
                return Ok(tonic::Response::new(BatchMigrateEntityResponse {
                    success: false,
                    migrated_count: 0,
                    error_message: format!(
                        "Entity {} position {:?} outside recipient node {} bounds {:?}",
                        entity_req.guid, target_pos, sim.node_id, sim.octree.bounds
                    ),
                }));
            }
        }

        // 2. Ingestion pass: insert all entities
        let mut migrated_count = 0;
        for entity_req in &req.entities {
            if let Err((_code, msg)) =
                crate::network::migration::ingest_migrated_entity(&mut sim, entity_req)
            {
                return Ok(tonic::Response::new(BatchMigrateEntityResponse {
                    success: false,
                    migrated_count,
                    error_message: msg,
                }));
            }
            migrated_count += 1;
        }

        metrics::counter!("mmo_batch_migrations_inbound_total", "status" => "success").increment(1);
        metrics::counter!("mmo_migration_inbound_total", "status" => "success")
            .increment(migrated_count as u64);

        Ok(tonic::Response::new(BatchMigrateEntityResponse {
            success: true,
            migrated_count,
            error_message: String::new(),
        }))
    }

    /// Dispatches an authoritative remote ability against an entity on this simulation node.
    async fn dispatch_ability(
        &self,
        request: tonic::Request<RemoteAbilityRequest>,
    ) -> Result<tonic::Response<RemoteAbilityResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut sim = self.sim.write().await;
        let response = execute_remote_ability(
            &mut sim,
            Guid::from_raw(req.caster_guid),
            Guid::from_raw(req.target_guid),
            req.caster_position.as_ref(),
            req.max_range,
            req.damage,
            req.healing,
            req.cast_id,
        );
        Ok(tonic::Response::new(response))
    }

    /// Dispatches batched authoritative remote abilities against multiple entities on this node.
    async fn dispatch_batch_ability(
        &self,
        request: tonic::Request<BatchRemoteAbilityRequest>,
    ) -> Result<tonic::Response<BatchRemoteAbilityResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut sim = self.sim.write().await;
        let caster = Guid::from_raw(req.caster_guid);
        let mut results = Vec::with_capacity(req.target_guids.len());

        for &target_raw in &req.target_guids {
            let res = execute_remote_ability(
                &mut sim,
                caster,
                Guid::from_raw(target_raw),
                req.caster_position.as_ref(),
                req.max_range,
                req.damage_per_target,
                req.healing_per_target,
                req.cast_id,
            );
            results.push(res);
        }

        Ok(tonic::Response::new(BatchRemoteAbilityResponse {
            success: true,
            results,
            cast_id: req.cast_id,
        }))
    }

    /// Reserves a container slot on this node for cross-node two-phase transfers.
    async fn reserve_container_slot(
        &self,
        request: tonic::Request<ReserveContainerSlotRequest>,
    ) -> Result<tonic::Response<ReserveContainerSlotResponse>, tonic::Status> {
        let req = request.into_inner();
        let coord = match &self.transaction_coord {
            Some(c) => c,
            None => {
                return Ok(tonic::Response::new(ReserveContainerSlotResponse {
                    success: false,
                    error_message: "Transaction coordinator not configured on this node"
                        .to_string(),
                    reservation_id: String::new(),
                    expires_at_epoch_ms: 0,
                }));
            }
        };

        if req.slot < 0 {
            return Ok(tonic::Response::new(ReserveContainerSlotResponse {
                success: false,
                error_message: format!("Negative slot index invalid: {}", req.slot),
                reservation_id: String::new(),
                expires_at_epoch_ms: 0,
            }));
        }

        let item_opt = if req.item_instance_id.is_empty() {
            None
        } else {
            Some(req.item_instance_id.as_str())
        };

        match coord
            .reserve_slot(
                &req.tx_id,
                req.container_id,
                req.slot as u32,
                item_opt,
                &req.reservation_type,
                req.lease_duration_ms,
            )
            .await
        {
            Ok(res) => Ok(tonic::Response::new(ReserveContainerSlotResponse {
                success: true,
                error_message: String::new(),
                reservation_id: res.reservation_id,
                expires_at_epoch_ms: res.expires_at_epoch_ms as u64,
            })),
            Err(e) => Ok(tonic::Response::new(ReserveContainerSlotResponse {
                success: false,
                error_message: e.to_string(),
                reservation_id: String::new(),
                expires_at_epoch_ms: 0,
            })),
        }
    }

    /// Commits an active two-phase container transfer atomically.
    async fn commit_container_transfer(
        &self,
        request: tonic::Request<CommitContainerTransferRequest>,
    ) -> Result<tonic::Response<CommitContainerTransferResponse>, tonic::Status> {
        let req = request.into_inner();
        let coord = match &self.transaction_coord {
            Some(c) => c,
            None => {
                return Ok(tonic::Response::new(CommitContainerTransferResponse {
                    success: false,
                    error_message: "Transaction coordinator not configured on this node"
                        .to_string(),
                    src_container_id: 0,
                    dst_container_id: 0,
                    item_instance_id: String::new(),
                }));
            }
        };

        match coord.commit_transfer(&req.tx_id).await {
            Ok(tx_rec) => {
                let sim = self.sim.read().await;
                sim.queue_container_transfer(ContainerTransferNotification {
                    tx_id: tx_rec.tx_id.clone(),
                    src_container_id: tx_rec.src_container_id,
                    dst_container_id: tx_rec.dst_container_id,
                    item_instance_id: tx_rec.item_instance_id.clone(),
                    success: true,
                    error_message: None,
                });

                Ok(tonic::Response::new(CommitContainerTransferResponse {
                    success: true,
                    error_message: String::new(),
                    src_container_id: tx_rec.src_container_id,
                    dst_container_id: tx_rec.dst_container_id,
                    item_instance_id: tx_rec.item_instance_id,
                }))
            }
            Err(e) => {
                let sim = self.sim.read().await;
                sim.queue_container_transfer(ContainerTransferNotification {
                    tx_id: req.tx_id.clone(),
                    src_container_id: 0,
                    dst_container_id: 0,
                    item_instance_id: String::new(),
                    success: false,
                    error_message: Some(e.to_string()),
                });

                Ok(tonic::Response::new(CommitContainerTransferResponse {
                    success: false,
                    error_message: e.to_string(),
                    src_container_id: 0,
                    dst_container_id: 0,
                    item_instance_id: String::new(),
                }))
            }
        }
    }

    /// Aborts an active two-phase container transfer and rolls back reservations.
    async fn abort_container_transfer(
        &self,
        request: tonic::Request<AbortContainerTransferRequest>,
    ) -> Result<tonic::Response<AbortContainerTransferResponse>, tonic::Status> {
        let req = request.into_inner();
        let coord = match &self.transaction_coord {
            Some(c) => c,
            None => {
                return Ok(tonic::Response::new(AbortContainerTransferResponse {
                    success: false,
                    error_message: "Transaction coordinator not configured on this node"
                        .to_string(),
                }));
            }
        };

        match coord.abort_transfer(&req.tx_id).await {
            Ok(_) => {
                let sim = self.sim.read().await;
                sim.queue_container_transfer(ContainerTransferNotification {
                    tx_id: req.tx_id,
                    src_container_id: 0,
                    dst_container_id: 0,
                    item_instance_id: String::new(),
                    success: false,
                    error_message: Some("Transaction aborted".to_string()),
                });

                Ok(tonic::Response::new(AbortContainerTransferResponse {
                    success: true,
                    error_message: String::new(),
                }))
            }
            Err(e) => Ok(tonic::Response::new(AbortContainerTransferResponse {
                success: false,
                error_message: e.to_string(),
            })),
        }
    }

    /// Takes over an entity's downstream session route to a new edge gateway.
    async fn takeover_session_route(
        &self,
        request: tonic::Request<TakeoverSessionRouteRequest>,
    ) -> Result<tonic::Response<TakeoverSessionRouteResponse>, tonic::Status> {
        let req = request.into_inner();
        let guid = Guid::from_raw(req.guid);
        let mut sim = self.sim.write().await;

        if !sim.entity_store.contains_guid(guid) {
            return Ok(tonic::Response::new(TakeoverSessionRouteResponse {
                success: false,
                error_message: format!("Entity {} not found on node {}", guid, sim.node_id),
                authoritative_epoch: 0,
            }));
        }

        let current_epoch = *sim.session_epochs.get(&guid).unwrap_or(&0);
        if req.session_epoch <= current_epoch {
            return Ok(tonic::Response::new(TakeoverSessionRouteResponse {
                success: false,
                error_message: format!(
                    "Stale session epoch {} <= authoritative epoch {}",
                    req.session_epoch, current_epoch
                ),
                authoritative_epoch: current_epoch,
            }));
        }

        // Update authoritative epoch
        sim.session_epochs.insert(guid, req.session_epoch);

        // Register new observer session if provided
        if req.new_session_id != 0 {
            let config = crate::aoi::ObserverConfig::default();
            sim.aoi.register_observer(req.new_session_id, guid, config);
        }

        metrics::counter!("mmo_node_session_takeovers_total").increment(1);

        Ok(tonic::Response::new(TakeoverSessionRouteResponse {
            success: true,
            error_message: String::new(),
            authoritative_epoch: req.session_epoch,
        }))
    }

    /// Receives and authoritatively executes an incoming forwarded client packet.
    async fn forward_client_packet(
        &self,
        request: tonic::Request<ForwardClientPacketRequest>,
    ) -> Result<tonic::Response<ForwardClientPacketResponse>, tonic::Status> {
        let req = request.into_inner();
        let max_size =
            mmo_schema::client::MAX_PACKET_PAYLOAD_SIZE + mmo_schema::client::HEADER_SIZE;
        if req.packet_data.len() > max_size {
            return Ok(tonic::Response::new(ForwardClientPacketResponse {
                success: false,
                error_message: format!(
                    "Packet size {} exceeds maximum allowed {}",
                    req.packet_data.len(),
                    max_size
                ),
            }));
        }

        let mut data = bytes::BytesMut::from(&req.packet_data[..]);
        match mmo_schema::client::ClientPacket::decode_frame(&mut data) {
            Ok(Some(packet)) => {
                use tracing::Instrument;
                let opcode = packet.opcode().as_str();
                let span = tracing::info_span!(
                    "node_packet_process",
                    session_id = req.session_id,
                    opcode = opcode
                );
                async move {
                    // Lock-free fast path: epoch fencing, hydration, anti-spoofing and plugin
                    // hook dispatch are all performed by `TickSimulation::drain_client_inputs`.
                    let queued = crate::sim::QueuedClientInput {
                        session_id: req.session_id,
                        entity_guid: req.guid,
                        session_epoch: req.session_epoch,
                        packet,
                    };
                    match crate::sim::try_enqueue_client_input(&self.input_tx, queued) {
                        Ok(()) => Ok(tonic::Response::new(ForwardClientPacketResponse {
                            success: true,
                            error_message: String::new(),
                        })),
                        Err(e) => Ok(tonic::Response::new(ForwardClientPacketResponse {
                            success: false,
                            error_message: match e {
                                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                    "Simulation ingress queue full".to_string()
                                }
                                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                                    "Simulation ingress queue closed".to_string()
                                }
                            },
                        })),
                    }
                }
                .instrument(span)
                .await
            }
            Ok(None) => Ok(tonic::Response::new(ForwardClientPacketResponse {
                success: false,
                error_message: "Incomplete frame received".to_string(),
            })),
            Err(e) => {
                tracing::error!(error = %e, "Failed to decode forwarded client packet");
                Ok(tonic::Response::new(ForwardClientPacketResponse {
                    success: false,
                    error_message: e.to_string(),
                }))
            }
        }
    }
}

/// Authoritatively executes an incoming remote ability request on a target entity.
#[allow(clippy::too_many_arguments)]
pub fn execute_remote_ability(
    sim: &mut TickSimulation,
    caster_guid: Guid,
    target_guid: Guid,
    caster_position: Option<&Vector3>,
    max_range: f32,
    damage: u32,
    healing: u32,
    cast_id: u64,
) -> RemoteAbilityResponse {
    // 1. Target entity validation
    if !sim.entity_store.contains_guid(target_guid) {
        return RemoteAbilityResponse {
            success: false,
            error_code: RemoteAbilityErrorCode::RemoteAbilityTargetNotFound as i32,
            error_message: format!(
                "Target entity {} not found on node {}",
                target_guid, sim.node_id
            ),
            target_guid: target_guid.raw(),
            current_health: 0,
            cast_id,
        };
    }

    // 2. Migration lease check (strict freeze)
    if sim.is_migrating(target_guid) {
        return RemoteAbilityResponse {
            success: false,
            error_code: RemoteAbilityErrorCode::RemoteAbilityTargetMigrating as i32,
            error_message: format!("Target entity {} is currently migrating", target_guid),
            target_guid: target_guid.raw(),
            current_health: 0,
            cast_id,
        };
    }

    let entity = match sim.entity_store.get_mut_by_guid(target_guid) {
        Some(e) => e,
        None => {
            return RemoteAbilityResponse {
                success: false,
                error_code: RemoteAbilityErrorCode::RemoteAbilityTargetNotFound as i32,
                error_message: format!("Target entity {} not found in store", target_guid),
                target_guid: target_guid.raw(),
                current_health: 0,
                cast_id,
            };
        }
    };

    // 3. Authoritative Range Re-Validation (Anti-Cheat / Anti-Stale Ghost)
    if let Some(pos) = caster_position {
        if !pos.x.is_finite() || !pos.y.is_finite() || !pos.z.is_finite() {
            return RemoteAbilityResponse {
                success: false,
                error_code: RemoteAbilityErrorCode::RemoteAbilityOutOfRange as i32,
                error_message: "Caster position coordinates must be finite".to_string(),
                target_guid: target_guid.raw(),
                current_health: 0,
                cast_id,
            };
        }
        let caster_pos = mmo_core::Vec3::new(pos.x, pos.y, pos.z);
        let target_pos =
            mmo_core::Vec3::new(entity.position.0, entity.position.1, entity.position.2);
        let dist = caster_pos.distance(&target_pos);
        let max_allowed = max_range * 1.2;
        if max_range > 0.0 && dist > max_allowed {
            return RemoteAbilityResponse {
                success: false,
                error_code: RemoteAbilityErrorCode::RemoteAbilityOutOfRange as i32,
                error_message: format!(
                    "Target is out of range: {:.2} > max allowed {:.2}",
                    dist, max_allowed
                ),
                target_guid: target_guid.raw(),
                current_health: 0,
                cast_id,
            };
        }
    }

    // 4. Alive check & Apply effects via unified combat pipeline
    let outcome = match crate::combat_pipeline::apply_combat_effect_with_resolver(
        sim.combat_resolver.as_ref(),
        entity,
        target_guid,
        Some(caster_guid),
        damage,
        healing,
        &mut sim.event_queue,
    ) {
        Ok(out) => out,
        Err(err) => {
            return RemoteAbilityResponse {
                success: false,
                error_code: RemoteAbilityErrorCode::RemoteAbilityTargetDead as i32,
                error_message: format!("Target entity {} is dead: {}", target_guid, err),
                target_guid: target_guid.raw(),
                current_health: 0,
                cast_id,
            };
        }
    };

    RemoteAbilityResponse {
        success: true,
        error_code: RemoteAbilityErrorCode::RemoteAbilitySuccess as i32,
        error_message: String::new(),
        target_guid: target_guid.raw(),
        current_health: outcome.current_health,
        cast_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use mmo_schema::cluster::RemoteCommandRequest;

    #[test]
    fn test_extract_initiator_happy_path_exact_size() {
        let guid = Guid::new(EntityType::Player, 1, 12345);
        let payload = guid.raw().to_le_bytes().to_vec();

        let req = RemoteCommandRequest {
            guid: 0,
            action_id: 0,
            payload,
            source_node_id: 0,
            target_node_id: 0,
        };

        let result = extract_initiator(&req);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), guid);
    }

    #[test]
    fn test_extract_initiator_happy_path_extra_data() {
        let guid = Guid::new(EntityType::Npc, 2, 9876);
        let mut payload = guid.raw().to_le_bytes().to_vec();
        payload.extend_from_slice(&[1, 2, 3, 4, 5]); // extra data

        let req = RemoteCommandRequest {
            guid: 0,
            action_id: 0,
            payload,
            source_node_id: 0,
            target_node_id: 0,
        };

        let result = extract_initiator(&req);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), guid);
    }

    #[test]
    fn test_extract_initiator_error_too_small() {
        let payload = vec![1, 2, 3, 4, 5, 6, 7]; // 7 bytes

        let req = RemoteCommandRequest {
            guid: 0,
            action_id: 0,
            payload,
            source_node_id: 0,
            target_node_id: 0,
        };

        let result = extract_initiator(&req);
        assert!(result.is_err());
        if let Err(NetworkError::MalformedPayload(msg)) = result {
            assert!(msg.contains("minimum 8 bytes required"));
        } else {
            panic!("Expected MalformedPayload error");
        }
    }

    #[test]
    fn test_extract_initiator_error_zero_guid() {
        let payload = vec![0, 0, 0, 0, 0, 0, 0, 0]; // 8 bytes of zeros

        let req = RemoteCommandRequest {
            guid: 0,
            action_id: 0,
            payload,
            source_node_id: 0,
            target_node_id: 0,
        };

        let result = extract_initiator(&req);
        assert!(result.is_err());
        if let Err(NetworkError::MalformedPayload(msg)) = result {
            assert!(msg.contains("cannot be zero"));
        } else {
            panic!("Expected MalformedPayload error");
        }
    }

    #[test]
    fn test_extract_initiator_roundtrip() {
        let initiator = Guid::new(EntityType::Player, 1, 42);
        let target = Guid::new(EntityType::Npc, 1, 100);
        let action_id = 99;
        let extra_payload = vec![10, 20, 30];
        let source_node_id = 1;
        let target_node_id = 2;

        let req = make_remote_command(
            initiator,
            target,
            action_id,
            &extra_payload,
            source_node_id,
            target_node_id,
        );

        let result = extract_initiator(&req);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), initiator);
    }
}
