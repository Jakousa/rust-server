//! Inter-node network services: Remote Command Forwarding and Ghost Broadcaster.
//!
//! Provides the gRPC [`NodeServiceImpl`] handling remote interaction forwarding and
//! ghost entity replication, along with [`GhostBroadcaster`] for boundary detection and synchronization.

use std::collections::HashMap;
use std::sync::Arc;

use mmo_core::guid::Guid;
use mmo_core::{BoundingBox3D, EntityStore, Event, Octree};
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::node_service_server::{NodeService, NodeServiceServer};
use mmo_schema::cluster::{
    GhostEntity, GhostSyncRequest, GhostSyncResponse, RemoteCommandRequest, RemoteCommandResponse,
    Vector3,
};

use crate::sim::TickSimulation;

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

/// Record representing a replicated ghost entity on a boundary node.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostRecord {
    pub guid: Guid,
    pub position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub yaw: f32,
    pub pitch: f32,
    pub component_data: Vec<u8>,
    pub last_tick: u64,
    pub source_node_id: u32,
}

/// Dedicated storage container for ghost entities replicated from neighboring nodes.
#[derive(Debug, Clone, Default)]
pub struct GhostStore {
    ghosts: HashMap<Guid, GhostRecord>,
}

impl GhostStore {
    /// Creates a new empty `GhostStore`.
    pub fn new() -> Self {
        Self {
            ghosts: HashMap::new(),
        }
    }

    /// Inserts or updates a ghost entity. Returns `true` if inserted, `false` if updated.
    pub fn insert_or_update(&mut self, record: GhostRecord) -> bool {
        let is_new = !self.ghosts.contains_key(&record.guid);
        self.ghosts.insert(record.guid, record);
        is_new
    }

    /// Upserts a ghost record from a protobuf [`GhostEntity`].
    pub fn upsert_from_proto(&mut self, source_node_id: u32, proto: &GhostEntity) -> bool {
        let pos = proto
            .position
            .as_ref()
            .map(|p| (p.x, p.y, p.z))
            .unwrap_or((0.0, 0.0, 0.0));
        let vel = proto
            .velocity
            .as_ref()
            .map(|v| (v.x, v.y, v.z))
            .unwrap_or((0.0, 0.0, 0.0));

        let record = GhostRecord {
            guid: Guid::from_raw(proto.guid),
            position: pos,
            velocity: vel,
            yaw: proto.yaw,
            pitch: proto.pitch,
            component_data: proto.component_data.clone(),
            last_tick: proto.last_tick,
            source_node_id,
        };
        self.insert_or_update(record)
    }

    /// Updates or inserts all ghosts contained in a [`GhostSyncRequest`].
    /// Returns the number of synced ghost entities.
    pub fn update_from_request(&mut self, req: &GhostSyncRequest) -> usize {
        let mut count = 0;
        for ghost in &req.ghosts {
            self.upsert_from_proto(req.source_node_id, ghost);
            count += 1;
        }
        count
    }

    /// Retrieves an immutable reference to a ghost entity by its GUID.
    pub fn get(&self, guid: Guid) -> Option<&GhostRecord> {
        self.ghosts.get(&guid)
    }

    /// Retrieves a mutable reference to a ghost entity by its GUID.
    pub fn get_mut(&mut self, guid: Guid) -> Option<&mut GhostRecord> {
        self.ghosts.get_mut(&guid)
    }

    /// Returns `true` if the ghost entity exists in this store.
    pub fn contains(&self, guid: Guid) -> bool {
        self.ghosts.contains_key(&guid)
    }

    /// Removes a ghost entity by its GUID, returning the removed record if found.
    pub fn remove(&mut self, guid: Guid) -> Option<GhostRecord> {
        self.ghosts.remove(&guid)
    }

    /// Returns the total number of ghost entities in storage.
    pub fn len(&self) -> usize {
        self.ghosts.len()
    }

    /// Returns `true` if the store contains no ghost entities.
    pub fn is_empty(&self) -> bool {
        self.ghosts.is_empty()
    }

    /// Clears all ghost entities from storage.
    pub fn clear(&mut self) {
        self.ghosts.clear();
    }

    /// Iterates over all ghost entities in storage.
    pub fn iter(&self) -> impl Iterator<Item = (&Guid, &GhostRecord)> {
        self.ghosts.iter()
    }

