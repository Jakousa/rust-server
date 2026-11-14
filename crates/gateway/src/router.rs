//! Dynamic Session Routing Table for MMO Edge Gateway.
//!
//! Maps client sessions (`session_id: u64`) and player entities (`player_guid: Guid`)
//! to authoritative simulation node network addresses (`String`), facilitating
//! dynamic, seamless packet forwarding and boundary handovers with lock-free sharded concurrency.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::RwLock;

use dashmap::DashMap;
use mmo_core::guid::Guid;
use mmo_core::BoundingBox3D;

/// Spatial partition route mapping a 3D bounding box to an authoritative simulation node.
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionRoute {
    pub bounds: BoundingBox3D,
    pub node_id: u32,
    pub node_address: String,
}

fn collect_unique_addresses<'a>(
    partitions: impl Iterator<Item = &'a PartitionRoute>,
) -> Vec<String> {
    let mut unique = Vec::new();
    for partition in partitions {
        let trimmed = partition.node_address.trim();
        if !trimmed.is_empty() && !unique.iter().any(|addr: &String| addr == trimmed) {
            unique.push(trimmed.to_string());
        }
    }
    unique
}

/// Concurrent session routing table mapping sessions and player GUIDs to authoritative node addresses.
///
/// Backed by [`DashMap`] for fine-grained sharded concurrency, eliminating reader-writer lock
/// contention on high-throughput packet forwarding paths (~100,000 packets/sec).
#[derive(Debug, Default)]
pub struct SessionRouter {
    /// Maps client session ID to player GUID.
    session_to_guid: DashMap<u64, Guid>,
    /// Maps player GUID to client session ID.
    guid_to_session: DashMap<Guid, u64>,
    /// Maps player GUID to authoritative simulation node address.
    guid_to_node: DashMap<Guid, String>,
    /// Maps client session ID directly to authoritative simulation node address.
    session_to_node: DashMap<u64, String>,
    /// Maps client session ID to authoritative session epoch.
    session_to_epoch: DashMap<u64, u64>,
    /// Authoritative cluster spatial partitions synchronized from the Director.
    partitions: RwLock<Vec<PartitionRoute>>,
    /// Latest topology epoch observed from the Director.
    topology_epoch: AtomicU64,
    /// Round-robin cursor across active/boundary-sharing simulation nodes.
    next_node_rr: AtomicUsize,
}

impl Clone for SessionRouter {
    fn clone(&self) -> Self {
        let router = Self::new();
        for item in self.session_to_guid.iter() {
            router.session_to_guid.insert(*item.key(), *item.value());
        }
        for item in self.guid_to_session.iter() {
            router.guid_to_session.insert(*item.key(), *item.value());
        }
        for item in self.guid_to_node.iter() {
            router
                .guid_to_node
                .insert(*item.key(), item.value().clone());
        }
        for item in self.session_to_node.iter() {
            router
                .session_to_node
                .insert(*item.key(), item.value().clone());
        }
        for item in self.session_to_epoch.iter() {
            router.session_to_epoch.insert(*item.key(), *item.value());
        }
        let partitions = self
            .partitions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        *router.partitions.write().unwrap_or_else(|e| e.into_inner()) = partitions;
        router.topology_epoch.store(
            self.topology_epoch.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        router
            .next_node_rr
            .store(self.next_node_rr.load(Ordering::Relaxed), Ordering::Relaxed);
        router
    }
}

impl PartialEq for SessionRouter {
    fn eq(&self, other: &Self) -> bool {
        if self.session_count() != other.session_count() {
            return false;
        }
        if self.topology_epoch.load(Ordering::Relaxed)
            != other.topology_epoch.load(Ordering::Relaxed)
        {
            return false;
        }
        for item in self.session_to_guid.iter() {
            if other.session_to_guid.get(item.key()).map(|r| *r.value()) != Some(*item.value()) {
                return false;
            }
        }
        let self_parts = self.partitions.read().unwrap_or_else(|e| e.into_inner());
        let other_parts = other.partitions.read().unwrap_or_else(|e| e.into_inner());
        *self_parts == *other_parts
    }
}

impl Eq for SessionRouter {}

impl SessionRouter {
    /// Creates a new empty `SessionRouter`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a client session with its associated player GUID and authoritative node address.
    ///
    /// Follows strict multi-map mutation ordering: updates identity maps first,
    /// and destination routes last, preventing transient routing anomalies.
    pub fn register_session(&self, session_id: u64, guid: Guid, node_address: String) {
        // If session_id was already mapped to a different guid, clean up stale reverse mapping
        if let Some((_, old_guid)) = self.session_to_guid.remove(&session_id) {
            if old_guid != guid {
                self.guid_to_node.remove(&old_guid);
                self.guid_to_session.remove(&old_guid);
            }
        }

        // If guid was already mapped to a different session_id, clean up stale reverse mapping
        if let Some((_, old_sid)) = self.guid_to_session.remove(&guid) {
            if old_sid != session_id {
                self.session_to_node.remove(&old_sid);
                self.session_to_epoch.remove(&old_sid);
                self.session_to_guid.remove(&old_sid);
            }
        }

        // 1. Identity maps first
        self.session_to_guid.insert(session_id, guid);
        self.guid_to_session.insert(guid, session_id);

        // 2. Routing destinations last
        self.guid_to_node.insert(guid, node_address.clone());
        self.session_to_node.insert(session_id, node_address);
    }

