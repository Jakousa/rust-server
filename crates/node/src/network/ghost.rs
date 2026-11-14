use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use mmo_core::world::SectorCoord3D;
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
    pub local_tick: u64,
}

impl GhostRecord {
    /// Creates a new `GhostRecord` with `local_tick = 0`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        guid: Guid,
        position: (f32, f32, f32),
        velocity: (f32, f32, f32),
        yaw: f32,
        pitch: f32,
        component_data: Vec<u8>,
        last_tick: u64,
        source_node_id: u32,
    ) -> Self {
        Self {
            guid,
            position,
            velocity,
            yaw,
            pitch,
            component_data,
            last_tick,
            source_node_id,
            local_tick: 0,
        }
    }
}

impl Default for GhostRecord {
    fn default() -> Self {
        Self {
            guid: Guid::from_raw(0),
            position: (0.0, 0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 0,
            source_node_id: 0,
            local_tick: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct GhostStoreInner {
    ghosts: HashMap<Guid, GhostRecord>,
    sectors: HashMap<SectorCoord3D, HashSet<Guid>>,
    ghost_sectors: HashMap<Guid, SectorCoord3D>,
}

impl GhostStoreInner {
    fn upsert_record(&mut self, record: GhostRecord) -> bool {
        let guid = record.guid;
        let new_sector = SectorCoord3D::from_world_pos(record.position);
        if let Some(old_sector) = self.ghost_sectors.insert(guid, new_sector) {
            if old_sector != new_sector {
                if let Some(set) = self.sectors.get_mut(&old_sector) {
                    set.remove(&guid);
                    if set.is_empty() {
                        self.sectors.remove(&old_sector);
                    }
                }
                self.sectors.entry(new_sector).or_default().insert(guid);
            }
        } else {
            self.sectors.entry(new_sector).or_default().insert(guid);
        }
        self.ghosts.insert(guid, record).is_none()
    }

    fn remove_record(&mut self, guid: Guid) -> Option<GhostRecord> {
        if let Some(sector) = self.ghost_sectors.remove(&guid) {
            if let Some(set) = self.sectors.get_mut(&sector) {
                set.remove(&guid);
                if set.is_empty() {
                    self.sectors.remove(&sector);
                }
            }
        }
        self.ghosts.remove(&guid)
    }

    fn clear(&mut self) {
        self.ghosts.clear();
        self.sectors.clear();
        self.ghost_sectors.clear();
    }
}

/// Dedicated storage container for ghost entities replicated from neighboring nodes.
///
/// Internally synchronized with `std::sync::RwLock` to avoid write-lock contention against the
/// main simulation tick loop.
#[derive(Debug, Clone, Default)]
pub struct GhostStore {
    inner: Arc<RwLock<GhostStoreInner>>,
    current_tick: Arc<AtomicU64>,
}

impl GhostStore {
    /// Creates a new empty `GhostStore`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(GhostStoreInner::default())),
            current_tick: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Advances the current simulation tick tracked by this ghost store.
    pub fn advance_tick(&self, tick: u64) {
        self.current_tick.store(tick, Ordering::Relaxed);
    }

    /// Returns the current simulation tick tracked by this ghost store.
    pub fn current_tick(&self) -> u64 {
        self.current_tick.load(Ordering::Relaxed)
    }

    /// Inserts or updates a ghost entity. Returns `true` if inserted, `false` if updated.
    pub fn insert_or_update(&self, mut record: GhostRecord) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if record.local_tick == 0 {
            record.local_tick = self.current_tick();
        }
        inner.upsert_record(record)
    }

    /// Internal helper upserting a single proto ghost into [`GhostStoreInner`] with finite validation.
    fn upsert_proto_into_inner(
        inner: &mut GhostStoreInner,
        source_node_id: u32,
        proto: &GhostEntity,
        current_tick: u64,
    ) -> bool {
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

        if !pos.0.is_finite()
            || !pos.1.is_finite()
            || !pos.2.is_finite()
            || !vel.0.is_finite()
            || !vel.1.is_finite()
            || !vel.2.is_finite()
            || !proto.yaw.is_finite()
            || !proto.pitch.is_finite()
        {
            return false;
        }

        let guid = Guid::from_raw(proto.guid);
        let record = GhostRecord {
            guid,
            position: pos,
            velocity: vel,
            yaw: proto.yaw,
            pitch: proto.pitch,
            component_data: proto.component_data.clone(),
            last_tick: proto.last_tick,
            source_node_id,
            local_tick: current_tick,
        };
        inner.upsert_record(record);
        true
    }

    /// Upserts a ghost record from a protobuf [`GhostEntity`].
    pub fn upsert_from_proto(&self, source_node_id: u32, proto: &GhostEntity) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        Self::upsert_proto_into_inner(&mut inner, source_node_id, proto, self.current_tick())
    }

    /// Updates or inserts all ghosts contained in a [`GhostSyncRequest`].
    /// Returns the number of synced ghost entities.
    pub fn update_from_request(&self, req: &GhostSyncRequest) -> usize {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let tick = self.current_tick();
        let mut count = 0;
        for ghost in &req.ghosts {
            if Self::upsert_proto_into_inner(&mut inner, req.source_node_id, ghost, tick) {
                count += 1;
            }
        }
        count
    }

    /// Prunes all ghost records that have not been refreshed within `ttl_ticks`.
    ///
    /// Returns the count of pruned records.
    pub fn prune_stale(&self, current_tick: u64, ttl_ticks: u64) -> usize {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let before = inner.ghosts.len();
        let GhostStoreInner {
            ghosts,
            sectors,
            ghost_sectors,
        } = &mut *inner;
        ghosts.retain(|guid, record| {
            let keep = current_tick.saturating_sub(record.local_tick) < ttl_ticks;
            if !keep {
                if let Some(sector) = ghost_sectors.remove(guid) {
                    if let Some(set) = sectors.get_mut(&sector) {
                        set.remove(guid);
                        if set.is_empty() {
                            sectors.remove(&sector);
                        }
                    }
                }
            }
            keep
        });
        let pruned = before - inner.ghosts.len();
        if pruned > 0 {
            metrics::counter!("mmo_node_ghosts_pruned_total").increment(pruned as u64);
        }
        pruned
    }

    /// Retrieves an immutable copy of a ghost entity by its GUID.
    pub fn get(&self, guid: Guid) -> Option<GhostRecord> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.ghosts.get(&guid).cloned()
    }

    /// Returns `true` if the ghost entity exists in this store.
    pub fn contains(&self, guid: Guid) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.ghosts.contains_key(&guid)
    }