    /// Queries all ghost entities within radial distance from `center`.
    pub fn query_radius(&self, center: (f32, f32, f32), radius: f32) -> Vec<&GhostRecord> {
        let r_sq = radius * radius;
        self.ghosts
            .values()
            .filter(|g| {
                let dx = g.position.0 - center.0;
                let dy = g.position.1 - center.1;
                let dz = g.position.2 - center.2;
                dx * dx + dy * dy + dz * dz <= r_sq
            })
            .collect()
    }
}

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
}

impl NodeServiceImpl {
    /// Creates a new `NodeServiceImpl` sharing the simulation state.
    pub fn new(sim: Arc<tokio::sync::RwLock<TickSimulation>>) -> Self {
        Self { sim }
    }

    /// Creates a Tonic gRPC [`NodeServiceServer`] wrapping this implementation.
    pub fn into_server(self) -> NodeServiceServer<Self> {
        NodeServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl NodeService for NodeServiceImpl {
    /// Processes a forwarded command targeted at an entity residing on this authoritative node.
    ///
    /// Validates the target entity exists in `entity_store`, dispatches `Event::BeforeInteract`
    /// through `HookDispatcher` (vetoable), and if approved, dispatches `Event::OnInteract` and returns success.
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
}

/// Prepared broadcast payload ready to be sent to a neighboring node endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct BroadcastJob {
    pub target_node_id: u32,
    pub endpoint: String,
    pub request: GhostSyncRequest,
}

/// Boundary Ghost Broadcaster.
///
/// Identifies entities located near spatial boundary faces or adjacent octant borders,
/// constructs [`GhostSyncRequest`] payloads, and prepares them for transmission to neighboring node endpoints.
#[derive(Debug, Clone)]
pub struct GhostBroadcaster {
    pub node_id: u32,
    pub boundary_margin: f32,
    pub neighbor_endpoints: HashMap<u32, String>,
}

impl GhostBroadcaster {
    /// Creates a new `GhostBroadcaster` for the specified node with a boundary margin.
    pub fn new(node_id: u32, boundary_margin: f32) -> Self {
        Self {
            node_id,
            boundary_margin,
            neighbor_endpoints: HashMap::new(),
        }
    }

    /// Creates a new `GhostBroadcaster` with pre-configured neighbor endpoints.
    pub fn with_neighbors(
        node_id: u32,
        boundary_margin: f32,
        neighbor_endpoints: HashMap<u32, String>,
    ) -> Self {
        Self {
            node_id,
            boundary_margin,
            neighbor_endpoints,
        }
    }

    /// Registers or updates a neighbor node ID and its gRPC listen endpoint.
    pub fn add_neighbor(&mut self, node_id: u32, endpoint: impl Into<String>) {
        self.neighbor_endpoints.insert(node_id, endpoint.into());
    }

    /// Removes a neighbor node endpoint.
    pub fn remove_neighbor(&mut self, node_id: u32) -> Option<String> {
        self.neighbor_endpoints.remove(&node_id)
    }

    /// Returns a reference to all registered neighbor endpoints.
    pub fn neighbors(&self) -> &HashMap<u32, String> {
        &self.neighbor_endpoints
    }

    /// Calculates the minimum perpendicular distance from a position to any of the 6 bounding box faces.
    pub fn distance_to_bounds(bounds: &BoundingBox3D, pos: (f32, f32, f32)) -> f32 {
        let dx = (pos.0 - bounds.min.0).abs().min((bounds.max.0 - pos.0).abs());
        let dy = (pos.1 - bounds.min.1).abs().min((bounds.max.1 - pos.1).abs());
        let dz = (pos.2 - bounds.min.2).abs().min((bounds.max.2 - pos.2).abs());
        dx.min(dy).min(dz)
    }

    /// Determines whether an entity position is near an outer boundary face of the node's domain
    /// or near an internal boundary of subdivided octant leaves.
    pub fn is_entity_near_boundary(&self, octree: &Octree, pos: (f32, f32, f32)) -> bool {
        if self.boundary_margin <= 0.0 {
            return false;
        }

        // Check distance to outer boundaries of the node's domain
        if Self::distance_to_bounds(&octree.bounds, pos) <= self.boundary_margin {
            return true;
        }

        // Check if octree identifies boundary neighbors (internal leaf boundaries)
        !octree
            .find_boundary_neighbors(pos, self.boundary_margin)
            .is_empty()
    }