    /// Registers a fallback route for a session before a player GUID is identified.
    pub fn register_session_fallback(&self, session_id: u64, node_address: String) {
        self.session_to_node.insert(session_id, node_address);
    }

    /// Updates the authoritative node address for a player GUID during seamless handovers.
    ///
    /// Updates both GUID and session-based lookups. Returns `true` if the route
    /// previously existed and was updated, or `false` if it was newly added.
    pub fn update_route(&self, guid: Guid, new_node_address: String) -> bool {
        let existed = self.guid_to_node.contains_key(&guid);
        if let Some(session_ref) = self.guid_to_session.get(&guid) {
            let session_id = *session_ref.value();
            drop(session_ref);
            self.session_to_node
                .insert(session_id, new_node_address.clone());
        }
        self.guid_to_node.insert(guid, new_node_address);
        existed
    }

    /// Updates the authoritative node address for a player GUID and explicit session ID.
    pub fn update_route_with_session(
        &self,
        guid: Guid,
        session_id: u64,
        new_node_address: String,
    ) -> bool {
        let existed = self.guid_to_node.contains_key(&guid);
        if session_id != 0 {
            if let Some((_, old_guid)) = self.session_to_guid.remove(&session_id) {
                if old_guid != guid {
                    self.guid_to_node.remove(&old_guid);
                    self.guid_to_session.remove(&old_guid);
                }
            }
            if let Some((_, old_sid)) = self.guid_to_session.remove(&guid) {
                if old_sid != session_id {
                    self.session_to_node.remove(&old_sid);
                    self.session_to_epoch.remove(&old_sid);
                    self.session_to_guid.remove(&old_sid);
                }
            }
            self.session_to_guid.insert(session_id, guid);
            self.guid_to_session.insert(guid, session_id);
            self.session_to_node
                .insert(session_id, new_node_address.clone());
        } else if let Some(session_ref) = self.guid_to_session.get(&guid) {
            let sid = *session_ref.value();
            drop(session_ref);
            self.session_to_node.insert(sid, new_node_address.clone());
        }
        self.guid_to_node.insert(guid, new_node_address);
        existed
    }

    /// Reassigns all routes pointing to a dead or evacuated node to a new surviving node.
    ///
    /// This prevents traffic blackholing when an entire node crashes or terminates.
    /// Returns the number of routes reassigned.
    pub fn reassign_node_routes(&self, dead_node_address: &str, new_node_address: &str) -> usize {
        let mut count = 0;
        for mut entry in self.guid_to_node.iter_mut() {
            if entry.value() == dead_node_address {
                *entry.value_mut() = new_node_address.to_string();
                count += 1;
            }
        }
        for mut entry in self.session_to_node.iter_mut() {
            if entry.value() == dead_node_address {
                *entry.value_mut() = new_node_address.to_string();
                count += 1;
            }
        }
        count
    }

