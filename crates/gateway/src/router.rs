//! Dynamic Session Routing Table for MMO Edge Gateway.
//!
//! Maps client sessions (`session_id: u64`) and player entities (`player_guid: Guid`)
//! to authoritative simulation node network addresses (`String`), facilitating
//! dynamic, seamless packet forwarding and boundary handovers with lock-free sharded concurrency.

use dashmap::DashMap;
use mmo_core::guid::Guid;

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
        router
    }
}

impl PartialEq for SessionRouter {
    fn eq(&self, other: &Self) -> bool {
        if self.session_count() != other.session_count() {
            return false;
        }
        for item in self.session_to_guid.iter() {
            if other.session_to_guid.get(item.key()).map(|r| *r.value()) != Some(*item.value()) {
                return false;
            }
        }
        true
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
            self.session_to_guid.insert(session_id, guid);
            self.guid_to_session.insert(guid, session_id);
            self.session_to_node
                .insert(session_id, new_node_address.clone());
        } else if let Some(session_ref) = self.guid_to_session.get(&guid) {
            let sid = *session_ref.value();
            drop(session_ref);
            self.session_to_node
                .insert(sid, new_node_address.clone());
        }
        self.guid_to_node.insert(guid, new_node_address);
        existed
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

    /// Removes a client session and its associated player GUID and route mappings.
    ///
    /// Removes routing destinations first to ensure no stale forwarding occurs.
    /// Returns the player `Guid` associated with the removed session, if found.
    pub fn remove_session(&self, session_id: u64) -> Option<Guid> {
        // 1. Remove destination route first
        self.session_to_node.remove(&session_id);

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
}
