//! Inter-node network services: Remote Command Forwarding, Ghost Broadcaster, Entity Migration, and Client Connection Pooling.

pub mod client_pool;
pub mod ghost;
pub mod migration;
pub mod service;

pub use client_pool::*;
pub use ghost::*;
pub use migration::*;
pub use service::*;

pub use crate::sim::{ContainerTransferNotification, TickSimulation};
pub use mmo_core::guid::Guid;
pub use mmo_core::{BoundingBox3D, EntityStore, Event, Octree};
pub use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
pub use mmo_schema::cluster::node_service_client::NodeServiceClient;
pub use mmo_schema::cluster::node_service_server::{NodeService, NodeServiceServer};
pub use mmo_schema::cluster::*;
pub use std::collections::HashMap;
pub use std::sync::Arc;

/// Component storing velocity vector for entities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityVelocity {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Error codes returned in [`RemoteCommandResponse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RemoteCommandErrorCode {
    Success = 0,
    EntityNotFound = 1,
    Vetoed = 2,
    MalformedPayload = 3,
}

/// Network errors encountered during inter-node communication and packet processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    MalformedPayload(String),
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedPayload(msg) => write!(f, "Malformed payload: {}", msg),
        }
    }
}

impl std::error::Error for NetworkError {}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::RwLock;
    use tokio_stream::wrappers::TcpListenerStream;

    fn test_bounds() -> BoundingBox3D {
        BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0))
    }

    #[tokio::test]
    async fn test_remote_command_success_when_entity_exists() {
        let sim = TickSimulation::new(1, test_bounds()).into_shared();
        let target_guid = Guid::new(EntityType::Npc, 1, 42);
        let initiator_guid = Guid::new(EntityType::Player, 2, 101);

        {
            let mut s = sim.write().await;
            assert!(s.spawn_entity(target_guid, (10.0, 10.0, 10.0)));
        }

        // Track hook dispatches
        let before_dispatched = Arc::new(AtomicUsize::new(0));
        let on_dispatched = Arc::new(AtomicUsize::new(0));

        let b_clone = before_dispatched.clone();
        let o_clone = on_dispatched.clone();

        {
            let mut s = sim.write().await;
            s.hooks.register_fn(move |event, _ctx| match event {
                Event::BeforeInteract {
                    initiator,
                    target,
                    action_id,
                } => {
                    assert_eq!(*initiator, initiator_guid);
                    assert_eq!(*target, target_guid);
                    assert_eq!(*action_id, 1001);
                    b_clone.fetch_add(1, Ordering::SeqCst);
                }
                Event::OnInteract {
                    initiator,
                    target,
                    action_id,
                } => {
                    assert_eq!(*initiator, initiator_guid);
                    assert_eq!(*target, target_guid);
                    assert_eq!(*action_id, 1001);
                    o_clone.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            });
        }

        let service = NodeServiceImpl::new(sim);
        let req = make_remote_command(initiator_guid, target_guid, 1001, &[], 2, 1);

        let resp = service.remote_command(tonic::Request::new(req)).await;
        assert!(resp.is_ok());

        let cmd_resp = resp.unwrap().into_inner();
        assert!(cmd_resp.success);
        assert_eq!(cmd_resp.error_code, RemoteCommandErrorCode::Success as u32);
        assert!(cmd_resp.error_message.is_empty());

        assert_eq!(before_dispatched.load(Ordering::SeqCst), 1);
        assert_eq!(on_dispatched.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_remote_command_error_when_not_found() {
        let sim = TickSimulation::new(1, test_bounds()).into_shared();
        let target_guid = Guid::new(EntityType::Npc, 1, 999);
        let initiator_guid = Guid::new(EntityType::Player, 2, 102);

        let service = NodeServiceImpl::new(sim);
        let req = make_remote_command(initiator_guid, target_guid, 2002, &[], 2, 1);

        let resp = service.remote_command(tonic::Request::new(req)).await;
        assert!(resp.is_ok());

        let cmd_resp = resp.unwrap().into_inner();
        assert!(!cmd_resp.success);
        assert_eq!(
            cmd_resp.error_code,
            RemoteCommandErrorCode::EntityNotFound as u32
        );
        assert!(cmd_resp.error_message.contains("not found"));
    }

    #[tokio::test]
    async fn test_remote_command_veto_when_hook_cancels() {
        let sim = TickSimulation::new(1, test_bounds()).into_shared();
        let target_guid = Guid::new(EntityType::Npc, 1, 55);
        let initiator_guid = Guid::new(EntityType::Player, 2, 103);

        {
            let mut s = sim.write().await;
            assert!(s.spawn_entity(target_guid, (0.0, 0.0, 0.0)));
        }

        let on_interact_called = Arc::new(AtomicUsize::new(0));
        let on_clone = on_interact_called.clone();

        {
            let mut s = sim.write().await;
            s.hooks.register_fn(move |event, ctx| match event {
                Event::BeforeInteract { action_id, .. } => {
                    if *action_id == 9999 {
                        ctx.cancel("Target is invulnerable");
                    }
                }
                Event::OnInteract { .. } => {
                    on_clone.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            });
        }

        let service = NodeServiceImpl::new(sim);
        let req = make_remote_command(initiator_guid, target_guid, 9999, &[], 2, 1);

        let resp = service.remote_command(tonic::Request::new(req)).await;
        assert!(resp.is_ok());

        let cmd_resp = resp.unwrap().into_inner();
        assert!(!cmd_resp.success);
        assert_eq!(cmd_resp.error_code, RemoteCommandErrorCode::Vetoed as u32);
        assert_eq!(cmd_resp.error_message, "Target is invulnerable");

        // Ensure OnInteract was short-circuited and NEVER called
        assert_eq!(on_interact_called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_remote_command_malformed_initiator_rejected() {
        let sim = TickSimulation::new(1, test_bounds()).into_shared();
        let target_guid = Guid::new(EntityType::Npc, 1, 60);

        {
            let mut s = sim.write().await;
            assert!(s.spawn_entity(target_guid, (0.0, 0.0, 0.0)));
        }

        let service = NodeServiceImpl::new(sim);

        // Case 1: Undersized payload (< 8 bytes)
        let req_short = RemoteCommandRequest {
            guid: target_guid.raw(),
            action_id: 1,
            payload: vec![1, 2, 3],
            source_node_id: 2,
            target_node_id: 1,
        };
        assert!(extract_initiator(&req_short).is_err());

        let resp_short = service
            .remote_command(tonic::Request::new(req_short))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_short.success);
        assert_eq!(
            resp_short.error_code,
            RemoteCommandErrorCode::MalformedPayload as u32
        );
        assert!(resp_short.error_message.contains("missing initiator GUID"));

        // Case 2: Zero initiator GUID (8 zero bytes)
        let req_zero = RemoteCommandRequest {
            guid: target_guid.raw(),
            action_id: 1,
            payload: vec![0u8; 8],
            source_node_id: 2,
            target_node_id: 1,
        };
        assert!(extract_initiator(&req_zero).is_err());

        let resp_zero = service
            .remote_command(tonic::Request::new(req_zero))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_zero.success);
        assert_eq!(
            resp_zero.error_code,
            RemoteCommandErrorCode::MalformedPayload as u32
        );
        assert!(resp_zero.error_message.contains("cannot be zero"));
    }

    #[tokio::test]
    async fn test_ghost_entity_sync_insert_update_and_query() {
        let sim = TickSimulation::new(1, test_bounds()).into_shared();
        let service = NodeServiceImpl::new(sim.clone());

        let ghost_guid = Guid::new(EntityType::Player, 2, 777);

        // 1. Initial ghost sync (Insert)
        let sync_req_1 = GhostSyncRequest {
            source_node_id: 2,
            ghosts: vec![GhostEntity {
                guid: ghost_guid.raw(),
                position: Some(Vector3 {
                    x: 95.0,
                    y: 0.0,
                    z: 0.0,
                }),
                velocity: Some(Vector3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                }),
                yaw: 1.57,
                pitch: 0.0,
                component_data: vec![1, 2, 3],
                last_tick: 50,
            }],
            tick: 50,
        };

        let resp1 = service
            .sync_ghosts(tonic::Request::new(sync_req_1))
            .await
            .expect("sync should succeed")
            .into_inner();

        assert!(resp1.acknowledged);
        assert_eq!(resp1.synced_count, 1);

        {
            let s = sim.read().await;
            assert_eq!(s.ghost_store.len(), 1);
            let ghost = s.ghost_store.get(ghost_guid).expect("ghost must exist");
            assert_eq!(ghost.position, (95.0, 0.0, 0.0));
            assert_eq!(ghost.velocity, (1.0, 0.0, 0.0));
            assert_eq!(ghost.yaw, 1.57);
            assert_eq!(ghost.last_tick, 50);
            assert_eq!(ghost.source_node_id, 2);
        }

        // 2. Updated ghost sync (Update)
        let sync_req_2 = GhostSyncRequest {
            source_node_id: 2,
            ghosts: vec![GhostEntity {
                guid: ghost_guid.raw(),
                position: Some(Vector3 {
                    x: 97.5,
                    y: 1.0,
                    z: 0.0,
                }),
                velocity: Some(Vector3 {
                    x: 0.5,
                    y: 0.2,
                    z: 0.0,
                }),
                yaw: std::f32::consts::PI,
                pitch: 0.1,
                component_data: vec![1, 2, 3, 4],
                last_tick: 60,
            }],
            tick: 60,
        };

        let resp2 = service
            .sync_ghosts(tonic::Request::new(sync_req_2))
            .await
            .expect("update should succeed")
            .into_inner();

        assert!(resp2.acknowledged);
        assert_eq!(resp2.synced_count, 1);

        {
            let s = sim.read().await;
            assert_eq!(s.ghost_store.len(), 1);
            let ghost = s.ghost_store.get(ghost_guid).expect("ghost must exist");
            assert_eq!(ghost.position, (97.5, 1.0, 0.0));
            assert_eq!(ghost.velocity, (0.5, 0.2, 0.0));
            assert_eq!(ghost.yaw, std::f32::consts::PI);
            assert_eq!(ghost.last_tick, 60);

            // Radius query finds it
            let in_radius = s.ghost_store.query_radius((97.0, 1.0, 0.0), 2.0);
            assert_eq!(in_radius.len(), 1);
            assert_eq!(in_radius[0].guid, ghost_guid);

            // Far query misses it
            let out_of_radius = s.ghost_store.query_radius((0.0, 0.0, 0.0), 10.0);
            assert!(out_of_radius.is_empty());
        }
    }

    #[test]
    fn test_ghost_broadcaster_boundary_identification() {
        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        let mut sim = TickSimulation::new(1, bounds);
        let margin = 5.0;
        let broadcaster = GhostBroadcaster::new(1, margin);

        // Near X max boundary: x = 47.0 (dist to 50.0 is 3.0 <= 5.0)
        let g_boundary = Guid::new(EntityType::Player, 1, 1);
        sim.spawn_entity(g_boundary, (47.0, 0.0, 0.0));

        // Deep interior entity: pos = (0.0, 0.0, 0.0) (dist to boundary is 50.0 > 5.0)
        let g_interior = Guid::new(EntityType::Player, 1, 2);
        sim.spawn_entity(g_interior, (0.0, 0.0, 0.0));

        // Near Z min boundary: z = -48.0 (dist to -50.0 is 2.0 <= 5.0)
        let g_boundary_2 = Guid::new(EntityType::Npc, 1, 3);
        sim.spawn_entity(g_boundary_2, (0.0, 0.0, -48.0));

        let boundary_ghosts =
            broadcaster.find_boundary_entities(&sim.octree, &sim.entity_store, 10);
        assert_eq!(boundary_ghosts.len(), 2);

        let guids: Vec<u64> = boundary_ghosts.iter().map(|g| g.guid).collect();
        assert!(guids.contains(&g_boundary.raw()));
        assert!(guids.contains(&g_boundary_2.raw()));
        assert!(!guids.contains(&g_interior.raw()));

        // Check build sync request
        let req = broadcaster.collect_sync_from_sim(&sim);
        assert_eq!(req.source_node_id, 1);
        assert_eq!(req.ghosts.len(), 2);
    }

    #[test]
    fn test_ghost_broadcaster_prepare_broadcast_jobs() {
        let mut broadcaster = GhostBroadcaster::new(1, 5.0);
        broadcaster.add_neighbor(2, "http://127.0.0.1:50052");
        broadcaster.add_neighbor(3, "http://127.0.0.1:50053");

        assert_eq!(broadcaster.neighbors().len(), 2);

        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut sim = TickSimulation::new(1, bounds);
        let g1 = Guid::new(EntityType::Player, 1, 10);
        sim.spawn_entity(g1, (98.0, 50.0, 50.0)); // 2m from boundary

        let jobs = broadcaster.prepare_broadcast_from_sim(&sim);
        assert_eq!(jobs.len(), 2);

        let target_node_ids: Vec<u32> = jobs.iter().map(|j| j.target_node_id).collect();
        assert!(target_node_ids.contains(&2));
        assert!(target_node_ids.contains(&3));

        for job in jobs {
            assert_eq!(job.request.source_node_id, 1);
            assert_eq!(job.request.ghosts.len(), 1);
            assert_eq!(job.request.ghosts[0].guid, g1.raw());
        }

        // Test removing neighbor
        assert_eq!(
            broadcaster.remove_neighbor(3),
            Some("http://127.0.0.1:50053".to_string())
        );
        assert_eq!(broadcaster.neighbors().len(), 1);
    }

    #[tokio::test]
    async fn test_grpc_network_roundtrip_integration() {
        // Spin up an actual Tonic gRPC server on a dynamic local port
        let sim = TickSimulation::new(1, test_bounds()).into_shared();
        let target_guid = Guid::new(EntityType::Npc, 1, 888);
        {
            let mut s = sim.write().await;
            s.spawn_entity(target_guid, (5.0, 5.0, 5.0));
        }

        let recorded_actions = Arc::new(Mutex::new(Vec::new()));
        let rec_clone = recorded_actions.clone();

        {
            let mut s = sim.write().await;
            s.hooks.register_fn(move |event, _| {
                if let Event::OnInteract {
                    initiator,
                    target,
                    action_id,
                } = event
                {
                    rec_clone
                        .lock()
                        .unwrap()
                        .push((*initiator, *target, *action_id));
                }
            });
        }

        let service = NodeServiceImpl::new(sim.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(service.into_server())
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        // Small delay to allow gRPC server to bind and start listening
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Connect client
        let channel = tonic::transport::Channel::from_shared(format!("http://{}", addr))
            .unwrap()
            .connect()
            .await
            .expect("Client connect failed");
        let mut client = NodeServiceClient::new(channel);

        // 1. Remote command over gRPC
        let initiator_guid = Guid::new(EntityType::Player, 2, 500);
        let cmd_req = make_remote_command(initiator_guid, target_guid, 777, &[], 2, 1);
        let cmd_resp = client
            .remote_command(cmd_req)
            .await
            .expect("RPC call failed")
            .into_inner();

        assert!(cmd_resp.success);
        assert_eq!(cmd_resp.error_code, 0);

        {
            let rec = recorded_actions.lock().unwrap();
            assert_eq!(rec.len(), 1);
            assert_eq!(rec[0], (initiator_guid, target_guid, 777));
        }

        // 2. Ghost sync over gRPC
        let ghost_guid = Guid::new(EntityType::Player, 2, 999);
        let sync_req = GhostSyncRequest {
            source_node_id: 2,
            ghosts: vec![GhostEntity {
                guid: ghost_guid.raw(),
                position: Some(Vector3 {
                    x: 99.0,
                    y: 99.0,
                    z: 99.0,
                }),
                velocity: Some(Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
                yaw: 0.0,
                pitch: 0.0,
                component_data: vec![],
                last_tick: 1,
            }],
            tick: 1,
        };

        let sync_resp = client
            .sync_ghosts(sync_req)
            .await
            .expect("Sync RPC failed")
            .into_inner();

        assert!(sync_resp.acknowledged);
        assert_eq!(sync_resp.synced_count, 1);

        {
            let s = sim.read().await;
            assert!(s.ghost_store.contains(ghost_guid));
            let g = s.ghost_store.get(ghost_guid).unwrap();
            assert_eq!(g.position, (99.0, 99.0, 99.0));
        }
    }

    #[tokio::test]
    async fn test_remote_command_veto_when_target_migrating() {
        let sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            1,
            test_bounds(),
        )));
        let service = NodeServiceImpl::new(Arc::clone(&sim));

        let initiator = Guid::new(EntityType::Player, 1, 10);
        let target = Guid::new(EntityType::Player, 1, 20);

        {
            let mut s = sim.write().await;
            s.spawn_entity(initiator, (0.0, 0.0, 0.0));
            s.spawn_entity(target, (5.0, 5.0, 5.0));
            // Place target under migration lease
            assert!(s.acquire_migration_lease(target));
        }

        let req = tonic::Request::new(make_remote_command(initiator, target, 1, &[], 1, 1));
        let resp = service.remote_command(req).await.unwrap().into_inner();

        assert!(!resp.success);
        assert_eq!(resp.error_code, RemoteCommandErrorCode::Vetoed as u32);
        assert!(resp.error_message.contains("currently migrating"));
    }

    #[tokio::test]
    async fn test_node_service_migrate_entity_and_outbound_execution() {
        use tonic::transport::Server;

        // Recipient Node on Node 2
        let recipient_bounds = BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let recipient_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            2,
            recipient_bounds,
        )));
        let recipient_service = NodeServiceImpl::new(Arc::clone(&recipient_sim));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let recipient_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(recipient_service.into_server())
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Donor Node on Node 1
        let donor_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0));
        let donor_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            1,
            donor_bounds,
        )));

        let guid = Guid::new(EntityType::Player, 1, 555);
        let target_pos = (55.0, 10.0, 10.0); // Inside Node 2 bounds

        {
            let mut d = donor_sim.write().await;
            d.spawn_entity(guid, (49.0, 10.0, 10.0));
            // Update entity to target_pos in preparation for migration
            d.entity_store.get_mut_by_guid(guid).unwrap().position = target_pos;
            let mut kin = crate::kinematics::EntityKinematicState::new(d.current_time);
            kin.initialized = true;
            kin.last_sequence = 123;
            d.kinematic_states.insert(guid, kin);
        }

        // Execute migration
        let res = execute_entity_migration(
            Arc::clone(&donor_sim),
            guid,
            2,
            &recipient_addr.to_string(),
            None,
        )
        .await;

        assert_eq!(res, Ok(true));

        // Verify donor has cleanly despawned the entity and cleared lease
        {
            let d = donor_sim.read().await;
            assert!(!d.entity_store.contains_guid(guid));
            assert!(!d.is_migrating(guid));
            assert!(!d.kinematic_states.contains_key(&guid));
        }

        // Verify recipient has adopted entity with sequence restored
        {
            let r = recipient_sim.read().await;
            assert!(r.entity_store.contains_guid(guid));
            let ent = r.entity_store.get_by_guid(guid).unwrap();
            assert_eq!(ent.position, target_pos);
            let kin = r.kinematic_states.get(&guid).unwrap();
            assert_eq!(kin.last_sequence, 123);
        }
    }

    #[tokio::test]
    async fn test_entity_migration_connection_failure_rollback() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(1, bounds)));
        let guid = Guid::new(EntityType::Player, 1, 777);
        let pos = (10.0, 10.0, 10.0);

        {
            let mut s = sim.write().await;
            s.spawn_entity(guid, pos);
        }

        // Try migrating to unreachable port
        let res =
            execute_entity_migration(Arc::clone(&sim), guid, 2, "127.0.0.1:59999", None).await;

        assert!(res.is_err());

        // Zero-duplication guarantee: Donor MUST still own the entity and lease MUST be released
        {
            let s = sim.read().await;
            assert!(s.entity_store.contains_guid(guid));
            assert_eq!(s.entity_store.get_by_guid(guid).unwrap().position, pos);
            assert!(!s.is_migrating(guid));
        }
    }

    #[tokio::test]
    async fn test_node_service_batch_migrate_entities_and_gateway_sync() {
        use tonic::transport::Server;

        // 1. Recipient Node on Node 2
        let recipient_bounds = BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let recipient_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            2,
            recipient_bounds,
        )));
        let recipient_service = NodeServiceImpl::new(Arc::clone(&recipient_sim));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let recipient_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(recipient_service.into_server())
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        // 2. Gateway Service
        let gw_router = Arc::new(mmo_gateway::router::SessionRouter::new());
        let gw_service = mmo_gateway::server::GatewayServiceImpl::new(Arc::clone(&gw_router));
        let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gw_addr = gw_listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(
                    mmo_schema::cluster::gateway_service_server::GatewayServiceServer::new(
                        gw_service,
                    ),
                )
                .serve_with_incoming(TcpListenerStream::new(gw_listener))
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 3. Donor Node on Node 1
        let donor_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0));
        let donor_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            1,
            donor_bounds,
        )));

        let g1 = Guid::new(EntityType::Player, 1, 801);
        let g2 = Guid::new(EntityType::Player, 1, 802);
        let g3 = Guid::new(EntityType::Player, 1, 803);

        let p1 = (55.0, 10.0, 10.0);
        let p2 = (60.0, 15.0, 15.0);
        let p3 = (65.0, 20.0, 20.0);

        let s1 = 901;
        let s2 = 902;
        let s3 = 903;

        gw_router.register_session(s1, g1, "127.0.0.1:50051".to_string());
        gw_router.register_session(s2, g2, "127.0.0.1:50051".to_string());
        gw_router.register_session(s3, g3, "127.0.0.1:50051".to_string());

        {
            let mut d = donor_sim.write().await;
            for (g, p, s, seq) in [(g1, p1, s1, 101), (g2, p2, s2, 102), (g3, p3, s3, 103)] {
                d.spawn_entity(g, (45.0, 10.0, 10.0));
                d.entity_store.get_mut_by_guid(g).unwrap().position = p;
                d.register_aoi_observer(s, g, Default::default());
                let mut kin = crate::kinematics::EntityKinematicState::new(d.current_time);
                kin.initialized = true;
                kin.last_sequence = seq;
                d.kinematic_states.insert(g, kin);
            }
        }

        // Execute batch migration
        let report = execute_batch_entity_migration(
            Arc::clone(&donor_sim),
            &[g1, g2, g3],
            2,
            &recipient_addr.to_string(),
            Some(&gw_addr.to_string()),
        )
        .await
        .unwrap();

        assert!(report.success);
        assert_eq!(report.total_requested, 3);
        assert_eq!(report.migrated_count, 3);

        // Verify donor has cleanly despawned all entities and released leases
        {
            let d = donor_sim.read().await;
            for &g in &[g1, g2, g3] {
                assert!(!d.entity_store.contains_guid(g));
                assert!(!d.is_migrating(g));
                assert!(!d.kinematic_states.contains_key(&g));
            }
        }

        // Verify recipient has adopted entities with restored state
        {
            let r = recipient_sim.read().await;
            for (g, p, seq) in [(g1, p1, 101), (g2, p2, 102), (g3, p3, 103)] {
                assert!(r.entity_store.contains_guid(g));
                let ent = r.entity_store.get_by_guid(g).unwrap();
                assert_eq!(ent.position, p);
                let kin = r.kinematic_states.get(&g).unwrap();
                assert_eq!(kin.last_sequence, seq);
            }
        }

        // Wait brief moment for gateway async route updates, then check router
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        for &g in &[g1, g2, g3] {
            assert_eq!(
                gw_router.get_route_by_guid(g).as_deref(),
                Some(recipient_addr.to_string().as_str())
            );
        }
    }

    #[tokio::test]
    async fn test_batch_entity_migration_connection_failure_rollback() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(1, bounds)));
        let g1 = Guid::new(EntityType::Player, 1, 811);
        let g2 = Guid::new(EntityType::Player, 1, 812);

        {
            let mut s = sim.write().await;
            s.spawn_entity(g1, (10.0, 10.0, 10.0));
            s.spawn_entity(g2, (20.0, 20.0, 20.0));
        }

        let res =
            execute_batch_entity_migration(Arc::clone(&sim), &[g1, g2], 2, "127.0.0.1:59998", None)
                .await;

        assert!(res.is_err());

        // Zero-duplication guarantee: Donor MUST retain entities and all leases MUST be released
        {
            let s = sim.read().await;
            assert!(s.entity_store.contains_guid(g1));
            assert!(s.entity_store.contains_guid(g2));
            assert!(!s.is_migrating(g1));
            assert!(!s.is_migrating(g2));
        }
    }

    #[tokio::test]
    async fn test_batch_entity_migration_recipient_rejection_rollback() {
        use tonic::transport::Server;

        // Recipient Node covers (50.0..100.0)
        let recipient_bounds = BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let recipient_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            2,
            recipient_bounds,
        )));
        let recipient_service = NodeServiceImpl::new(Arc::clone(&recipient_sim));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let recipient_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(recipient_service.into_server())
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Donor Node covers (0.0..50.0)
        let donor_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0));
        let donor_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
            1,
            donor_bounds,
        )));

        let g1 = Guid::new(EntityType::Player, 1, 821);
        let g2 = Guid::new(EntityType::Player, 1, 822);

        // Position g1 and g2 outside recipient bounds (at 10.0, 10.0, 10.0)
        {
            let mut d = donor_sim.write().await;
            d.spawn_entity(g1, (10.0, 10.0, 10.0));
            d.spawn_entity(g2, (10.0, 10.0, 10.0));
        }

        let res = execute_batch_entity_migration(
            Arc::clone(&donor_sim),
            &[g1, g2],
            2,
            &recipient_addr.to_string(),
            None,
        )
        .await;

        assert!(matches!(res, Err(MigrationError::RecipientRejected(_))));

        // Verify donor still owns entities and all leases were released
        {
            let d = donor_sim.read().await;
            assert!(d.entity_store.contains_guid(g1));
            assert!(d.entity_store.contains_guid(g2));
            assert!(!d.is_migrating(g1));
            assert!(!d.is_migrating(g2));
        }

        // Verify recipient rejected all entities and ingested none
        {
            let r = recipient_sim.read().await;
            assert!(!r.entity_store.contains_guid(g1));
            assert!(!r.entity_store.contains_guid(g2));
        }
    }

    #[tokio::test]
    async fn test_execute_remote_ability_damage_and_interrupt() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds).into_shared();
        let target_guid = Guid::new(EntityType::Player, 1, 101);
        let caster_guid = Guid::new(EntityType::Player, 2, 202);

        {
            let mut s = sim.write().await;
            s.spawn_entity(target_guid, (10.0, 0.0, 0.0));

            let mut q = mmo_core::ActionQueue::new();
            let action = mmo_core::TimedAction::new(
                1,
                2.0,
                mmo_core::ActionInterruptFlags {
                    cancel_on_damage: true,
                    cancel_on_movement: false,
                    cancel_on_stun: true,
                    cancel_on_target_lost: true,
                },
                None,
            );
            q.start_action(
                target_guid,
                action,
                mmo_core::QueuePolicy::Replace,
                &mut s.event_queue,
            )
            .unwrap();

            let ent = s.entity_store.get_mut_by_guid(target_guid).unwrap();
            let mut attrs = mmo_core::AttributeSet::new();
            attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
            ent.add_component(attrs);
            ent.add_component(q);
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));
        let req = RemoteAbilityRequest {
            ability_id: 1,
            caster_guid: caster_guid.raw(),
            target_guid: target_guid.raw(),
            source_node_id: 2,
            target_node_id: 1,
            caster_position: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            max_range: 20.0,
            damage: 35,
            healing: 0,
            resource_cost: 10,
            cast_id: 1,
        };

        let resp = service
            .dispatch_ability(tonic::Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.success);
        assert_eq!(
            resp.error_code,
            RemoteAbilityErrorCode::RemoteAbilitySuccess as i32
        );
        assert_eq!(resp.current_health, 65);

        // Verify action queue interrupted
        let s = sim.read().await;
        let ent = s.entity_store.get_by_guid(target_guid).unwrap();
        let q = ent.get_component::<mmo_core::ActionQueue>().unwrap();
        assert!(q.current().is_none());
    }

    #[tokio::test]
    async fn test_execute_remote_ability_out_of_range() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds).into_shared();
        let target_guid = Guid::new(EntityType::Player, 1, 102);
        let caster_guid = Guid::new(EntityType::Player, 2, 203);

        {
            let mut s = sim.write().await;
            s.spawn_entity(target_guid, (50.0, 0.0, 0.0));
            let ent = s.entity_store.get_mut_by_guid(target_guid).unwrap();
            let mut attrs = mmo_core::AttributeSet::new();
            attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
            ent.add_component(attrs);
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));
        let req = RemoteAbilityRequest {
            ability_id: 1,
            caster_guid: caster_guid.raw(),
            target_guid: target_guid.raw(),
            source_node_id: 2,
            target_node_id: 1,
            caster_position: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            max_range: 30.0, // max allowed 36.0, dist is 50.0
            damage: 30,
            healing: 0,
            resource_cost: 10,
            cast_id: 2,
        };

        let resp = service
            .dispatch_ability(tonic::Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.success);
        assert_eq!(
            resp.error_code,
            RemoteAbilityErrorCode::RemoteAbilityOutOfRange as i32
        );
    }

    #[tokio::test]
    async fn test_execute_remote_ability_target_dead_and_migrating() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds).into_shared();
        let dead_guid = Guid::new(EntityType::Player, 1, 103);
        let migrating_guid = Guid::new(EntityType::Player, 1, 104);
        let caster_guid = Guid::new(EntityType::Player, 2, 204);

        {
            let mut s = sim.write().await;
            s.spawn_entity(dead_guid, (5.0, 0.0, 0.0));
            let ent_dead = s.entity_store.get_mut_by_guid(dead_guid).unwrap();
            let mut dead_attrs = mmo_core::AttributeSet::new();
            dead_attrs.add_attribute("health", 0.0, 0.0, 100.0, 0.0);
            ent_dead.add_component(dead_attrs);
            ent_dead.state_flags |= mmo_core::entity::state_flags::INCAPACITATED;

            s.spawn_entity(migrating_guid, (6.0, 0.0, 0.0));
            let ent_mig = s.entity_store.get_mut_by_guid(migrating_guid).unwrap();
            let mut mig_attrs = mmo_core::AttributeSet::new();
            mig_attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
            ent_mig.add_component(mig_attrs);
            assert!(s.acquire_migration_lease(migrating_guid));
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));

        // Test dead target
        let req_dead = RemoteAbilityRequest {
            ability_id: 1,
            caster_guid: caster_guid.raw(),
            target_guid: dead_guid.raw(),
            source_node_id: 2,
            target_node_id: 1,
            caster_position: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            max_range: 20.0,
            damage: 20,
            healing: 0,
            resource_cost: 5,
            cast_id: 3,
        };
        let resp_dead = service
            .dispatch_ability(tonic::Request::new(req_dead))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_dead.success);
        assert_eq!(
            resp_dead.error_code,
            RemoteAbilityErrorCode::RemoteAbilityTargetDead as i32
        );

        // Test migrating target
        let req_mig = RemoteAbilityRequest {
            ability_id: 1,
            caster_guid: caster_guid.raw(),
            target_guid: migrating_guid.raw(),
            source_node_id: 2,
            target_node_id: 1,
            caster_position: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            max_range: 20.0,
            damage: 20,
            healing: 0,
            resource_cost: 5,
            cast_id: 4,
        };
        let resp_mig = service
            .dispatch_ability(tonic::Request::new(req_mig))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_mig.success);
        assert_eq!(
            resp_mig.error_code,
            RemoteAbilityErrorCode::RemoteAbilityTargetMigrating as i32
        );
    }

    #[tokio::test]
    async fn test_dispatch_batch_ability() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds).into_shared();
        let t1 = Guid::new(EntityType::Player, 1, 105);
        let t2 = Guid::new(EntityType::Player, 1, 106);
        let caster = Guid::new(EntityType::Player, 2, 205);

        {
            let mut s = sim.write().await;
            s.spawn_entity(t1, (2.0, 0.0, 0.0));
            let mut a1 = mmo_core::AttributeSet::new();
            a1.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
            s.entity_store
                .get_mut_by_guid(t1)
                .unwrap()
                .add_component(a1);

            s.spawn_entity(t2, (3.0, 0.0, 0.0));
            let mut a2 = mmo_core::AttributeSet::new();
            a2.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
            s.entity_store
                .get_mut_by_guid(t2)
                .unwrap()
                .add_component(a2);
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));
        let req = BatchRemoteAbilityRequest {
            ability_id: 10,
            caster_guid: caster.raw(),
            source_node_id: 2,
            target_node_id: 1,
            caster_position: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            max_range: 15.0,
            damage_per_target: 40,
            healing_per_target: 0,
            target_guids: vec![t1.raw(), t2.raw()],
            cast_id: 5,
        };

        let resp = service
            .dispatch_batch_ability(tonic::Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.success);
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].current_health, 60);
        assert_eq!(resp.results[1].current_health, 60);
    }

    #[tokio::test]
    async fn test_node_service_container_transfer_reserve_commit_and_mpsc_notification() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds).into_shared();
        let db = mmo_storage::Database::connect_memory().await.unwrap();

        let container_repo = mmo_storage::ContainerRepository::new(db.clone());
        let c1 = mmo_storage::ContainerRecord {
            container_id: 8001,
            owner_type: "character".to_string(),
            owner_id: "2001".to_string(),
            name: "Vault A".to_string(),
            capacity: 10,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        let c2 = mmo_storage::ContainerRecord {
            container_id: 8002,
            owner_type: "character".to_string(),
            owner_id: "2002".to_string(),
            name: "Vault B".to_string(),
            capacity: 10,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        container_repo.create_container(&c1).await.unwrap();
        container_repo.create_container(&c2).await.unwrap();

        let char_repo = mmo_storage::CharacterRepository::new(db.clone());
        let item = mmo_storage::ItemRecord {
            item_instance_id: "xnode_item_1".to_string(),
            character_guid: 2001,
            container_id: 8001,
            item_def_id: 777,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        let coord = Arc::new(mmo_storage::TransactionCoordinator::new(db));
        let service = NodeServiceImpl::with_coordinator(Arc::clone(&sim), coord);

        // 1. Reserve source slot
        let res_out = service
            .reserve_container_slot(tonic::Request::new(ReserveContainerSlotRequest {
                tx_id: "tx_grpc_1".to_string(),
                container_id: 8001,
                slot: 0,
                item_instance_id: "xnode_item_1".to_string(),
                reservation_type: "transfer_out".to_string(),
                lease_duration_ms: 5000,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(res_out.success);
        assert!(!res_out.reservation_id.is_empty());

        // 2. Reserve destination slot
        let res_in = service
            .reserve_container_slot(tonic::Request::new(ReserveContainerSlotRequest {
                tx_id: "tx_grpc_1".to_string(),
                container_id: 8002,
                slot: 3,
                item_instance_id: String::new(),
                reservation_type: "transfer_in".to_string(),
                lease_duration_ms: 5000,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(res_in.success);

        // 3. Commit transfer
        let commit_res = service
            .commit_container_transfer(tonic::Request::new(CommitContainerTransferRequest {
                tx_id: "tx_grpc_1".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(commit_res.success);
        assert_eq!(commit_res.src_container_id, 8001);
        assert_eq!(commit_res.dst_container_id, 8002);
        assert_eq!(commit_res.item_instance_id, "xnode_item_1");

        // 4. Verify item moved in storage
        let item_moved = char_repo.get_item("xnode_item_1").await.unwrap().unwrap();
        assert_eq!(item_moved.container_id, 8002);
        assert_eq!(item_moved.slot, 3);

        // 5. Verify non-blocking MPSC notification received in simulation tick
        {
            let mut s = sim.write().await;
            let notif = s.container_transfer_receiver.try_recv().unwrap();
            assert_eq!(notif.tx_id, "tx_grpc_1");
            assert!(notif.success);
            assert_eq!(notif.src_container_id, 8001);
            assert_eq!(notif.dst_container_id, 8002);
        }
    }

    #[tokio::test]
    async fn test_node_service_container_transfer_abort_and_mpsc_notification() {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds).into_shared();
        let db = mmo_storage::Database::connect_memory().await.unwrap();

        let container_repo = mmo_storage::ContainerRepository::new(db.clone());
        let c1 = mmo_storage::ContainerRecord {
            container_id: 8101,
            owner_type: "character".to_string(),
            owner_id: "2001".to_string(),
            name: "Vault A".to_string(),
            capacity: 10,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        container_repo.create_container(&c1).await.unwrap();

        let coord = Arc::new(mmo_storage::TransactionCoordinator::new(db));
        let service = NodeServiceImpl::with_coordinator(Arc::clone(&sim), coord);

        // Reserve slot
        let res_in = service
            .reserve_container_slot(tonic::Request::new(ReserveContainerSlotRequest {
                tx_id: "tx_grpc_abort".to_string(),
                container_id: 8101,
                slot: 2,
                item_instance_id: String::new(),
                reservation_type: "transfer_in".to_string(),
                lease_duration_ms: 5000,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(res_in.success);

        // Abort transfer
        let abort_res = service
            .abort_container_transfer(tonic::Request::new(AbortContainerTransferRequest {
                tx_id: "tx_grpc_abort".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(abort_res.success);

        // Verify MPSC notification
        {
            let mut s = sim.write().await;
            let notif = s.container_transfer_receiver.try_recv().unwrap();
            assert_eq!(notif.tx_id, "tx_grpc_abort");
            assert!(!notif.success);
        }
    }

    #[tokio::test]
    async fn test_forward_client_packet_epoch_fencing() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let sim = Arc::new(RwLock::new(TickSimulation::new(1, bounds)));
        let guid = Guid::new(EntityType::Player, 1, 42);

        {
            let mut s = sim.write().await;
            s.spawn_entity(guid, (0.0, 0.0, 0.0));
            s.session_epochs.insert(guid, 2);
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));

        let move_pkt =
            mmo_schema::client::ClientPacket::Movement(mmo_schema::client::MovementPacket {
                entity_id: guid.raw(),
                position: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                velocity: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                timestamp: 100,
                sequence: 1,
            });

        // Stale epoch 1 (less than authoritative epoch 2)
        let resp_stale = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 1,
                guid: guid.raw(),
                packet_data: move_pkt.encode(),
                session_epoch: 1,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_stale.success);
        assert!(resp_stale.error_message.contains("Stale session epoch"));

        // Current epoch 2 succeeds
        let resp_ok = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 1,
                guid: guid.raw(),
                packet_data: move_pkt.encode(),
                session_epoch: 2,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(resp_ok.success);
    }

    #[tokio::test]
    async fn test_forward_client_packet_place_object_error() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let sim = Arc::new(RwLock::new(TickSimulation::new(1, bounds)));
        let guid = Guid::new(EntityType::Player, 1, 50);

        {
            let mut s = sim.write().await;
            s.spawn_entity(guid, (0.0, 0.0, 0.0));
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));

        // Invalid placement with NaN coordinate
        let place_pkt =
            mmo_schema::client::ClientPacket::PlaceObject(mmo_schema::client::PlaceObjectPacket {
                prefab_id: 1,
                pos: mmo_schema::client::Vec3::new(f32::NAN, 0.0, 0.0),
                rot: (0.0, 0.0, 0.0, 1.0),
                scale: mmo_schema::client::Vec3::new(1.0, 1.0, 1.0),
                properties: std::collections::HashMap::new(),
            });

        let resp = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 1,
                guid: guid.raw(),
                packet_data: place_pkt.encode(),
                session_epoch: 0,
            }))
            .await
            .unwrap()
            .into_inner();

        assert!(!resp.success);
        assert!(resp.error_message.contains("finite"));
    }

    #[tokio::test]
    async fn test_forward_client_packet_world_object_resync_queued() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let sim = Arc::new(RwLock::new(TickSimulation::new(1, bounds)));
        let player = Guid::new(EntityType::Player, 1, 60);
        let obj_guid = Guid::new(EntityType::DynamicObject, 1, 777);

        {
            let mut s = sim.write().await;
            s.spawn_entity(player, (0.0, 0.0, 0.0));
            let obj = mmo_core::world::WorldObject::new(
                obj_guid,
                1,
                mmo_core::world::Transform3D::new(
                    (5.0, 0.0, 0.0),
                    (0.0, 0.0, 0.0, 1.0),
                    (1.0, 1.0, 1.0),
                ),
                BoundingBox3D::new((4.0, -1.0, -1.0), (6.0, 1.0, 1.0)),
                mmo_core::container::ContainerOwner::Character(10),
            );
            s.insert_world_object(obj);
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));

        let resync_pkt = mmo_schema::client::ClientPacket::WorldObjectResync(
            mmo_schema::client::WorldObjectResyncPacket {
                object_id: obj_guid.raw(),
            },
        );

        let resp = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 55,
                guid: player.raw(),
                packet_data: resync_pkt.encode(),
                session_epoch: 0,
            }))
            .await
            .unwrap()
            .into_inner();

        assert!(resp.success);

        // Verify pending broadcast jobs has WorldObjectSpawn
        let s = sim.read().await;
        assert_eq!(s.pending_broadcast_jobs.len(), 1);
        let job = &s.pending_broadcast_jobs[0];
        assert_eq!(job.session_id, 55);
        assert_eq!(job.player_guid, player);
        assert!(matches!(
            job.packets[0],
            mmo_schema::client::ClientPacket::WorldObjectSpawn(_)
        ));
    }

    #[tokio::test]
    async fn test_forward_client_packet_modular_router_and_anti_spoofing() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let sim = Arc::new(RwLock::new(TickSimulation::new(1, bounds)));
        let player = Guid::new(EntityType::Player, 1, 70);

        struct TestModuleHandler;
        impl mmo_core::ModularPacketHandler for TestModuleHandler {
            fn handle(
                &self,
                _entity_guid: Guid,
                sub_opcode: u16,
                payload: &[u8],
                _entities: &mut EntityStore,
            ) -> Result<Option<Vec<u8>>, mmo_core::PacketHandlerError> {
                assert_eq!(sub_opcode, 1);
                assert_eq!(payload, b"ping");
                Ok(Some(b"pong".to_vec()))
            }
        }

        {
            let mut s = sim.write().await;
            s.spawn_entity(player, (0.0, 0.0, 0.0));
            s.packets.register(0x00A0, Box::new(TestModuleHandler));
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));

        // 1. Spoofed source_guid rejected
        let spoofed_pkt =
            mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
                source_guid: 9999, // mismatch with player
                target_guid: 0,
                interaction_type: mmo_schema::client::InteractionType::Custom(1),
                action_id: 0x00A0_0001,
                payload: bytes::Bytes::from_static(b"ping"),
            });

        let resp_spoofed = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 1,
                guid: player.raw(),
                packet_data: spoofed_pkt.encode(),
                session_epoch: 0,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_spoofed.success);
        assert!(resp_spoofed.error_message.contains("Spoofed"));

        // 2. Valid interaction dispatched to module
        let valid_pkt =
            mmo_schema::client::ClientPacket::Interaction(mmo_schema::client::InteractionPacket {
                source_guid: player.raw(),
                target_guid: 0,
                interaction_type: mmo_schema::client::InteractionType::Custom(1),
                action_id: 0x00A0_0001,
                payload: bytes::Bytes::from_static(b"ping"),
            });

        let resp_valid = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 1,
                guid: player.raw(),
                packet_data: valid_pkt.encode(),
                session_epoch: 0,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(resp_valid.success);

        // Verify response packet queued
        let s = sim.read().await;
        assert_eq!(s.pending_broadcast_jobs.len(), 1);
        let job = &s.pending_broadcast_jobs[0];
        if let mmo_schema::client::ClientPacket::InteractionResult(ref res) = job.packets[0] {
            assert_eq!(res.payload, b"pong");
            assert!(res.success);
        } else {
            panic!("Expected InteractionResultPacket");
        }
    }

    #[tokio::test]
    async fn test_forward_client_packet_auto_hydration_for_new_player() {
        let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
        let sim = Arc::new(RwLock::new(TickSimulation::new(1, bounds)));
        let new_player = Guid::new(EntityType::Player, 1, 888);
        let non_player = Guid::new(EntityType::Npc, 1, 999);

        let spawn_counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&spawn_counter);

        {
            let mut s = sim.write().await;
            s.hooks.register_fn(move |event, _ctx| {
                if let mmo_core::Event::OnSpawn { entity, .. } = event {
                    if *entity == new_player {
                        counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            });
        }

        let service = NodeServiceImpl::new(Arc::clone(&sim));

        // 1. Unknown non-player entity is rejected
        let move_pkt =
            mmo_schema::client::ClientPacket::Movement(mmo_schema::client::MovementPacket {
                entity_id: non_player.raw(),
                position: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                velocity: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                sequence: 1,
                timestamp: 100,
            });
        let resp_non_player = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 2,
                guid: non_player.raw(),
                packet_data: move_pkt.encode(),
                session_epoch: 1,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp_non_player.success);
        assert!(resp_non_player.error_message.contains("not found on node"));

        // 2. Newly arriving Player entity is auto-hydrated
        let player_move_pkt =
            mmo_schema::client::ClientPacket::Movement(mmo_schema::client::MovementPacket {
                entity_id: new_player.raw(),
                position: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                velocity: mmo_schema::client::Vec3::new(0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                sequence: 1,
                timestamp: 100,
            });
        let resp_player = service
            .forward_client_packet(tonic::Request::new(ForwardClientPacketRequest {
                session_id: 3,
                guid: new_player.raw(),
                packet_data: player_move_pkt.encode(),
                session_epoch: 1,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(
            resp_player.success,
            "resp_player failed: {}",
            resp_player.error_message
        );

        // Verify entity is now stored, AoI observer is registered, and spawn hook was triggered
        let s = sim.read().await;
        assert!(s.entity_store.contains_guid(new_player));
        assert!(s.aoi.get_observer(3).is_some());
        assert_eq!(spawn_counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