    /// Resolves the authoritative node address for a given player GUID.
    pub fn get_route_by_guid(&self, guid: Guid) -> Option<String> {
        self.guid_to_node.get(&guid).map(|s| s.value().clone())
    }

    /// Resolves the authoritative node address for a given client session ID.
    pub fn get_route_by_session(&self, session_id: u64) -> Option<String> {
        if let Some(r) = self.session_to_node.get(&session_id) {
            return Some(r.value().clone());
        }
        if let Some(guid_ref) = self.session_to_guid.get(&session_id) {
            let guid = *guid_ref.value();
            drop(guid_ref);
            return self.guid_to_node.get(&guid).map(|s| s.value().clone());
        }
        None
    }

    /// Resolves the authoritative session epoch for a given client session ID.
    pub fn get_epoch_by_session(&self, session_id: u64) -> Option<u64> {
        self.session_to_epoch.get(&session_id).map(|r| *r.value())
    }

    /// Sets or updates the authoritative session epoch for a given client session ID.
    pub fn set_session_epoch(&self, session_id: u64, epoch: u64) {
        self.session_to_epoch.insert(session_id, epoch);
    }

    /// Removes a client session and its associated player GUID, epoch, and route mappings.
    ///
    /// Removes routing destinations first to ensure no stale forwarding occurs.
    /// Returns the player `Guid` associated with the removed session, if found.
    pub fn remove_session(&self, session_id: u64) -> Option<Guid> {
        // 1. Remove destination route and epoch first
        self.session_to_node.remove(&session_id);
        self.session_to_epoch.remove(&session_id);

        // 2. Remove identity maps last
        if let Some((_, guid)) = self.session_to_guid.remove(&session_id) {
            self.guid_to_node.remove(&guid);
            self.guid_to_session.remove(&guid);
            Some(guid)
        } else {
            None
        }
    }

    /// Removes a route by player GUID.
    ///
    /// Removes routing destinations first to ensure no stale forwarding occurs.
    /// Returns the session ID associated with the removed player, if found.
    pub fn remove_route_by_guid(&self, guid: Guid) -> Option<u64> {
        // 1. Remove destination route first
        self.guid_to_node.remove(&guid);

        // 2. Remove identity maps last
        if let Some((_, session_id)) = self.guid_to_session.remove(&guid) {
            self.session_to_node.remove(&session_id);
            self.session_to_epoch.remove(&session_id);
            self.session_to_guid.remove(&session_id);
            Some(session_id)
        } else {
            None
        }
    }

    /// Retrieves the player `Guid` corresponding to a client session ID.
    pub fn get_guid_by_session(&self, session_id: u64) -> Option<Guid> {
        self.session_to_guid.get(&session_id).map(|r| *r.value())
    }

    /// Retrieves the client session ID corresponding to a player `Guid`.
    pub fn get_session_by_guid(&self, guid: Guid) -> Option<u64> {
        self.guid_to_session.get(&guid).map(|r| *r.value())
    }

    /// Returns the number of currently active sessions.
    pub fn session_count(&self) -> usize {
        self.session_to_guid.len()
    }

    /// Returns `true` if the router contains no active sessions.
    pub fn is_empty(&self) -> bool {
        self.session_to_guid.is_empty()
    }

    /// Clears all routes and session mappings.
    pub fn clear(&self) {
        self.session_to_node.clear();
        self.guid_to_node.clear();
        self.session_to_guid.clear();
        self.guid_to_session.clear();
        self.session_to_epoch.clear();
    }

    /// Replaces the cached spatial partition map and updates the observed topology epoch.
    ///
    /// Updates unconditionally to support Director restarts where `topology_epoch` may reset.
    pub fn update_topology_partitions(&self, epoch: u64, partitions: Vec<PartitionRoute>) {
        let mut guard = self.partitions.write().unwrap_or_else(|e| e.into_inner());
        if *guard != partitions {
            *guard = partitions;
        }
        self.topology_epoch.store(epoch, Ordering::Release);
    }

