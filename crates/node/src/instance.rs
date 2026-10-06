//! Node-side Dynamic Instance Domain Manager.
//!
//! Hosts isolated, transient simulation domains inside a simulation node.
//! Tracks entity memberships, spatial bounds per instance, and automatic idle expiration.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use mmo_core::guid::Guid;
use mmo_core::octree::BoundingBox3D;
use thiserror::Error;

/// Errors that can occur within the node-side instance domain manager.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NodeInstanceError {
    #[error("Instance domain {0} already exists on this node")]
    AlreadyExists(u64),

    #[error("Instance domain {0} not found on this node")]
    NotFound(u64),

    #[error("Entity {0} already belongs to instance {1}")]
    EntityAlreadyInInstance(Guid, u64),

    #[error("Invalid instance bounding box: {0}")]
    InvalidBoundingBox(String),
}

/// An active transient instance domain hosted on this node.
#[derive(Debug)]
pub struct LocalInstanceDomain {
    pub instance_id: u64,
    pub template_id: u32,
    pub bounds: BoundingBox3D,
    pub entities: HashSet<Guid>,
    pub player_entities: HashSet<Guid>,
    pub created_at: Instant,
    pub last_occupied_at: Instant,
    pub idle_timeout: Duration,
}

impl LocalInstanceDomain {
    /// Creates a new `LocalInstanceDomain`.
    pub fn new(
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout: Duration,
    ) -> Self {
        let now = Instant::now();
        Self {
            instance_id,
            template_id,
            bounds,
            entities: HashSet::new(),
            player_entities: HashSet::new(),
            created_at: now,
            last_occupied_at: now,
            idle_timeout,
        }
    }

    /// Checks if the instance is currently idle past its timeout threshold.
    pub fn is_idle_expired(&self, now: Instant) -> bool {
        if !self.player_entities.is_empty() {
            return false;
        }
        now.duration_since(self.last_occupied_at) >= self.idle_timeout
    }

    /// Adds an entity to the instance domain, specifying whether it is a player.
    pub fn add_entity(&mut self, guid: Guid, is_player: bool) {
        self.entities.insert(guid);
        if is_player {
            self.player_entities.insert(guid);
            self.last_occupied_at = Instant::now();
        }
    }

    /// Removes an entity from the instance domain.
    pub fn remove_entity(&mut self, guid: Guid) -> bool {
        let was_player = self.player_entities.remove(&guid);
        let removed = self.entities.remove(&guid);
        if was_player && self.player_entities.is_empty() {
            self.last_occupied_at = Instant::now();
        }
        removed
    }
}

/// Node-side manager coordinating all local instance domains.
#[derive(Debug, Default)]
pub struct InstanceDomainManager {
    instances: HashMap<u64, LocalInstanceDomain>,
    entity_to_instance: HashMap<Guid, u64>,
}

impl InstanceDomainManager {
    /// Creates a new empty `InstanceDomainManager`.
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            entity_to_instance: HashMap::new(),
        }
    }

    /// Returns the number of active local instances.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Returns true if there are no active instances.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Creates and registers an isolated instance domain.
    pub fn create_instance(
        &mut self,
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout: Duration,
    ) -> Result<(), NodeInstanceError> {
        if self.instances.contains_key(&instance_id) {
            return Err(NodeInstanceError::AlreadyExists(instance_id));
        }

        let domain = LocalInstanceDomain::new(instance_id, template_id, bounds, idle_timeout);
        self.instances.insert(instance_id, domain);
        Ok(())
    }

    /// Retrieves a reference to an instance domain.
    pub fn get_instance(&self, instance_id: u64) -> Option<&LocalInstanceDomain> {
        self.instances.get(&instance_id)
    }

    /// Retrieves a mutable reference to an instance domain.
    pub fn get_instance_mut(&mut self, instance_id: u64) -> Option<&mut LocalInstanceDomain> {
        self.instances.get_mut(&instance_id)
    }

    /// Returns the instance ID an entity currently belongs to, if any.
    pub fn get_entity_instance(&self, guid: Guid) -> Option<u64> {
        self.entity_to_instance.get(&guid).copied()
    }

    /// Assigns an entity to a local instance domain.
    pub fn assign_entity(
        &mut self,
        guid: Guid,
        instance_id: u64,
        is_player: bool,
    ) -> Result<(), NodeInstanceError> {
        if let Some(&existing_instance) = self.entity_to_instance.get(&guid) {
            if existing_instance != instance_id {
                return Err(NodeInstanceError::EntityAlreadyInInstance(
                    guid,
                    existing_instance,
                ));
            }
        }

        let domain = self
            .instances
            .get_mut(&instance_id)
            .ok_or(NodeInstanceError::NotFound(instance_id))?;

        domain.add_entity(guid, is_player);
        self.entity_to_instance.insert(guid, instance_id);
        Ok(())
    }

    /// Removes an entity from its associated instance domain.
    pub fn remove_entity(&mut self, guid: Guid) -> Option<u64> {
        if let Some(instance_id) = self.entity_to_instance.remove(&guid) {
            if let Some(domain) = self.instances.get_mut(&instance_id) {
                domain.remove_entity(guid);
            }
            Some(instance_id)
        } else {
            None
        }
    }

    /// Tears down and unregisters an instance domain, returning all entities evicted from it.
    pub fn teardown_instance(
        &mut self,
        instance_id: u64,
    ) -> Result<HashSet<Guid>, NodeInstanceError> {
        let domain = self
            .instances
            .remove(&instance_id)
            .ok_or(NodeInstanceError::NotFound(instance_id))?;

        for entity_guid in &domain.entities {
            self.entity_to_instance.remove(entity_guid);
        }

        Ok(domain.entities)
    }

    /// Reaps and tears down all instance domains that have exceeded their idle timeouts.
    ///
    /// Returns a list of `(instance_id, evicted_entities)` pairs.
    pub fn reap_idle_instances(&mut self) -> Vec<(u64, HashSet<Guid>)> {
        let now = Instant::now();
        let expired_ids: Vec<u64> = self
            .instances
            .iter()
            .filter_map(|(&id, domain)| {
                if domain.is_idle_expired(now) {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();

        let mut reaped = Vec::with_capacity(expired_ids.len());
        for id in expired_ids {
            if let Ok(entities) = self.teardown_instance(id) {
                reaped.push((id, entities));
            }
        }
        reaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_instance_lifecycle_and_entity_membership() {
        let mut mgr = InstanceDomainManager::new();
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        mgr.create_instance(500, 1, bounds, Duration::from_millis(50))
            .expect("create instance");

        let p1 = Guid::from_raw(100);
        let npc1 = Guid::from_raw(200);

        mgr.assign_entity(p1, 500, true).unwrap();
        mgr.assign_entity(npc1, 500, false).unwrap();

        assert_eq!(mgr.get_entity_instance(p1), Some(500));
        assert_eq!(mgr.get_entity_instance(npc1), Some(500));

        let inst = mgr.get_instance(500).unwrap();
        assert_eq!(inst.entities.len(), 2);
        assert_eq!(inst.player_entities.len(), 1);

        // Remove player
        mgr.remove_entity(p1);
        assert_eq!(mgr.get_entity_instance(p1), None);

        let inst = mgr.get_instance(500).unwrap();
        assert_eq!(inst.player_entities.len(), 0);
        assert_eq!(inst.entities.len(), 1);

        // Sleep to trigger idle expiration
        std::thread::sleep(Duration::from_millis(60));
        let reaped = mgr.reap_idle_instances();
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].0, 500);
        assert!(reaped[0].1.contains(&npc1));
        assert!(mgr.is_empty());
    }
}
