//! Director gRPC Service Implementation.
//!
//! Implements the `DirectorService` trait handling node registrations,
//! heartbeats, telemetry aggregation, and split command execution.

use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};

use mmo_core::BoundingBox3D;
use mmo_schema::cluster::director_service_server::DirectorService;
use mmo_schema::cluster::{
    BoundingBox, HeartbeatRequest, HeartbeatResponse, RegisterNodeRequest, RegisterNodeResponse,
    SplitReportRequest, SplitReportResponse, Vector3,
};

use crate::coordinator::GlobalOctreeCoordinator;
use crate::registry::NodeRegistry;

/// Converts a protobuf `BoundingBox` into a core `BoundingBox3D`.
pub fn proto_bbox_to_bounding_box_3d(bbox: &BoundingBox) -> Option<BoundingBox3D> {
    let min = bbox.min.as_ref()?;
    let max = bbox.max.as_ref()?;
    Some(BoundingBox3D::new(
        (min.x, min.y, min.z),
        (max.x, max.y, max.z),
    ))
}

/// Converts a core `BoundingBox3D` into a protobuf `BoundingBox`.
pub fn bounding_box_3d_to_proto(bbox: &BoundingBox3D) -> BoundingBox {
    BoundingBox {
        min: Some(Vector3 {
            x: bbox.min.0,
            y: bbox.min.1,
            z: bbox.min.2,
        }),
        max: Some(Vector3 {
            x: bbox.max.0,
            y: bbox.max.1,
            z: bbox.max.2,
        }),
    }
}

/// Implementation of the `DirectorService` gRPC interface.
#[derive(Clone)]
pub struct DirectorServiceImpl {
    pub registry: Arc<RwLock<NodeRegistry>>,
    pub coordinator: Arc<RwLock<GlobalOctreeCoordinator>>,
}

impl DirectorServiceImpl {
    /// Creates a new `DirectorServiceImpl` with shared registry and coordinator.
    pub fn new(
        registry: Arc<RwLock<NodeRegistry>>,
        coordinator: Arc<RwLock<GlobalOctreeCoordinator>>,
    ) -> Self {
        Self {
            registry,
            coordinator,
        }
    }

    /// Creates a `DirectorServiceImpl` with default empty registry and world coordinator.
    pub fn default_service() -> Self {
        Self::new(
            Arc::new(RwLock::new(NodeRegistry::new())),
            Arc::new(RwLock::new(GlobalOctreeCoordinator::default())),
        )
    }

    /// Checks for dead/unresponsive simulation nodes after `timeout`, evicting them from the registry
    /// and immediately reassigning their orphaned spatial partitions to surviving nodes.
    ///
    /// Maintains strict lock acquisition ordering (`registry` then `coordinator`) to prevent deadlocks.
    pub async fn check_and_recover_dead_nodes(
        &self,
        timeout: std::time::Duration,
    ) -> Vec<crate::coordinator::PartitionRecoveryEvent> {
        let mut reg = self.registry.write().await;
        let dead_nodes = reg.check_dead_nodes(timeout);

        if dead_nodes.is_empty() {
            metrics::gauge!("mmo_director_active_nodes").set(reg.len() as f64);
            return Vec::new();
        }

        let mut coord = self.coordinator.write().await;
        let events = coord.recover_orphaned_partitions(&dead_nodes, &mut reg);
        if !events.is_empty() {
            metrics::counter!("mmo_director_partition_recoveries_total")
                .increment(events.len() as u64);
            metrics::gauge!("mmo_director_topology_epoch").set(coord.topology_epoch() as f64);
        }
        metrics::gauge!("mmo_director_active_nodes").set(reg.len() as f64);
        events
    }
}