    /// Returns a snapshot of the cached spatial partition routes.
    pub fn get_topology_partitions(&self) -> Vec<PartitionRoute> {
        self.partitions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Returns the latest topology epoch synchronized from the Director.
    pub fn topology_epoch(&self) -> u64 {
        self.topology_epoch.load(Ordering::Acquire)
    }

    /// Resolves the first non-empty node address whose partition bounding box contains `pos`.
    pub fn resolve_node_for_position(&self, pos: (f32, f32, f32)) -> Option<String> {
        let partitions = self.partitions.read().unwrap_or_else(|e| e.into_inner());
        partitions.iter().find_map(|p| {
            if p.bounds.contains(pos) {
                let trimmed = p.node_address.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            None
        })
    }

    /// Selects an authoritative node address for a new client session.
    ///
    /// - When `spawn_pos` falls inside one or more partitions: returns the sole owning node
    ///   deterministically, or round-robins across unique owning nodes when `spawn_pos` lies on
    ///   a shared boundary seam.
    /// - Otherwise, round-robins across all unique active partition nodes if any exist.
    /// - Falls back to `default_node` when no valid partition routes are cached.
    pub fn select_node_for_new_session(
        &self,
        spawn_pos: Option<(f32, f32, f32)>,
        default_node: Option<&str>,
    ) -> Option<String> {
        let partitions = self.partitions.read().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = spawn_pos {
            let matching =
                collect_unique_addresses(partitions.iter().filter(|p| p.bounds.contains(pos)));
            if matching.len() == 1 {
                return matching.into_iter().next();
            }
            if !matching.is_empty() {
                let idx = self.next_node_rr.fetch_add(1, Ordering::Relaxed) % matching.len();
                return Some(matching[idx].clone());
            }
        }

        let unique = collect_unique_addresses(partitions.iter());
        if unique.len() == 1 {
            return unique.into_iter().next();
        }
        if !unique.is_empty() {
            let idx = self.next_node_rr.fetch_add(1, Ordering::Relaxed) % unique.len();
            return Some(unique[idx].clone());
        }

        default_node.map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_route_registration_and_lookup() {
        let router = SessionRouter::new();
        assert!(router.is_empty());
        assert_eq!(router.session_count(), 0);

        let guid = Guid::new(EntityType::Player, 1, 1001);
        let session_id = 42;
        let node_addr = "127.0.0.1:50051".to_string();

        router.register_session(session_id, guid, node_addr.clone());

        assert_eq!(router.session_count(), 1);
        assert!(!router.is_empty());
        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50051")
        );
        assert_eq!(
            router.get_route_by_session(session_id).as_deref(),
            Some("127.0.0.1:50051")
        );
        assert_eq!(router.get_guid_by_session(session_id), Some(guid));
        assert_eq!(router.get_session_by_guid(guid), Some(session_id));
    }

    #[test]
    fn test_route_update_handover() {
        let router = SessionRouter::new();
        let guid = Guid::new(EntityType::Player, 1, 1002);
        let session_id = 100;
        let node1 = "127.0.0.1:50051".to_string();
        let node2 = "127.0.0.1:50052".to_string();

        router.register_session(session_id, guid, node1);
        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50051")
        );
        assert_eq!(
            router.get_route_by_session(session_id).as_deref(),
            Some("127.0.0.1:50051")
        );

        // Perform seamless handover to Node 2
        let updated = router.update_route(guid, node2);
        assert!(updated);

        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50052")
        );
        assert_eq!(
            router.get_route_by_session(session_id).as_deref(),
            Some("127.0.0.1:50052")
        );
        assert_eq!(router.get_guid_by_session(session_id), Some(guid));
    }

    #[test]
    fn test_remove_session() {
        let router = SessionRouter::new();
        let guid = Guid::new(EntityType::Player, 1, 1003);
        let session_id = 7;
        let node = "127.0.0.1:50051".to_string();

        router.register_session(session_id, guid, node);
        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50051")
        );

        let removed_guid = router.remove_session(session_id);
        assert_eq!(removed_guid, Some(guid));

