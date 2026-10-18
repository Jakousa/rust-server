use std::collections::HashMap;

use mmo_core::{BoundingBox3D, EntityStore, Guid, Octree};
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::{GhostEntity, GhostSyncRequest, GhostSyncResponse, Vector3};

use crate::network::client_pool::GrpcClientPool;
use crate::network::EntityVelocity;
use crate::sim::TickSimulation;

/// Ghost entity record stored locally on a neighbor partition for zero-latency client visibility.
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
        let dx = (pos.0 - bounds.min.0)
            .abs()
            .min((bounds.max.0 - pos.0).abs());
        let dy = (pos.1 - bounds.min.1)
            .abs()
            .min((bounds.max.1 - pos.1).abs());
        let dz = (pos.2 - bounds.min.2)
            .abs()
            .min((bounds.max.2 - pos.2).abs());
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

    /// Sends a [`GhostSyncRequest`] to a gRPC endpoint URL using a pooled connection channel.
    #[allow(clippy::result_large_err)]
    pub async fn send_sync_to_endpoint_pooled(
        pool: &GrpcClientPool,
        endpoint: &str,
        request: GhostSyncRequest,
    ) -> Result<GhostSyncResponse, tonic::Status> {
        let channel = pool
            .get_channel(endpoint)
            .map_err(|e| tonic::Status::unavailable(format!("Endpoint connection failed: {e}")))?;
        let mut client = NodeServiceClient::new(channel);
        let resp = client.sync_ghosts(request).await?;
        Ok(resp.into_inner())
    }

    /// Sends a [`GhostSyncRequest`] to a gRPC endpoint URL.
    #[allow(clippy::result_large_err)]
    pub async fn send_sync_to_endpoint(
        endpoint: &str,
        request: GhostSyncRequest,
    ) -> Result<GhostSyncResponse, tonic::Status> {
        let endpoint_str = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{}", endpoint)
        };
        let mut client = NodeServiceClient::connect(endpoint_str)
            .await
            .map_err(|e| tonic::Status::unavailable(format!("Endpoint connection failed: {e}")))?;
        let resp = client.sync_ghosts(request).await?;
        Ok(resp.into_inner())
    }
}