#[tonic::async_trait]
impl DirectorService for DirectorServiceImpl {
    async fn register_node(
        &self,
        request: Request<RegisterNodeRequest>,
    ) -> Result<Response<RegisterNodeResponse>, Status> {
        let req = request.into_inner();

        let mut assigned_bounds = Vec::new();
        if let Some(ref bounds_proto) = req.bounds {
            if let Some(bbox) = proto_bbox_to_bounding_box_3d(bounds_proto) {
                assigned_bounds.push(bbox);
            }
        }

        let mut reg = self.registry.write().await;
        let assigned_node_id =
            reg.register_node_with_id(req.node_id, req.listen_addr, assigned_bounds);

        let mut coord = self.coordinator.write().await;
        coord.assign_initial_node(assigned_node_id);
        let topology_epoch = coord.topology_epoch();

        metrics::gauge!("mmo_director_active_nodes").set(reg.len() as f64);
        metrics::gauge!("mmo_director_topology_epoch").set(topology_epoch as f64);

        Ok(Response::new(RegisterNodeResponse {
            success: true,
            message: "Simulation node registered successfully".to_string(),
            assigned_node_id,
            topology_epoch,
        }))
    }

    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let req = request.into_inner();

        let mut reg = self.registry.write().await;
        let acknowledged = reg.record_heartbeat(&req);

        let coord = self.coordinator.read().await;
        let topology_epoch = coord.topology_epoch();

        metrics::gauge!("mmo_director_active_nodes").set(reg.len() as f64);
        metrics::gauge!("mmo_director_topology_epoch").set(topology_epoch as f64);