        assert_eq!(router.get_route_by_guid(guid), None);
        assert_eq!(router.get_route_by_session(session_id), None);
        assert_eq!(router.get_guid_by_session(session_id), None);
        assert_eq!(router.get_session_by_guid(guid), None);
        assert_eq!(router.session_count(), 0);
        assert!(router.is_empty());
    }

    #[test]
    fn test_remove_nonexistent_session() {
        let router = SessionRouter::new();
        assert_eq!(router.remove_session(9999), None);
    }

    #[test]
    fn test_multiple_sessions_and_routes() {
        let router = SessionRouter::new();

        let guid1 = Guid::new(EntityType::Player, 1, 1);
        let guid2 = Guid::new(EntityType::Player, 2, 2);

        router.register_session(1, guid1, "node1:50051".into());
        router.register_session(2, guid2, "node2:50052".into());

        assert_eq!(
            router.get_route_by_guid(guid1).as_deref(),
            Some("node1:50051")
        );
        assert_eq!(
            router.get_route_by_guid(guid2).as_deref(),
            Some("node2:50052")
        );
        assert_eq!(
            router.get_route_by_session(1).as_deref(),
            Some("node1:50051")
        );
        assert_eq!(
            router.get_route_by_session(2).as_deref(),
            Some("node2:50052")
        );

        // Remove session 1
        router.remove_session(1);
        assert_eq!(router.get_route_by_guid(guid1), None);
        assert_eq!(router.get_route_by_session(1), None);

        // Session 2 intact
        assert_eq!(
            router.get_route_by_guid(guid2).as_deref(),
            Some("node2:50052")
        );
        assert_eq!(
            router.get_route_by_session(2).as_deref(),
            Some("node2:50052")
        );
    }

    #[test]
    fn test_register_session_replacement() {
        let router = SessionRouter::new();
        let guid1 = Guid::new(EntityType::Player, 1, 1);
        let guid2 = Guid::new(EntityType::Player, 1, 2);

        // Session 1 mapped to guid1
        router.register_session(1, guid1, "node1:50051".into());
        assert_eq!(router.get_guid_by_session(1), Some(guid1));

        // Re-assign session 1 to guid2
        router.register_session(1, guid2, "node2:50052".into());
        assert_eq!(router.get_guid_by_session(1), Some(guid2));
        assert_eq!(router.get_route_by_guid(guid1), None);
        assert_eq!(
            router.get_route_by_guid(guid2).as_deref(),
            Some("node2:50052")
        );
    }

    #[test]
    fn test_reassign_node_routes_on_node_blackout() {
        let router = SessionRouter::new();
        let guid1 = Guid::new(EntityType::Player, 1, 101);
        let guid2 = Guid::new(EntityType::Player, 1, 102);
        let guid3 = Guid::new(EntityType::Player, 1, 103);

        router.register_session(1, guid1, "mmo-node-0.mmo-node:50052".into());
        router.register_session(2, guid2, "mmo-node-0.mmo-node:50052".into());
        router.register_session(3, guid3, "mmo-node-1.mmo-node:50052".into());

        assert_eq!(
            router.get_route_by_guid(guid1).as_deref(),
            Some("mmo-node-0.mmo-node:50052")
        );

        // When node 0 dies, reassign all its routes to node 1
        let reassigned =
            router.reassign_node_routes("mmo-node-0.mmo-node:50052", "mmo-node-1.mmo-node:50052");
        assert_eq!(reassigned, 4); // 2 guids + 2 sessions

        assert_eq!(
            router.get_route_by_guid(guid1).as_deref(),
            Some("mmo-node-1.mmo-node:50052")
        );
        assert_eq!(
            router.get_route_by_guid(guid2).as_deref(),
            Some("mmo-node-1.mmo-node:50052")
        );
        assert_eq!(
            router.get_route_by_guid(guid3).as_deref(),
            Some("mmo-node-1.mmo-node:50052")
        );
    }

