use std::sync::Arc;

use mmo_core::Guid;
use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::{
    BatchMigrateEntityRequest, BatchRouteUpdateRequest, MigrateEntityRequest, RouteUpdateRequest,
    SerializedComponent, Vector3,
};
use thiserror::Error;

use crate::network::client_pool::GrpcClientPool;
use crate::sim::TickSimulation;

/// Errors that can occur during live inter-node entity migration.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MigrationError {
    #[error("Failed to acquire migration lease for GUID {0} (already migrating or not found)")]
    LeaseConflict(Guid),

    #[error("Entity {0} not found in entity store")]
    EntityNotFound(Guid),

    #[error("Connection to recipient node failed: {0}")]
    ConnectionFailed(String),

    #[error("Migration RPC failed: {0}")]
    RpcFailed(String),

    #[error("Recipient rejected migration: {0}")]
    RecipientRejected(String),

    #[error("Missing required 3D position vector for GUID {0}")]
    MissingPosition(Guid),
}

/// Consolidated outcome report for a completed or aborted batch entity migration.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchMigrationReport {
    pub success: bool,
    pub total_requested: usize,
    pub migrated_count: usize,
    pub error_message: String,
}

/// Reconstructs and ingests a single entity into the simulation state on the recipient node.
/// Shared by both single-entity and batch-entity migration handlers in `NodeServiceImpl`.
pub fn ingest_migrated_entity(
    sim: &mut TickSimulation,
    req: &MigrateEntityRequest,
) -> Result<(), (i32, String)> {
    let target_pos = match req.position {
        Some(ref p) => (p.x, p.y, p.z),
        None => {
            return Err((
                1,
                "Missing position coordinates in MigrateEntityRequest".to_string(),
            ))
        }
    };

    if !sim.octree.bounds.contains(target_pos) {
        return Err((
            2,
            format!(
                "Position {:?} is outside recipient node {} bounds {:?}",
                target_pos, sim.node_id, sim.octree.bounds
            ),
        ));
    }

    let guid = Guid::from_raw(req.guid);

    // 1. Remove from ghost store if previously mirrored as a ghost
    sim.ghost_store.remove(guid);

    // 2. Insert entity into EntityStore and Octree
    sim.entity_store.spawn(guid, target_pos);
    sim.octree.insert(guid, target_pos);

    // 3. Restore serialized components
    let codecs = sim.codecs.clone();
    for c in &req.components {
        if c.component_id == 0 {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&c.data) {
                if let Some(entity) = sim.entity_store.get_mut_by_guid(guid) {
                    let _ = codecs.deserialize_into(&val, &mut entity.components);
                }
            }
        }
    }

    // 4. Restore kinematic tracking state with preserved sequence
    let mut kin_state = crate::kinematics::EntityKinematicState::new(sim.current_time);
    kin_state.initialized = true;
    kin_state.last_sequence = req.last_sequence;
    sim.kinematic_states.insert(guid, kin_state);

    // 5. Restore cooldown tick
    if req.last_migration_tick > 0 {
        sim.set_last_migration_tick(guid, req.last_migration_tick);
    }

    // 6. Register AoI observer if an active client session was attached
    if req.session_id != 0 {
        sim.register_aoi_observer(req.session_id, guid, crate::aoi::ObserverConfig::default());
    }

    Ok(())
}

