//! Dynamic Session Routing Table for MMO Edge Gateway.
//!
//! Maps client sessions (`session_id: u64`) and player entities (`player_guid: Guid`)
//! to authoritative simulation node network addresses (`String`), facilitating
//! dynamic, seamless packet forwarding and boundary handovers.

use std::collections::HashMap;
use mmo_core::guid::Guid;

/// Session routing table mapping sessions and player GUIDs to authoritative node addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRouter {
    /// Maps client session ID to player GUID.
    session_to_guid: HashMap<u64, Guid>,
    /// Maps player GUID to client session ID.
    guid_to_session: HashMap<Guid, u64>,
    /// Maps player GUID to authoritative simulation node address.
    guid_to_node: HashMap<Guid, String>,
    /// Maps client session ID directly to authoritative simulation node address.
    session_to_node: HashMap<u64, String>,
}

impl SessionRouter {
    /// Creates a new empty `SessionRouter`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a client session with its associated player GUID and authoritative node address.
    pub fn register_session(&mut self, session_id: u64, guid: Guid, node_address: String) {
        // If session_id was already mapped to a different guid, clean up stale reverse mapping
        if let Some(old_guid) = self.session_to_guid.insert(session_id, guid) {
            if old_guid != guid {
                self.guid_to_session.remove(&old_guid);
                self.guid_to_node.remove(&old_guid);
            }
        }

        // If guid was already mapped to a different session_id, clean up stale reverse mapping
        if let Some(old_sid) = self.guid_to_session.insert(guid, session_id) {
            if old_sid != session_id {
                self.session_to_guid.remove(&old_sid);
                self.session_to_node.remove(&old_sid);
            }
        }

        self.guid_to_node.insert(guid, node_address.clone());
        self.session_to_node.insert(session_id, node_address);
    }

    /// Updates the authoritative node address for a player GUID during seamless handovers.
    ///
    /// Updates both GUID and session-based lookups. Returns `true` if the route
    /// previously existed and was updated, or `false` if it was newly added.
    pub fn update_route(&mut self, guid: Guid, new_node_address: String) -> bool {
        let existed = self.guid_to_node.contains_key(&guid);
        self.guid_to_node.insert(guid, new_node_address.clone());
        if let Some(&session_id) = self.guid_to_session.get(&guid) {
            self.session_to_node.insert(session_id, new_node_address);
        }
        existed
    }

    /// Resolves the authoritative node address for a given player GUID.
    pub fn get_route_by_guid(&self, guid: Guid) -> Option<&str> {
        self.guid_to_node.get(&guid).map(|s| s.as_str())
    }

    /// Resolves the authoritative node address for a given client session ID.
    pub fn get_route_by_session(&self, session_id: u64) -> Option<&str> {
        self.session_to_node
            .get(&session_id)
            .map(|s| s.as_str())
            .or_else(|| {
                let guid = self.session_to_guid.get(&session_id)?;
                self.guid_to_node.get(guid).map(|s| s.as_str())
            })
    }

    /// Removes a client session and its associated player GUID and route mappings.
    ///
    /// Returns the player `Guid` associated with the removed session, if found.
    pub fn remove_session(&mut self, session_id: u64) -> Option<Guid> {
        self.session_to_node.remove(&session_id);
        if let Some(guid) = self.session_to_guid.remove(&session_id) {
            self.guid_to_session.remove(&guid);
            self.guid_to_node.remove(&guid);
            Some(guid)
        } else {
            None
        }
    }

    /// Removes a route by player GUID.
    ///
    /// Returns the session ID associated with the removed player, if found.
    pub fn remove_route_by_guid(&mut self, guid: Guid) -> Option<u64> {
        self.guid_to_node.remove(&guid);
        if let Some(session_id) = self.guid_to_session.remove(&guid) {
            self.session_to_guid.remove(&session_id);
            self.session_to_node.remove(&session_id);
            Some(session_id)
        } else {
            None
        }
    }

    /// Retrieves the player `Guid` corresponding to a client session ID.
    pub fn get_guid_by_session(&self, session_id: u64) -> Option<Guid> {
        self.session_to_guid.get(&session_id).copied()
    }