    #[test]
    fn test_session_to_epoch_cleanup_and_stale_reverse_mapping_eviction() {
        let router = SessionRouter::new();
        let guid1 = Guid::new(EntityType::Player, 1, 100);
        let guid2 = Guid::new(EntityType::Player, 1, 200);

        // 1. remove_route_by_guid removes session_to_epoch
        router.register_session(10, guid1, "node-a:50051".to_string());
        router.set_session_epoch(10, 3);
        assert_eq!(router.get_epoch_by_session(10), Some(3));
        assert!(router.remove_route_by_guid(guid1).is_some());
        assert_eq!(router.get_epoch_by_session(10), None);
        assert_eq!(router.get_route_by_session(10), None);
        assert_eq!(router.get_guid_by_session(10), None);
        assert_eq!(router.get_session_by_guid(guid1), None);

        // 2. clear removes session_to_epoch along with all routes
        router.register_session(20, guid1, "node-a:50051".to_string());
        router.set_session_epoch(20, 7);
        router.clear();
        assert_eq!(router.get_epoch_by_session(20), None);
        assert_eq!(router.get_route_by_guid(guid1), None);

        // 3. update_route_with_session evicts stale reverse mappings when rebinding guid or session_id
        router.register_session(30, guid1, "node-a:50051".to_string());
        router.set_session_epoch(30, 5);
        router.register_session(40, guid2, "node-b:50051".to_string());
        router.set_session_epoch(40, 9);

        // Rebind session 40 from guid2 -> guid1 (replaces old session 30 for guid1 and old guid2 for session 40)
        router.update_route_with_session(guid1, 40, "node-c:50051".to_string());

        assert_eq!(router.get_session_by_guid(guid1), Some(40));
        assert_eq!(router.get_guid_by_session(40), Some(guid1));
        assert_eq!(
            router.get_route_by_guid(guid1).as_deref(),
            Some("node-c:50051")
        );
        assert_eq!(
            router.get_route_by_session(40).as_deref(),
            Some("node-c:50051")
        );
        // Old session 30 was evicted completely (route, guid, and epoch)
        assert_eq!(router.get_route_by_session(30), None);
        assert_eq!(router.get_guid_by_session(30), None);
        assert_eq!(router.get_epoch_by_session(30), None);
        // Old guid2 no longer points to session 40
        assert_eq!(router.get_session_by_guid(guid2), None);
        assert_eq!(router.get_route_by_guid(guid2), None);
    }

    #[test]
    fn test_topology_partition_update_and_position_lookup() {
        let router = SessionRouter::new();
        assert_eq!(router.topology_epoch(), 0);
        assert!(router.get_topology_partitions().is_empty());
        assert_eq!(router.resolve_node_for_position((0.0, 0.0, 0.0)), None);

        let world = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let octants = world.subdivide();
        let partitions: Vec<PartitionRoute> = octants
            .into_iter()
            .enumerate()
            .map(|(idx, bounds)| {
                let (node_id, node_address) = if idx % 2 == 0 {
                    (1, " 127.0.0.1:50051 ".to_string())
                } else {
                    (2, "127.0.0.1:50052".to_string())
                };
                PartitionRoute {
                    bounds,
                    node_id,
                    node_address,
                }
            })
            .collect();

        router.update_topology_partitions(2, partitions.clone());
        assert_eq!(router.topology_epoch(), 2);
        assert_eq!(router.get_topology_partitions(), partitions);

        // Octant 0 interior -> Node 1 (trimmed)
        assert_eq!(
            router.resolve_node_for_position((-50.0, -50.0, -50.0)),
            Some("127.0.0.1:50051".to_string())
        );
        // Octant 1 interior -> Node 2
        assert_eq!(
            router.resolve_node_for_position((50.0, -50.0, -50.0)),
            Some("127.0.0.1:50052".to_string())
        );
        // Outside world bounds -> None
        assert_eq!(router.resolve_node_for_position((250.0, 0.0, 0.0)), None);

        // Epoch reset (e.g. Director restart) updates cleanly
        router.update_topology_partitions(
            1,
            vec![PartitionRoute {
                bounds: world,
                node_id: 3,
                node_address: "127.0.0.1:50053".to_string(),
            }],
        );
        assert_eq!(router.topology_epoch(), 1);
        assert_eq!(
            router.resolve_node_for_position((50.0, -50.0, -50.0)),
            Some("127.0.0.1:50053".to_string())
        );
    }

