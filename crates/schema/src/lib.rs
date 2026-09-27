//! Generated Protocol and Schema definitions

#[cfg(feature = "grpc")]
#[allow(clippy::result_large_err)]
pub mod cluster {
    tonic::include_proto!("mmo.cluster");
}

pub mod bitpack;
pub mod client;
pub mod quantize;

#[cfg(all(test, feature = "grpc"))]
mod tests {
    use super::cluster::*;

    #[test]
    fn test_cluster_director_messages() {
        let req = RegisterNodeRequest {
            node_id: 1,
            listen_addr: "127.0.0.1:50051".to_string(),
            bounds: Some(BoundingBox {
                min: Some(Vector3 {
                    x: -100.0,
                    y: -100.0,
                    z: -100.0,
                }),
                max: Some(Vector3 {
                    x: 100.0,
                    y: 100.0,
                    z: 100.0,
                }),
            }),
        };
        assert_eq!(req.node_id, 1);
        assert_eq!(req.listen_addr, "127.0.0.1:50051");

        let resp = RegisterNodeResponse {
            success: true,
            message: "Node registered".to_string(),
            assigned_node_id: 1,
            topology_epoch: 1,
        };
        assert!(resp.success);
        assert_eq!(resp.topology_epoch, 1);

        let hb = HeartbeatRequest {
            node_id: 1,
            tick_rate: 20.0,
            entity_count: 150,
            cpu_percent: 12.5,
            memory_bytes: 1024 * 1024 * 64,
            timestamp: 1700000000,
        };
        assert_eq!(hb.entity_count, 150);

        let split = SplitReportRequest {
            node_id: 1,
            original_bounds: Some(BoundingBox {
                min: Some(Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
                max: Some(Vector3 {
                    x: 100.0,
                    y: 100.0,
                    z: 100.0,
                }),
            }),
            child_bounds: vec![BoundingBox {
                min: Some(Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
                max: Some(Vector3 {
                    x: 50.0,
                    y: 50.0,
                    z: 50.0,
                }),
            }],
            total_entities: 300,
        };
        assert_eq!(split.total_entities, 300);
        assert_eq!(split.child_bounds.len(), 1);
    }

    #[test]
    fn test_cluster_node_messages() {
        let cmd = RemoteCommandRequest {
            guid: 42,
            action_id: 1,
            payload: vec![1, 2, 3],
            source_node_id: 1,
            target_node_id: 2,
        };
        assert_eq!(cmd.guid, 42);
        assert_eq!(cmd.action_id, 1);

        let ghost_sync = GhostSyncRequest {
            source_node_id: 1,
            ghosts: vec![GhostEntity {
                guid: 100,
                position: Some(Vector3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                }),
                velocity: Some(Vector3 {
                    x: 0.1,
                    y: 0.0,
                    z: 0.0,
                }),
                yaw: 1.0,
                pitch: 0.0,
                component_data: vec![],
                last_tick: 50,
            }],
            tick: 50,
        };
        assert_eq!(ghost_sync.ghosts.len(), 1);
    }

    #[test]
    fn test_cluster_migration_and_gateway_messages() {
        let req = MigrateEntityRequest {
            guid: 12345,
            source_node_id: 1,
            target_node_id: 2,
            position: Some(Vector3 {
                x: 10.0,
                y: 20.0,
                z: 30.0,
            }),
            velocity: Some(Vector3 {
                x: 0.5,
                y: 0.0,
                z: 0.0,
            }),
            yaw: 1.57,
            pitch: 0.0,
            last_sequence: 42,
            session_id: 999,
            components: vec![SerializedComponent {
                component_id: 1,
                data: vec![1, 2, 3, 4],
            }],
            topology_epoch: 2,
            last_migration_tick: 100,
        };
        assert_eq!(req.guid, 12345);
        assert_eq!(req.components.len(), 1);
        assert_eq!(req.last_migration_tick, 100);

        let resp = MigrateEntityResponse {
            success: true,
            error_code: 0,
            error_message: String::new(),
            acknowledged_guid: 12345,
        };
        assert!(resp.success);
        assert_eq!(resp.acknowledged_guid, 12345);

        let route_req = RouteUpdateRequest {
            guid: 12345,
            session_id: 999,
            node_address: "127.0.0.1:50052".to_string(),
        };
        assert_eq!(route_req.node_address, "127.0.0.1:50052");

        let route_resp = RouteUpdateResponse {
            success: true,
            error_message: String::new(),
        };
        assert!(route_resp.success);
    }
}