        if acknowledged {
            let should_split = coord.evaluate_heartbeat(req.node_id, req.entity_count);
            Ok(Response::new(HeartbeatResponse {
                acknowledged: true,
                should_split,
                topology_epoch,
            }))
        } else {
            Ok(Response::new(HeartbeatResponse {
                acknowledged: false,
                should_split: false,
                topology_epoch,
            }))
        }
    }

    async fn report_split(
        &self,
        request: Request<SplitReportRequest>,
    ) -> Result<Response<SplitReportResponse>, Status> {
        let req = request.into_inner();

        let orig_proto = req.original_bounds.ok_or_else(|| {
            Status::invalid_argument("SplitReportRequest requires original_bounds")
        })?;

        let original_bounds = proto_bbox_to_bounding_box_3d(&orig_proto)
            .ok_or_else(|| Status::invalid_argument("Invalid coordinates in original_bounds"))?;

        let child_bounds: Vec<BoundingBox3D> = req
            .child_bounds
            .iter()
            .filter_map(proto_bbox_to_bounding_box_3d)
            .collect();

        let mut reg = self.registry.write().await;
        let mut coord = self.coordinator.write().await;

        let assigned_node_ids =
            coord.handle_split(req.node_id, original_bounds, child_bounds, &mut reg);
        let topology_epoch = coord.topology_epoch();

        metrics::gauge!("mmo_director_topology_epoch").set(topology_epoch as f64);

        Ok(Response::new(SplitReportResponse {
            acknowledged: true,
            assigned_node_ids,
            topology_epoch,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bbox_proto_conversions() {
        let original = BoundingBox3D::new((-50.0, -25.0, 0.0), (50.0, 25.0, 100.0));
        let proto = bounding_box_3d_to_proto(&original);

        let roundtrip = proto_bbox_to_bounding_box_3d(&proto).expect("roundtrip should succeed");
        assert_eq!(original, roundtrip);

        // Missing min/max
        let invalid_proto = BoundingBox {
            min: None,
            max: None,
        };
        assert!(proto_bbox_to_bounding_box_3d(&invalid_proto).is_none());
    }

    #[tokio::test]
    async fn test_service_register_node() {
        let service = DirectorServiceImpl::default_service();

        let req = Request::new(RegisterNodeRequest {
            node_id: 0,
            listen_addr: "127.0.0.1:50051".to_string(),
            bounds: Some(bounding_box_3d_to_proto(&BoundingBox3D::new(
                (0.0, 0.0, 0.0),
                (100.0, 100.0, 100.0),
            ))),
        });

        let resp = service.register_node(req).await.unwrap().into_inner();
        assert!(resp.success);
        assert_eq!(resp.assigned_node_id, 1);

        // Second node
        let req2 = Request::new(RegisterNodeRequest {
            node_id: 0,
            listen_addr: "127.0.0.1:50052".to_string(),
            bounds: None,
        });
        let resp2 = service.register_node(req2).await.unwrap().into_inner();
        assert!(resp2.success);
        assert_eq!(resp2.assigned_node_id, 2);

        // Verify registry contains both nodes
        let reg = service.registry.read().await;
        assert_eq!(reg.len(), 2);
    }

    #[tokio::test]
    async fn test_service_heartbeat() {
        let service = DirectorServiceImpl::default_service();

        // Register node
        let reg_req = Request::new(RegisterNodeRequest {
            node_id: 1,
            listen_addr: "127.0.0.1:50051".to_string(),
            bounds: None,
        });
        service.register_node(reg_req).await.unwrap();

        // Normal heartbeat (< 50 entities) -> should_split = false
        let hb1 = Request::new(HeartbeatRequest {
            node_id: 1,
            tick_rate: 20.0,
            entity_count: 30,
            cpu_percent: 10.0,
            memory_bytes: 1024,
            timestamp: 1000,
        });
        let resp1 = service.heartbeat(hb1).await.unwrap().into_inner();
        assert!(resp1.acknowledged);
        assert!(!resp1.should_split);

        // High load heartbeat (>= 50 entities) -> should_split = true
        let hb2 = Request::new(HeartbeatRequest {
            node_id: 1,
            tick_rate: 20.0,
            entity_count: 55,
            cpu_percent: 45.0,
            memory_bytes: 2048,
            timestamp: 2000,
        });
        let resp2 = service.heartbeat(hb2).await.unwrap().into_inner();
        assert!(resp2.acknowledged);
        assert!(resp2.should_split);

        // Heartbeat for unknown node -> acknowledged = false
        let hb_unknown = Request::new(HeartbeatRequest {
            node_id: 999,
            tick_rate: 20.0,
            entity_count: 10,
            cpu_percent: 5.0,
            memory_bytes: 1024,
            timestamp: 3000,
        });
        let resp_unknown = service.heartbeat(hb_unknown).await.unwrap().into_inner();
        assert!(!resp_unknown.acknowledged);
        assert!(!resp_unknown.should_split);
    }

    #[tokio::test]
    async fn test_service_report_split() {
        let service = DirectorServiceImpl::default_service();

        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));

        // Register node 1 and node 2
        service
            .register_node(Request::new(RegisterNodeRequest {
                node_id: 1,
                listen_addr: "127.0.0.1:50051".to_string(),
                bounds: Some(bounding_box_3d_to_proto(&bounds)),
            }))
            .await
            .unwrap();

        service
            .register_node(Request::new(RegisterNodeRequest {
                node_id: 2,
                listen_addr: "127.0.0.1:50052".to_string(),
                bounds: None,
            }))
            .await
            .unwrap();

        // Valid split report
        let split_req = Request::new(SplitReportRequest {
            node_id: 1,
            original_bounds: Some(bounding_box_3d_to_proto(&bounds)),
            child_bounds: vec![],
            total_entities: 80,
        });
        let split_resp = service.report_split(split_req).await.unwrap().into_inner();
        assert!(split_resp.acknowledged);
        assert_eq!(split_resp.assigned_node_ids.len(), 8);

        // Invalid split report: missing original_bounds
        let invalid_req = Request::new(SplitReportRequest {
            node_id: 1,
            original_bounds: None,
            child_bounds: vec![],
            total_entities: 80,
        });
        let err = service.report_split(invalid_req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_service_check_and_recover_dead_nodes_with_epoch_tracking() {
        let service = DirectorServiceImpl::default_service();
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));

        // 1. Register node 1 (authoritative for root bounds)
        let reg_resp1 = service
            .register_node(Request::new(RegisterNodeRequest {
                node_id: 1,
                listen_addr: "127.0.0.1:50051".to_string(),
                bounds: Some(bounding_box_3d_to_proto(&bounds)),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(reg_resp1.topology_epoch, 1);

        // 2. Register node 2
        let reg_resp2 = service
            .register_node(Request::new(RegisterNodeRequest {
                node_id: 2,
                listen_addr: "127.0.0.1:50052".to_string(),
                bounds: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(reg_resp2.topology_epoch, 1);

        // 3. Heartbeat from node 1
        let hb_resp = service
            .heartbeat(Request::new(HeartbeatRequest {
                node_id: 1,
                tick_rate: 20.0,
                entity_count: 5,
                cpu_percent: 10.0,
                memory_bytes: 1024,
                timestamp: 1000,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(hb_resp.topology_epoch, 1);

        // 4. Split bounds across node 1 and 2
        let split_resp = service
            .report_split(Request::new(SplitReportRequest {
                node_id: 1,
                original_bounds: Some(bounding_box_3d_to_proto(&bounds)),
                child_bounds: vec![],
                total_entities: 60,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(split_resp.topology_epoch, 2);

        // 5. Expire node 1's heartbeat directly in registry
        {
            let mut reg = service.registry.write().await;
            reg.set_last_heartbeat(
                1,
                std::time::Instant::now() - std::time::Duration::from_secs(10),
            );
        }

        // 6. Recover dead nodes
        let recovery = service
            .check_and_recover_dead_nodes(std::time::Duration::from_secs(5))
            .await;
        assert!(!recovery.is_empty());
        assert_eq!(recovery[0].epoch, 3);
        assert_eq!(recovery[0].previous_node_id, 1);
        assert_eq!(recovery[0].new_node_id, 2);

        // 7. Verify subsequent heartbeat returns epoch 3
        let hb_node2 = service
            .heartbeat(Request::new(HeartbeatRequest {
                node_id: 2,
                tick_rate: 20.0,
                entity_count: 10,
                cpu_percent: 15.0,
                memory_bytes: 2048,
                timestamp: 2000,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(hb_node2.topology_epoch, 3);
    }

    #[tokio::test]
    async fn test_director_telemetry_metrics() {
        let telemetry = mmo_core::TelemetryServer::init().unwrap();
        let service = DirectorServiceImpl::default_service();

        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        let reg_req = Request::new(RegisterNodeRequest {
            node_id: 1,
            listen_addr: "127.0.0.1:50052".to_string(),
            bounds: Some(bounding_box_3d_to_proto(&bounds)),
        });

        service.register_node(reg_req).await.unwrap();

        let render_reg = telemetry.render_metrics();
        assert!(render_reg.contains("mmo_director_active_nodes"));
        assert!(render_reg.contains("mmo_director_topology_epoch"));

        // Register node 2 and trigger split and recovery
        let reg_req2 = Request::new(RegisterNodeRequest {
            node_id: 2,
            listen_addr: "127.0.0.1:50053".to_string(),
            bounds: None,
        });
        service.register_node(reg_req2).await.unwrap();

        service
            .report_split(Request::new(SplitReportRequest {
                node_id: 1,
                original_bounds: Some(bounding_box_3d_to_proto(&bounds)),
                child_bounds: vec![],
                total_entities: 100,
            }))
            .await
            .unwrap();

        {
            let mut reg = service.registry.write().await;
            reg.set_last_heartbeat(
                1,
                std::time::Instant::now() - std::time::Duration::from_secs(10),
            );
        }

        let recovery = service
            .check_and_recover_dead_nodes(std::time::Duration::from_secs(5))
            .await;
        assert!(!recovery.is_empty());

        let render_recovery = telemetry.render_metrics();
        assert!(render_recovery.contains("mmo_director_partition_recoveries_total"));
    }
}