    /// Retrieves the client session ID corresponding to a player `Guid`.
    pub fn get_session_by_guid(&self, guid: Guid) -> Option<u64> {
        self.guid_to_session.get(&guid).copied()
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
    pub fn clear(&mut self) {
        self.session_to_guid.clear();
        self.guid_to_session.clear();
        self.guid_to_node.clear();
        self.session_to_node.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_route_registration_and_lookup() {
        let mut router = SessionRouter::new();
        assert!(router.is_empty());
        assert_eq!(router.session_count(), 0);

        let guid = Guid::new(EntityType::Player, 1, 1001);
        let session_id = 42;
        let node_addr = "127.0.0.1:50051".to_string();

        router.register_session(session_id, guid, node_addr.clone());

        assert_eq!(router.session_count(), 1);
        assert!(!router.is_empty());
        assert_eq!(router.get_route_by_guid(guid), Some("127.0.0.1:50051"));
        assert_eq!(router.get_route_by_session(session_id), Some("127.0.0.1:50051"));
        assert_eq!(router.get_guid_by_session(session_id), Some(guid));
        assert_eq!(router.get_session_by_guid(guid), Some(session_id));
    }

    #[test]
    fn test_route_update_handover() {
        let mut router = SessionRouter::new();
        let guid = Guid::new(EntityType::Player, 1, 1002);
        let session_id = 100;
        let node1 = "127.0.0.1:50051".to_string();
        let node2 = "127.0.0.1:50052".to_string();

        router.register_session(session_id, guid, node1);
        assert_eq!(router.get_route_by_guid(guid), Some("127.0.0.1:50051"));
        assert_eq!(router.get_route_by_session(session_id), Some("127.0.0.1:50051"));

        // Perform seamless handover to Node 2
        let updated = router.update_route(guid, node2);
        assert!(updated);

        assert_eq!(router.get_route_by_guid(guid), Some("127.0.0.1:50052"));
        assert_eq!(router.get_route_by_session(session_id), Some("127.0.0.1:50052"));
        assert_eq!(router.get_guid_by_session(session_id), Some(guid));
    }

    #[test]
    fn test_remove_session() {
        let mut router = SessionRouter::new();
        let guid = Guid::new(EntityType::Player, 1, 1003);
        let session_id = 7;
        let node = "127.0.0.1:50051".to_string();

        router.register_session(session_id, guid, node);
        assert_eq!(router.get_route_by_guid(guid), Some("127.0.0.1:50051"));

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
        let mut router = SessionRouter::new();
        assert_eq!(router.remove_session(9999), None);
    }

    #[test]
    fn test_multiple_sessions_and_routes() {
        let mut router = SessionRouter::new();

        let guid1 = Guid::new(EntityType::Player, 1, 1);
        let guid2 = Guid::new(EntityType::Player, 2, 2);

        router.register_session(1, guid1, "node1:50051".into());
        router.register_session(2, guid2, "node2:50052".into());

        assert_eq!(router.get_route_by_guid(guid1), Some("node1:50051"));
        assert_eq!(router.get_route_by_guid(guid2), Some("node2:50052"));
        assert_eq!(router.get_route_by_session(1), Some("node1:50051"));
        assert_eq!(router.get_route_by_session(2), Some("node2:50052"));

        // Remove session 1
        router.remove_session(1);
        assert_eq!(router.get_route_by_guid(guid1), None);
        assert_eq!(router.get_route_by_session(1), None);

        // Session 2 intact
        assert_eq!(router.get_route_by_guid(guid2), Some("node2:50052"));
        assert_eq!(router.get_route_by_session(2), Some("node2:50052"));
    }

    #[test]
    fn test_register_session_replacement() {
        let mut router = SessionRouter::new();
        let guid1 = Guid::new(EntityType::Player, 1, 1);
        let guid2 = Guid::new(EntityType::Player, 1, 2);

        // Session 1 mapped to guid1
        router.register_session(1, guid1, "node1:50051".into());
        assert_eq!(router.get_guid_by_session(1), Some(guid1));

        // Re-assign session 1 to guid2
        router.register_session(1, guid2, "node2:50052".into());
        assert_eq!(router.get_guid_by_session(1), Some(guid2));
        assert_eq!(router.get_route_by_guid(guid1), None);
        assert_eq!(router.get_route_by_guid(guid2), Some("node2:50052"));
    }
}