    /// Removes a ghost entity by its GUID, returning the removed record if found.
    pub fn remove(&self, guid: Guid) -> Option<GhostRecord> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.remove_record(guid)
    }

    /// Returns the total number of ghost entities in storage.
    pub fn len(&self) -> usize {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.ghosts.len()
    }

    /// Returns `true` if the store contains no ghost entities.
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.ghosts.is_empty()
    }

    /// Clears all ghost entities from storage.
    pub fn clear(&self) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.clear();
    }

    /// Queries all ghost entities within radial distance from `center` using the spatial sector index.
    pub fn query_radius(&self, center: (f32, f32, f32), radius: f32) -> Vec<GhostRecord> {
        if radius < 0.0
            || !radius.is_finite()
            || !center.0.is_finite()
            || !center.1.is_finite()
            || !center.2.is_finite()
        {
            return Vec::new();
        }
        let r_sq = radius * radius;
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if inner.ghosts.is_empty() {
            return Vec::new();
        }

        let min_sec = SectorCoord3D::from_world_pos((
            center.0 - radius,
            center.1 - radius,
            center.2 - radius,
        ));
        let max_sec = SectorCoord3D::from_world_pos((
            center.0 + radius,
            center.1 + radius,
            center.2 + radius,
        ));
        let span_x = (max_sec.x as i64 - min_sec.x as i64 + 1).max(0) as u64;
        let span_y = (max_sec.y as i64 - min_sec.y as i64 + 1).max(0) as u64;
        let span_z = (max_sec.z as i64 - min_sec.z as i64 + 1).max(0) as u64;
        let sector_volume = span_x.saturating_mul(span_y).saturating_mul(span_z);

        let mut results = Vec::new();
        if sector_volume <= inner.sectors.len() as u64 {
            for sx in min_sec.x..=max_sec.x {
                for sy in min_sec.y..=max_sec.y {
                    for sz in min_sec.z..=max_sec.z {
                        let sector = SectorCoord3D::new(sx, sy, sz);
                        if !sector.bounds().intersects_sphere(center, radius) {
                            continue;
                        }
                        if let Some(guids) = inner.sectors.get(&sector) {
                            for guid in guids {
                                if let Some(g) = inner.ghosts.get(guid) {
                                    let dx = g.position.0 - center.0;
                                    let dy = g.position.1 - center.1;
                                    let dz = g.position.2 - center.2;
                                    if dx * dx + dy * dy + dz * dz <= r_sq {
                                        results.push(g.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
            for (&sector, guids) in &inner.sectors {
                if !sector.bounds().intersects_sphere(center, radius) {
                    continue;
                }
                for guid in guids {
                    if let Some(g) = inner.ghosts.get(guid) {
                        let dx = g.position.0 - center.0;
                        let dy = g.position.1 - center.1;
                        let dz = g.position.2 - center.2;
                        if dx * dx + dy * dy + dz * dz <= r_sq {
                            results.push(g.clone());
                        }
                    }
                }
            }
        }
        results
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
        octree.has_boundary_neighbors(pos, self.boundary_margin)
    }

    fn build_ghost_entity(entity: &mmo_core::Entity, tick: u64) -> GhostEntity {
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

        GhostEntity {
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
        }
    }

    /// Queries `Octree` and `EntityStore` to find all entities near octant boundary faces,
    /// converting them to [`GhostEntity`] protocol messages.
    pub fn find_boundary_entities(
        &self,
        octree: &Octree,
        entity_store: &EntityStore,
        tick: u64,
    ) -> Vec<GhostEntity> {
        if self.boundary_margin <= 0.0 || entity_store.count() == 0 {
            return Vec::new();
        }

        let mut ghosts = Vec::new();

        // Fast path: prune entire interior subtrees directly in the octree when synchronized.
        if !octree.is_empty() && octree.len() == entity_store.count() {
            let mut candidate_guids = Vec::new();
            octree.collect_boundary_entities(self.boundary_margin, &mut candidate_guids);
            ghosts.reserve(candidate_guids.len());
            for guid in candidate_guids {
                if let Some(entity) = entity_store.get_by_guid(guid) {
                    ghosts.push(Self::build_ghost_entity(entity, tick));
                }
            }
            return ghosts;
        }

        for (_key, entity) in entity_store.iter() {
            if self.is_entity_near_boundary(octree, entity.position) {
                ghosts.push(Self::build_ghost_entity(entity, tick));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ghost_store_lifecycle() {
        let store = GhostStore::new();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());

        let g1 = Guid::from_raw(101);
        let record = GhostRecord {
            guid: g1,
            position: (10.0, 20.0, 30.0),
            velocity: (1.0, 0.0, 0.0),
            yaw: 0.5,
            pitch: 0.1,
            component_data: vec![1, 2, 3],
            last_tick: 50,
            source_node_id: 2,
            local_tick: 0,
        };

        // Insert and verify
        assert!(store.insert_or_update(record.clone()));
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert!(store.contains(g1));
        assert_eq!(store.get(g1), Some(record.clone()));
        assert_eq!(store.get(g1).unwrap().last_tick, 50);

        // Update returns false (not new)
        let mut updated = record.clone();
        updated.last_tick = 51;
        assert!(!store.insert_or_update(updated));
        assert_eq!(store.get(g1).unwrap().last_tick, 51);

        // Query radius
        let nearby = store.query_radius((10.0, 20.0, 30.0), 5.0);
        assert_eq!(nearby.len(), 1);
        let far = store.query_radius((100.0, 100.0, 100.0), 5.0);
        assert_eq!(far.len(), 0);

        // Upsert from protobuf GhostEntity
        let proto = GhostEntity {
            guid: 102,
            position: Some(Vector3 {
                x: 5.0,
                y: 5.0,
                z: 5.0,
            }),
            velocity: Some(Vector3 {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            }),
            yaw: 1.0,
            pitch: 0.0,
            component_data: vec![],
            last_tick: 52,
        };
        assert!(store.upsert_from_proto(3, &proto));
        assert_eq!(store.len(), 2);

        // Update from GhostSyncRequest
        let req = GhostSyncRequest {
            source_node_id: 3,
            ghosts: vec![proto],
            tick: 53,
        };
        assert_eq!(store.update_from_request(&req), 1);

        // Remove and clear
        let removed = store.remove(g1).expect("removed g1");
        assert_eq!(removed.guid, g1);
        assert_eq!(store.len(), 1);
        store.clear();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_ghost_store_tick_based_ttl_pruning() {
        let store = GhostStore::new();
        store.advance_tick(10);

        let g_stale = Guid::from_raw(201);
        let g_fresh = Guid::from_raw(202);

        // Insert stale ghost at local_tick = 10
        assert!(store.insert_or_update(GhostRecord {
            guid: g_stale,
            position: (1.0, 1.0, 1.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: vec![],
            last_tick: 1,
            source_node_id: 2,
            local_tick: 0, // will be auto-stamped to store.current_tick() = 10
        }));

        // Advance tick to 25
        store.advance_tick(25);

        // Insert fresh ghost at local_tick = 25
        assert!(store.insert_or_update(GhostRecord {
            guid: g_fresh,
            position: (2.0, 2.0, 2.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: vec![],
            last_tick: 2,
            source_node_id: 2,
            local_tick: 0, // will be stamped to 25
        }));

        assert_eq!(store.len(), 2);

        // Prune with TTL = 10 at tick 25:
        // g_stale delta is 25 - 10 = 15 >= 10 -> pruned
        // g_fresh delta is 25 - 25 = 0 < 10 -> kept
        let pruned = store.prune_stale(25, 10);
        assert_eq!(pruned, 1);
        assert_eq!(store.len(), 1);
        assert!(!store.contains(g_stale));
        assert!(store.contains(g_fresh));
    }

    #[test]
    fn test_ghost_store_concurrent_access() {
        let store = GhostStore::new();
        let store_clone = store.clone();

        let handle = std::thread::spawn(move || {
            for i in 0..100 {
                let guid = Guid::from_raw(1000 + i);
                store_clone.insert_or_update(GhostRecord {
                    guid,
                    position: (i as f32, 0.0, 0.0),
                    velocity: (0.0, 0.0, 0.0),
                    yaw: 0.0,
                    pitch: 0.0,
                    component_data: vec![],
                    last_tick: i,
                    source_node_id: 2,
                    local_tick: 0,
                });
            }
        });

        // Concurrent reader
        for _ in 0..50 {
            let _ = store.query_radius((0.0, 0.0, 0.0), 50.0);
        }

        handle.join().unwrap();
        assert_eq!(store.len(), 100);
    }

    #[test]
    fn test_ghost_store_upsert_rejects_non_finite_floats() {
        let store = GhostStore::new();

        // 1. Ghost with NaN in position
        let proto_nan_pos = GhostEntity {
            guid: 501,
            position: Some(Vector3 {
                x: f32::NAN,
                y: 1.0,
                z: 2.0,
            }),
            velocity: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            yaw: 0.0,
            pitch: 0.0,
            component_data: vec![],
            last_tick: 10,
        };
        assert!(!store.upsert_from_proto(1, &proto_nan_pos));
        assert!(store.is_empty());

        // 2. Ghost with Infinity in yaw
        let proto_inf_yaw = GhostEntity {
            guid: 502,
            position: Some(Vector3 {
                x: 0.0,
                y: 1.0,
                z: 2.0,
            }),
            velocity: Some(Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            yaw: f32::INFINITY,
            pitch: 0.0,
            component_data: vec![],
            last_tick: 10,
        };
        assert!(!store.upsert_from_proto(1, &proto_inf_yaw));
        assert!(store.is_empty());
    }

    #[test]
    fn test_ghost_broadcaster_boundary_detection_and_preparation() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let octree = Octree::new(bounds);
        let mut entity_store = EntityStore::new();

        // Distance to bounds
        assert_eq!(
            GhostBroadcaster::distance_to_bounds(&bounds, (50.0, 50.0, 50.0)),
            50.0
        );
        assert_eq!(
            GhostBroadcaster::distance_to_bounds(&bounds, (2.0, 50.0, 50.0)),
            2.0
        );

        let mut broadcaster = GhostBroadcaster::new(1, 10.0);
        assert_eq!(broadcaster.node_id, 1);
        assert_eq!(broadcaster.boundary_margin, 10.0);

        // Non-positive margin returns false
        let no_margin = GhostBroadcaster::new(1, 0.0);
        assert!(!no_margin.is_entity_near_boundary(&octree, (1.0, 50.0, 50.0)));

        // Boundary margin check
        assert!(broadcaster.is_entity_near_boundary(&octree, (5.0, 50.0, 50.0)));
        assert!(!broadcaster.is_entity_near_boundary(&octree, (50.0, 50.0, 50.0)));

        // Add neighbor endpoints
        broadcaster.add_neighbor(2, "127.0.0.1:50052");
        assert_eq!(broadcaster.neighbors().len(), 1);

        // Spawn near-boundary entity
        let g1 = Guid::from_raw(201);
        entity_store.spawn(g1, (5.0, 50.0, 50.0));
        // Spawn center entity
        let g2 = Guid::from_raw(202);
        entity_store.spawn(g2, (50.0, 50.0, 50.0));

        let boundary_ghosts = broadcaster.find_boundary_entities(&octree, &entity_store, 100);
        assert_eq!(boundary_ghosts.len(), 1);
        assert_eq!(boundary_ghosts[0].guid, 201);

        let jobs = broadcaster.prepare_broadcast(&octree, &entity_store, 100);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].target_node_id, 2);
        assert_eq!(jobs[0].endpoint, "127.0.0.1:50052");
        assert_eq!(jobs[0].request.ghosts.len(), 1);

        assert_eq!(
            broadcaster.remove_neighbor(2),
            Some("127.0.0.1:50052".to_string())
        );
        assert_eq!(broadcaster.neighbors().len(), 0);
    }

    #[test]
    fn test_ghost_store_spatial_sector_index_move_prune_remove() {
        let store = GhostStore::new();
        store.advance_tick(10);

        let g1 = Guid::from_raw(101);
        let g2 = Guid::from_raw(102);
        let g3 = Guid::from_raw(103);

        // Sector (0,0,0) and adjacent sector (1,0,0) and distant sector (5,0,0)
        assert!(store.insert_or_update(GhostRecord::new(
            g1,
            (10.0, 10.0, 10.0),
            (0.0, 0.0, 0.0),
            0.0,
            0.0,
            Vec::new(),
            1,
            2,
        )));
        assert!(store.insert_or_update(GhostRecord::new(
            g2,
            (66.0, 10.0, 10.0),
            (0.0, 0.0, 0.0),
            0.0,
            0.0,
            Vec::new(),
            1,
            2,
        )));
        assert!(store.insert_or_update(GhostRecord::new(
            g3,
            (350.0, 10.0, 10.0),
            (0.0, 0.0, 0.0),
            0.0,
            0.0,
            Vec::new(),
            1,
            2,
        )));

        // Query centered at (50, 10, 10) with r = 45 hits g1 (dist 40) and g2 (dist 16)
        let mut res: Vec<u64> = store
            .query_radius((50.0, 10.0, 10.0), 45.0)
            .into_iter()
            .map(|r| r.guid.raw())
            .collect();
        res.sort_unstable();
        assert_eq!(res, vec![101, 102]);

        // Move g3 from sector (5,0,0) into sector (0,0,0) near (50,10,10) -> sector index updates
        assert!(!store.insert_or_update(GhostRecord::new(
            g3,
            (48.0, 10.0, 10.0),
            (0.0, 0.0, 0.0),
            0.0,
            0.0,
            Vec::new(),
            2,
            2,
        )));
        // Large radius spanning many empty sectors falls back cleanly to non-empty sector scan
        let mut large_res: Vec<u64> = store
            .query_radius((50.0, 10.0, 10.0), 2000.0)
            .into_iter()
            .map(|r| r.guid.raw())
            .collect();
        large_res.sort_unstable();
        assert_eq!(large_res, vec![101, 102, 103]);

        // Remove g2 and prune g1 (tick 10 vs 15 with ttl=4 -> g1 pruned, g3 refreshed at tick 14 stays)
        assert!(store.remove(g2).is_some());
        store.advance_tick(14);
        assert!(!store.insert_or_update(GhostRecord::new(
            g3,
            (48.0, 10.0, 10.0),
            (0.0, 0.0, 0.0),
            0.0,
            0.0,
            Vec::new(),
            3,
            2,
        )));
        assert_eq!(store.prune_stale(15, 4), 1);

        let after_prune: Vec<u64> = store
            .query_radius((50.0, 10.0, 10.0), 100.0)
            .into_iter()
            .map(|r| r.guid.raw())
            .collect();
        assert_eq!(after_prune, vec![103]);
    }

    #[test]
    fn test_ghost_broadcaster_find_boundary_entities_populated_octree() {
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);
        let mut entity_store = EntityStore::new();

        // Populate >16 entities across multiple octants (<=16 per octant so leaves stay 50x50x50).
        // Entities near center of an octant (e.g. (25, 25, 25) or (75, 75, 75)) are strictly interior;
        // entity at (4.0, 25.0, 25.0) is near outer x=0 boundary;
        // entity at (49.0, 25.0, 25.0) is near internal x=50 split face.
        let mut expected_boundary = Vec::new();
        for i in 1..=20u64 {
            let guid = Guid::from_raw(i);
            let pos = if i == 1 {
                expected_boundary.push(i);
                (4.0, 25.0, 25.0)
            } else if i == 2 {
                expected_boundary.push(i);
                (49.0, 25.0, 25.0)
            } else if i <= 14 {
                // Cluster deep inside octant (0..50, 0..50, 0..50), well away from 0 and 50 faces
                (20.0 + (i as f32) * 0.2, 25.0, 25.0)
            } else {
                // Cluster deep inside octant (50..100, 50..100, 50..100), well away from 50 and 100 faces
                (72.0 + ((i - 14) as f32) * 0.2, 75.0, 75.0)
            };
            entity_store.spawn(guid, pos);
            octree.insert(guid, pos);
        }
        assert!(octree.is_branch());

        let broadcaster = GhostBroadcaster::new(1, 5.0);
        let mut found: Vec<u64> = broadcaster
            .find_boundary_entities(&octree, &entity_store, 50)
            .into_iter()
            .map(|g| g.guid)
            .collect();
        found.sort_unstable();
        assert_eq!(found, expected_boundary);
    }
}