/// Executes an authoritative two-phase entity migration for a single player or NPC using a pooled client connection.
pub async fn execute_entity_migration_pooled(
    pool: &GrpcClientPool,
    sim: Arc<tokio::sync::RwLock<TickSimulation>>,
    guid: Guid,
    target_node_id: u32,
    target_node_addr: &str,
    gateway_addr: Option<&str>,
) -> Result<bool, MigrationError> {
    let (req, session_id) = {
        let mut sim_guard = sim.write().await;
        if !sim_guard.acquire_migration_lease(guid) {
            return Err(MigrationError::LeaseConflict(guid));
        }
        let entity = match sim_guard.entity_store.get_by_guid(guid) {
            Some(e) => e,
            None => {
                sim_guard.release_migration_lease(guid);
                return Err(MigrationError::EntityNotFound(guid));
            }
        };
        let pos = entity.position;
        let last_seq = sim_guard
            .kinematic_states
            .get(&guid)
            .map(|s| s.last_sequence)
            .unwrap_or(0);
        let last_mig_tick = sim_guard.tick_counter;

        let comp_json = sim_guard.codecs.serialize_all(&entity.components);
        let comp_bytes = serde_json::to_vec(&comp_json).unwrap_or_default();
        let components = vec![SerializedComponent {
            component_id: 0,
            data: comp_bytes,
        }];

        let session_id = sim_guard.aoi.get_session_by_guid(guid).unwrap_or(0);

        let req = MigrateEntityRequest {
            guid: guid.raw(),
            source_node_id: sim_guard.node_id as u32,
            target_node_id,
            position: Some(Vector3 {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            }),
            velocity: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            yaw: 0.0,
            pitch: 0.0,
            last_sequence: last_seq,
            session_id,
            components,
            topology_epoch: 1,
            last_migration_tick: last_mig_tick,
        };
        (req, session_id)
    };

    let channel = match pool.get_connected_channel(target_node_addr).await {
        Ok(c) => c,
        Err(e) => {
            let mut sim_guard = sim.write().await;
            sim_guard.release_migration_lease(guid);
            metrics::counter!("mmo_migration_rollbacks_total").increment(1);
            return Err(MigrationError::ConnectionFailed(e.to_string()));
        }
    };

    let mut client = NodeServiceClient::new(channel);

    let resp = match client.migrate_entity(req).await {
        Ok(r) => r.into_inner(),
        Err(e) => {
            pool.invalidate(target_node_addr);
            let mut sim_guard = sim.write().await;
            sim_guard.release_migration_lease(guid);
            metrics::counter!("mmo_migration_rollbacks_total").increment(1);
            return Err(MigrationError::RpcFailed(e.to_string()));
        }
    };

    if !resp.success {
        let mut sim_guard = sim.write().await;
        sim_guard.release_migration_lease(guid);
        metrics::counter!("mmo_migration_rollbacks_total").increment(1);
        return Err(MigrationError::RecipientRejected(resp.error_message));
    }

    // Step 3: Gateway commit
    if let Some(gw_addr) = gateway_addr {
        if let Ok(gw_channel) = pool.get_connected_channel(gw_addr).await {
            let mut gw_client = GatewayServiceClient::new(gw_channel);
            let _ = gw_client
                .update_route(RouteUpdateRequest {
                    guid: guid.raw(),
                    session_id,
                    node_address: target_node_addr.to_string(),
                })
                .await;
        }
    }

    // Step 4: Despawn local entity cleanly
    {
        let mut sim_guard = sim.write().await;
        sim_guard.release_migration_lease(guid);
        sim_guard.kinematic_states.remove(&guid);
        sim_guard.last_migration_ticks.remove(&guid);
        sim_guard.session_epochs.remove(&guid);
        sim_guard.aoi.unregister_by_guid(guid);
        sim_guard.sectors.clear_rate_limit(guid);
        sim_guard.instances.remove_entity(guid);
        if session_id != 0 {
            sim_guard.unregister_aoi_observer(session_id);
        }
        if let Some(entity) = sim_guard.entity_store.despawn_by_guid(guid) {
            if !sim_guard.octree.remove(guid, entity.position) {
                sim_guard.octree.remove_by_guid(guid);
            }
        }
    }

    metrics::counter!("mmo_migration_outbound_total", "status" => "success").increment(1);
    Ok(true)
}

/// Executes an authoritative two-phase entity migration for a single player or NPC.
pub async fn execute_entity_migration(
    sim: Arc<tokio::sync::RwLock<TickSimulation>>,
    guid: Guid,
    target_node_id: u32,
    target_node_addr: &str,
    gateway_addr: Option<&str>,
) -> Result<bool, MigrationError> {
    execute_entity_migration_pooled(
        &GrpcClientPool::default(),
        sim,
        guid,
        target_node_id,
        target_node_addr,
        gateway_addr,
    )
    .await
}