    #[test]
    fn test_select_node_for_new_session_fallback_seam_and_round_robin() {
        let router = SessionRouter::new();

        // 1. Empty partitions -> falls back to default_node
        assert_eq!(
            router.select_node_for_new_session(Some((0.0, 0.0, 5.0)), Some("default:50052")),
            Some("default:50052".to_string())
        );
        assert_eq!(
            router.select_node_for_new_session(Some((0.0, 0.0, 5.0)), None),
            None
        );

        // 2. Populated octants: octants 0,2,4,6 -> node1, octants 1,3,5,7 -> node2
        let world = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let partitions: Vec<PartitionRoute> = world
            .subdivide()
            .into_iter()
            .enumerate()
            .map(|(idx, bounds)| PartitionRoute {
                bounds,
                node_id: if idx % 2 == 0 { 1 } else { 2 },
                node_address: if idx % 2 == 0 {
                    "node1:50051".to_string()
                } else {
                    "node2:50052".to_string()
                },
            })
            .collect();
        router.update_topology_partitions(2, partitions);

        // Interior points deterministically select their exact owner
        for _ in 0..4 {
            assert_eq!(
                router.select_node_for_new_session(
                    Some((-50.0, -50.0, -50.0)),
                    Some("default:50052")
                ),
                Some("node1:50051".to_string())
            );
            assert_eq!(
                router
                    .select_node_for_new_session(Some((50.0, -50.0, -50.0)), Some("default:50052")),
                Some("node2:50052".to_string())
            );
        }

        // Octant boundary seam (0.0, 0.0, 5.0) is shared by octants 4,5,6,7 (both node1 and node2)
        let seam_picks: Vec<Option<String>> = (0..4)
            .map(|_| {
                router.select_node_for_new_session(Some((0.0, 0.0, 5.0)), Some("default:50052"))
            })
            .collect();
        assert_eq!(
            seam_picks,
            vec![
                Some("node1:50051".to_string()),
                Some("node2:50052".to_string()),
                Some("node1:50051".to_string()),
                Some("node2:50052".to_string()),
            ]
        );

        // None spawn_pos (and out-of-bounds spawn_pos) round-robins across unique active nodes
        let rr_none: Vec<Option<String>> = (0..4)
            .map(|_| router.select_node_for_new_session(None, Some("default:50052")))
            .collect();
        assert_eq!(
            rr_none,
            vec![
                Some("node1:50051".to_string()),
                Some("node2:50052".to_string()),
                Some("node1:50051".to_string()),
                Some("node2:50052".to_string()),
            ]
        );

        let rr_oob = router.select_node_for_new_session(Some((999.0, 0.0, 0.0)), None);
        assert_eq!(rr_oob, Some("node1:50051".to_string()));
        let rr_oob2 = router.select_node_for_new_session(Some((999.0, 0.0, 0.0)), None);
        assert_eq!(rr_oob2, Some("node2:50052".to_string()));
    }

    #[test]
    fn test_router_clone_eq_and_clear_preserves_topology() {
        let router = SessionRouter::new();
        let guid = Guid::new(EntityType::Player, 1, 77);
        router.register_session(10, guid, "node1:50051".to_string());
        router.update_topology_partitions(
            3,
            vec![PartitionRoute {
                bounds: BoundingBox3D::new((-10.0, -10.0, -10.0), (10.0, 10.0, 10.0)),
                node_id: 1,
                node_address: "node1:50051".to_string(),
            }],
        );

        let cloned = router.clone();
        assert_eq!(router, cloned);
        assert_eq!(cloned.topology_epoch(), 3);
        assert_eq!(cloned.get_topology_partitions().len(), 1);

        // clear() wipes session mappings while leaving topology partitions intact
        cloned.clear();
        assert!(cloned.is_empty());
        assert_eq!(cloned.topology_epoch(), 3);
        assert_eq!(cloned.get_topology_partitions().len(), 1);
        assert_ne!(router, cloned);
    }
}