    /// Queries `Octree` and `EntityStore` to find all entities near octant boundary faces,
    /// converting them to [`GhostEntity`] protocol messages.
    pub fn find_boundary_entities(
        &self,
        octree: &Octree,
        entity_store: &EntityStore,
        tick: u64,
    ) -> Vec<GhostEntity> {
        let mut ghosts = Vec::new();

        for (_key, entity) in entity_store.iter() {
            if self.is_entity_near_boundary(octree, entity.position) {
                let vel = entity
                    .get_component::<EntityVelocity>()
                    .map(|v| Vector3 {
                        x: v.x,
                        y: v.y,
                        z: v.z,
                    })
                    .unwrap_or(Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    });

                ghosts.push(GhostEntity {
                    guid: entity.guid.raw(),
                    position: Some(Vector3 {
                        x: entity.position.0,
                        y: entity.position.1,
                        z: entity.position.2,
                    }),
                    velocity: Some(vel),
                    yaw: entity.rotation,
                    pitch: 0.0,
                    component_data: Vec::new(),
                    last_tick: tick,
                });
            }
        }

        ghosts
    }

    /// Builds a [`GhostSyncRequest`] from a list of ghost entities.
    pub fn build_sync_request(&self, tick: u64, ghosts: Vec<GhostEntity>) -> GhostSyncRequest {
        GhostSyncRequest {
            source_node_id: self.node_id,
            ghosts,
            tick,
        }
    }

    /// Collects boundary entities and constructs a [`GhostSyncRequest`].
    pub fn collect_sync_request(
        &self,
        octree: &Octree,
        entity_store: &EntityStore,
        tick: u64,
    ) -> GhostSyncRequest {
        let ghosts = self.find_boundary_entities(octree, entity_store, tick);
        self.build_sync_request(tick, ghosts)
    }

    /// Collects boundary entities directly from a [`TickSimulation`].
    pub fn collect_sync_from_sim(&self, sim: &TickSimulation) -> GhostSyncRequest {
        self.collect_sync_request(&sim.octree, &sim.entity_store, sim.tick_counter)
    }

    /// Prepares broadcast jobs for all registered neighbor endpoints using entities from octree and entity store.
    pub fn prepare_broadcast(
        &self,
        octree: &Octree,
        entity_store: &EntityStore,
        tick: u64,
    ) -> Vec<BroadcastJob> {
        let req = self.collect_sync_request(octree, entity_store, tick);
        self.neighbor_endpoints
            .iter()
            .map(|(&target_node_id, endpoint)| BroadcastJob {
                target_node_id,
                endpoint: endpoint.clone(),
                request: req.clone(),
            })
            .collect()
    }

    /// Prepares broadcast jobs directly from a [`TickSimulation`].
    pub fn prepare_broadcast_from_sim(&self, sim: &TickSimulation) -> Vec<BroadcastJob> {
        self.prepare_broadcast(&sim.octree, &sim.entity_store, sim.tick_counter)
    }

    /// Sends a [`GhostSyncRequest`] using an established Tonic client channel.
    #[allow(clippy::result_large_err)]
    pub async fn send_sync_to_channel(
        client: &mut NodeServiceClient<tonic::transport::Channel>,
        request: GhostSyncRequest,
    ) -> Result<GhostSyncResponse, tonic::Status> {
        let resp = client.sync_ghosts(request).await?;
        Ok(resp.into_inner())
    }

    /// Sends a [`GhostSyncRequest`] to a gRPC endpoint URL.
    #[allow(clippy::result_large_err)]
    pub async fn send_sync_to_endpoint(
        endpoint: &str,
        request: GhostSyncRequest,
    ) -> Result<GhostSyncResponse, tonic::Status> {
        let mut client = NodeServiceClient::connect(endpoint.to_string())
            .await
            .map_err(|e| tonic::Status::unavailable(format!("Endpoint connection failed: {e}")))?;
        let resp = client.sync_ghosts(request).await?;
        Ok(resp.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

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
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(service.into_server())
                .serve(addr)
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
}