/// Executes an authoritative two-phase batch entity migration for up to 500 entities using a pooled client connection.
pub async fn execute_batch_entity_migration_pooled(
    pool: &GrpcClientPool,
    sim: Arc<tokio::sync::RwLock<TickSimulation>>,
    guids: &[Guid],
    target_node_id: u32,
    target_node_addr: &str,
    gateway_addr: Option<&str>,
) -> Result<BatchMigrationReport, MigrationError> {
    if guids.is_empty() {
        return Ok(BatchMigrationReport {
            success: true,
            total_requested: 0,
            migrated_count: 0,
            error_message: String::new(),
        });
    }

    let (batch_req, session_updates) = {
        let mut sim_guard = sim.write().await;

        // Atomic all-or-nothing lease acquisition pass
        let mut acquired = Vec::with_capacity(guids.len());
        for &guid in guids {
            if !sim_guard.acquire_migration_lease(guid) {
                for &acq in &acquired {
                    sim_guard.release_migration_lease(acq);
                }
                return Err(MigrationError::LeaseConflict(guid));
            }
            acquired.push(guid);
        }

        let mut entity_requests = Vec::with_capacity(guids.len());
        let mut session_routes = Vec::new();
        let last_mig_tick = sim_guard.tick_counter;
        let source_node_id = sim_guard.node_id as u32;

        for &guid in guids {
            let entity = match sim_guard.entity_store.get_by_guid(guid) {
                Some(e) => e,
                None => {
                    for &acq in &acquired {
                        sim_guard.release_migration_lease(acq);
                    }
                    return Err(MigrationError::EntityNotFound(guid));
                }
            };

            let pos = entity.position;
            let last_seq = sim_guard
                .kinematic_states
                .get(&guid)
                .map(|s| s.last_sequence)
                .unwrap_or(0);

            let comp_json = sim_guard.codecs.serialize_all(&entity.components);
            let comp_bytes = serde_json::to_vec(&comp_json).unwrap_or_default();
            let components = vec![SerializedComponent {
                component_id: 0,
                data: comp_bytes,
            }];

            let session_id = sim_guard.aoi.get_session_by_guid(guid).unwrap_or(0);
            if session_id != 0 {
                session_routes.push((guid, session_id));
            }

            entity_requests.push(MigrateEntityRequest {
                guid: guid.raw(),
                source_node_id,
                target_node_id,
                position: Some(Vector3 {
                    x: pos.0,
                    y: pos.1,
                    z: pos.2,
                }),
                velocity: Some(Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
                yaw: 0.0,
                pitch: 0.0,
                last_sequence: last_seq,
                session_id,
                components,
                topology_epoch: 1,
                last_migration_tick: last_mig_tick,
            });
        }

        let batch_req = BatchMigrateEntityRequest {
            source_node_id,
            target_node_id,
            topology_epoch: 1,
            entities: entity_requests,
        };

        (batch_req, session_routes)
    };

    let channel = match pool.get_connected_channel(target_node_addr).await {
        Ok(c) => c,
        Err(e) => {
            let mut sim_guard = sim.write().await;
            for &g in guids {
                sim_guard.release_migration_lease(g);
            }
            metrics::counter!("mmo_batch_migration_rollbacks_total").increment(1);
            return Err(MigrationError::ConnectionFailed(e.to_string()));
        }
    };

    let mut client = NodeServiceClient::new(channel);

    let resp = match client.migrate_batch_entities(batch_req).await {
        Ok(r) => r.into_inner(),
        Err(e) => {
            pool.invalidate(target_node_addr);
            let mut sim_guard = sim.write().await;
            for &g in guids {
                sim_guard.release_migration_lease(g);
            }
            metrics::counter!("mmo_batch_migration_rollbacks_total").increment(1);
            return Err(MigrationError::RpcFailed(e.to_string()));
        }
    };

    if !resp.success {
        let mut sim_guard = sim.write().await;
        for &g in guids {
            sim_guard.release_migration_lease(g);
        }
        metrics::counter!("mmo_batch_migration_rollbacks_total").increment(1);
        return Err(MigrationError::RecipientRejected(resp.error_message));
    }

    // POINT OF NO RETURN: Recipient successfully ingested entities.
    {
        let mut sim_guard = sim.write().await;
        for &guid in guids {
            sim_guard.release_migration_lease(guid);
            sim_guard.kinematic_states.remove(&guid);
            sim_guard.last_migration_ticks.remove(&guid);
            sim_guard.session_epochs.remove(&guid);
            sim_guard.aoi.unregister_by_guid(guid);
            sim_guard.sectors.clear_rate_limit(guid);
            sim_guard.instances.remove_entity(guid);
            if let Some((_, sid)) = session_updates.iter().find(|(g, _)| *g == guid) {
                if *sid != 0 {
                    sim_guard.unregister_aoi_observer(*sid);
                }
            }
            if let Some(entity) = sim_guard.entity_store.despawn_by_guid(guid) {
                if !sim_guard.octree.remove(guid, entity.position) {
                    sim_guard.octree.remove_by_guid(guid);
                }
            }
            metrics::counter!("mmo_migration_outbound_total", "status" => "success").increment(1);
        }
        metrics::counter!("mmo_batch_migrations_outbound_total", "status" => "success")
            .increment(1);
    }

    // Step 4: Dispatch gateway batch route updates post-ingestion
    if let Some(gw_addr) = gateway_addr {
        if let Ok(gw_channel) = pool.get_connected_channel(gw_addr).await {
            let mut gw_client = GatewayServiceClient::new(gw_channel);
            let updates = session_updates
                .into_iter()
                .map(|(guid, session_id)| RouteUpdateRequest {
                    guid: guid.raw(),
                    session_id,
                    node_address: target_node_addr.to_string(),
                })
                .collect();
            let _ = gw_client
                .update_batch_routes(BatchRouteUpdateRequest { updates })
                .await;
        }
    }

    Ok(BatchMigrationReport {
        success: true,
        total_requested: guids.len(),
        migrated_count: resp.migrated_count as usize,
        error_message: String::new(),
    })
}

/// Executes an authoritative two-phase batch entity migration for up to 500 entities.
pub async fn execute_batch_entity_migration(
    sim: Arc<tokio::sync::RwLock<TickSimulation>>,
    guids: &[Guid],
    target_node_id: u32,
    target_node_addr: &str,
    gateway_addr: Option<&str>,
) -> Result<BatchMigrationReport, MigrationError> {
    execute_batch_entity_migration_pooled(
        &GrpcClientPool::default(),
        sim,
        guids,
        target_node_id,
        target_node_addr,
        gateway_addr,
    )
    .await
}
