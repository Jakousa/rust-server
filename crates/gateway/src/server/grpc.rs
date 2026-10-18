use bytes::Bytes;
use dashmap::DashMap;
use mmo_core::guid::Guid;
pub use mmo_schema::cluster::gateway_service_server::{GatewayService, GatewayServiceServer};
pub use mmo_schema::cluster::{
    BatchRouteUpdateRequest, BatchRouteUpdateResponse, DeliverBroadcastJobsRequest,
    DeliverBroadcastJobsResponse, MultiGatewayRouteBroadcastRequest,
    MultiGatewayRouteBroadcastResponse, ReassignNodeRoutesRequest, ReassignNodeRoutesResponse,
    RevokeGatewaySessionRequest, RevokeGatewaySessionResponse, RouteUpdateRequest,
    RouteUpdateResponse,
};
use std::sync::Arc;

use super::dispatcher::is_ephemeral_packet;
use super::sink::SessionSink;
use crate::router::SessionRouter;

pub struct GatewayServiceImpl {
    router: Arc<SessionRouter>,
    session_sinks: Option<Arc<DashMap<u64, SessionSink>>>,
}

impl GatewayServiceImpl {
    /// Creates a new `GatewayServiceImpl` backed by the given `SessionRouter`.
    pub fn new(router: Arc<SessionRouter>) -> Self {
        Self {
            router,
            session_sinks: None,
        }
    }

    /// Creates a new `GatewayServiceImpl` with router and session sinks.
    pub fn with_sinks(
        router: Arc<SessionRouter>,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
    ) -> Self {
        Self {
            router,
            session_sinks: Some(session_sinks),
        }
    }

    /// Wraps this implementation in a Tonic gRPC `GatewayServiceServer`.
    pub fn into_server(self) -> GatewayServiceServer<Self> {
        GatewayServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl GatewayService for GatewayServiceImpl {
    async fn update_route(
        &self,
        request: tonic::Request<RouteUpdateRequest>,
    ) -> Result<tonic::Response<RouteUpdateResponse>, tonic::Status> {
        let req = request.into_inner();
        let guid = Guid::from_raw(req.guid);
        self.router
            .update_route_with_session(guid, req.session_id, req.node_address);
        metrics::counter!("mmo_gateway_route_updates_total").increment(1);
        Ok(tonic::Response::new(RouteUpdateResponse {
            success: true,
            error_message: String::new(),
        }))
    }

    async fn update_batch_routes(
        &self,
        request: tonic::Request<BatchRouteUpdateRequest>,
    ) -> Result<tonic::Response<BatchRouteUpdateResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut count = 0;
        for u in req.updates {
            let guid = Guid::from_raw(u.guid);
            self.router
                .update_route_with_session(guid, u.session_id, u.node_address);
            count += 1;
        }
        metrics::counter!("mmo_gateway_batch_route_updates_total").increment(1);
        metrics::counter!("mmo_gateway_route_updates_total").increment(count as u64);
        Ok(tonic::Response::new(BatchRouteUpdateResponse {
            success: true,
            updated_count: count as u32,
            error_message: String::new(),
        }))
    }

    async fn reassign_node_routes(
        &self,
        request: tonic::Request<ReassignNodeRoutesRequest>,
    ) -> Result<tonic::Response<ReassignNodeRoutesResponse>, tonic::Status> {
        let req = request.into_inner();
        let count = self
            .router
            .reassign_node_routes(&req.dead_node_address, &req.new_node_address);
        metrics::counter!("mmo_gateway_route_reassignments_total").increment(count as u64);
        Ok(tonic::Response::new(ReassignNodeRoutesResponse {
            success: true,
            reassigned_count: count as u32,
        }))
    }

    async fn revoke_gateway_session(
        &self,
        request: tonic::Request<RevokeGatewaySessionRequest>,
    ) -> Result<tonic::Response<RevokeGatewaySessionResponse>, tonic::Status> {
        let req = request.into_inner();
        let session_id = req.session_id;
        let _ = self.router.remove_session(session_id);
        if let Some(ref sinks) = self.session_sinks {
            sinks.remove(&session_id);
            metrics::gauge!("mmo_gateway_active_sessions").set(sinks.len() as f64);
        }
        metrics::counter!("mmo_gateway_revoked_sessions_total").increment(1);
        Ok(tonic::Response::new(RevokeGatewaySessionResponse {
            success: true,
        }))
    }

    async fn broadcast_route(
        &self,
        request: tonic::Request<MultiGatewayRouteBroadcastRequest>,
    ) -> Result<tonic::Response<MultiGatewayRouteBroadcastResponse>, tonic::Status> {
        let req = request.into_inner();
        let guid = Guid::from_raw(req.guid);
        self.router
            .update_route_with_session(guid, req.session_id, req.new_node_address);
        metrics::counter!("mmo_gateway_broadcast_routes_received_total").increment(1);
        Ok(tonic::Response::new(MultiGatewayRouteBroadcastResponse {
            success: true,
            acknowledged_gateways: 1,
        }))
    }

    async fn deliver_broadcast_jobs(
        &self,
        request: tonic::Request<DeliverBroadcastJobsRequest>,
    ) -> Result<tonic::Response<DeliverBroadcastJobsResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut delivered = 0;
        let mut disconnected = Vec::new();
        if let Some(ref sinks) = self.session_sinks {
            for batch in req.jobs {
                if let Some(sink_entry) = sinks.get(&batch.session_id) {
                    let sink = sink_entry.value();
                    for pkt_bytes in batch.packets {
                        let bytes = Bytes::from(pkt_bytes);
                        let is_ephemeral = is_ephemeral_packet(&bytes);
                        let sent = if is_ephemeral {
                            sink.datagram_tx.try_send(bytes).is_ok()
                        } else {
                            sink.stream_tx.try_send(bytes).is_ok()
                        };
                        if sent {
                            delivered += 1;
                        }
                    }
                } else {
                    disconnected.push(batch.session_id);
                }
            }
        }
        metrics::counter!("mmo_gateway_broadcast_jobs_delivered_total").increment(delivered as u64);
        if !disconnected.is_empty() {
            metrics::counter!("mmo_gateway_broadcast_jobs_disconnected_total")
                .increment(disconnected.len() as u64);
        }
        Ok(tonic::Response::new(DeliverBroadcastJobsResponse {
            success: true,
            delivered_count: delivered,
            disconnected_sessions: disconnected,
        }))
    }
}
