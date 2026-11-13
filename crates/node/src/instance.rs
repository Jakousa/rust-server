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
    /// Creates a new `LocalInstanceDomain` anchored at explicit simulation timestamp `now`.
    pub fn new_at(
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout: Duration,
        now: Instant,
    ) -> Self {
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

    /// Creates a new `LocalInstanceDomain`.
    pub fn new(
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout: Duration,
    ) -> Self {
        Self::new_at(
            instance_id,
            template_id,
            bounds,
            idle_timeout,
            Instant::now(),
        )
    }

    /// Checks if the instance is currently idle past its timeout threshold.
    pub fn is_idle_expired(&self, now: Instant) -> bool {
        if !self.player_entities.is_empty() {
            return false;
        }
        now.saturating_duration_since(self.last_occupied_at) >= self.idle_timeout
    }

    /// Adds an entity to the instance domain at explicit timestamp `now`, specifying whether it is a player.
    pub fn add_entity_at(&mut self, guid: Guid, is_player: bool, now: Instant) {
        self.entities.insert(guid);
        if is_player {
            self.player_entities.insert(guid);
            self.last_occupied_at = now;
        }
    }

    /// Adds an entity to the instance domain, specifying whether it is a player.
    pub fn add_entity(&mut self, guid: Guid, is_player: bool) {
        self.add_entity_at(guid, is_player, Instant::now());
    }

    /// Removes an entity from the instance domain at explicit timestamp `now`.
    pub fn remove_entity_at(&mut self, guid: Guid, now: Instant) -> bool {
        let was_player = self.player_entities.remove(&guid);
        let removed = self.entities.remove(&guid);
        if was_player && self.player_entities.is_empty() {
            self.last_occupied_at = now;
        }
        removed
    }

    /// Removes an entity from the instance domain.
    pub fn remove_entity(&mut self, guid: Guid) -> bool {
        self.remove_entity_at(guid, Instant::now())
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

    /// Returns `(instance_id, player_count, total_entity_count, is_idle)` for all active instances.
    pub fn active_instance_heartbeats(&self) -> Vec<(u64, u32, u32, bool)> {
        self.instances
            .values()
            .map(|domain| {
                (
                    domain.instance_id,
                    domain.player_entities.len() as u32,
                    domain.entities.len() as u32,
                    domain.player_entities.is_empty(),
                )
            })
            .collect()
    }

    /// Creates and registers an isolated instance domain anchored at explicit timestamp `now`.
    pub fn create_instance_at(
        &mut self,
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout: Duration,
        now: Instant,
    ) -> Result<(), NodeInstanceError> {
        if self.instances.contains_key(&instance_id) {
            return Err(NodeInstanceError::AlreadyExists(instance_id));
        }

        let domain =
            LocalInstanceDomain::new_at(instance_id, template_id, bounds, idle_timeout, now);
        self.instances.insert(instance_id, domain);
        Ok(())
    }

    /// Creates and registers an isolated instance domain.
    pub fn create_instance(
        &mut self,
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout: Duration,
    ) -> Result<(), NodeInstanceError> {
        self.create_instance_at(
            instance_id,
            template_id,
            bounds,
            idle_timeout,
            Instant::now(),
        )
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

    /// Assigns an entity to a local instance domain at explicit timestamp `now`.
    pub fn assign_entity_at(
        &mut self,
        guid: Guid,
        instance_id: u64,
        is_player: bool,
        now: Instant,
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

        domain.add_entity_at(guid, is_player, now);
        self.entity_to_instance.insert(guid, instance_id);
        Ok(())
    }

    /// Assigns an entity to a local instance domain.
    pub fn assign_entity(
        &mut self,
        guid: Guid,
        instance_id: u64,
        is_player: bool,
    ) -> Result<(), NodeInstanceError> {
        self.assign_entity_at(guid, instance_id, is_player, Instant::now())
    }

    /// Removes an entity from its associated instance domain at explicit timestamp `now`.
    pub fn remove_entity_at(&mut self, guid: Guid, now: Instant) -> Option<u64> {
        if let Some(instance_id) = self.entity_to_instance.remove(&guid) {
            if let Some(domain) = self.instances.get_mut(&instance_id) {
                domain.remove_entity_at(guid, now);
            }
            Some(instance_id)
        } else {
            None
        }
    }

    /// Removes an entity from its associated instance domain.
    pub fn remove_entity(&mut self, guid: Guid) -> Option<u64> {
        self.remove_entity_at(guid, Instant::now())
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

    /// Reaps and tears down all instance domains that have exceeded their idle timeouts at timestamp `now`.
    ///
    /// Returns a list of `(instance_id, evicted_entities)` pairs.
    pub fn reap_idle_instances_at(&mut self, now: Instant) -> Vec<(u64, HashSet<Guid>)> {
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

    /// Reaps and tears down all instance domains that have exceeded their idle timeouts.
    ///
    /// Returns a list of `(instance_id, evicted_entities)` pairs.
    pub fn reap_idle_instances(&mut self) -> Vec<(u64, HashSet<Guid>)> {
        self.reap_idle_instances_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_instance_already_exists() {
        let mut mgr = InstanceDomainManager::new();
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        mgr.create_instance(500, 1, bounds, Duration::from_millis(50))
            .expect("create instance");

        let err = mgr
            .create_instance(500, 2, bounds, Duration::from_millis(100))
            .expect_err("should fail to create duplicate instance");

        assert_eq!(err, NodeInstanceError::AlreadyExists(500));
    }

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

    #[test]
    fn test_assign_entity_errors() {
        let mut mgr = InstanceDomainManager::new();
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        let instance_id_1 = 100;
        let instance_id_2 = 200;
        mgr.create_instance(instance_id_1, 1, bounds, Duration::from_millis(50))
            .expect("create instance 1");
        mgr.create_instance(instance_id_2, 1, bounds, Duration::from_millis(50))
            .expect("create instance 2");

        let entity_guid = Guid::from_raw(1);

        // 1. Assign to non-existent instance
        let non_existent_instance = 999;
        let res = mgr.assign_entity(entity_guid, non_existent_instance, false);
        assert_eq!(res, Err(NodeInstanceError::NotFound(non_existent_instance)));

        // 2. Successful assignment
        assert!(mgr.assign_entity(entity_guid, instance_id_1, false).is_ok());

        // 3. Assign to a different instance should fail with EntityAlreadyInInstance
        let res = mgr.assign_entity(entity_guid, instance_id_2, false);
        assert_eq!(
            res,
            Err(NodeInstanceError::EntityAlreadyInInstance(
                entity_guid,
                instance_id_1
            ))
        );

        // 4. Assigning to the same instance again should succeed (idempotent)
        assert!(mgr.assign_entity(entity_guid, instance_id_1, false).is_ok());
    }

    #[test]
    fn test_teardown_instance() {
        let mut mgr = InstanceDomainManager::new();
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        // Create an instance
        mgr.create_instance(999, 1, bounds, Duration::from_secs(60))
            .expect("create instance");

        // Add some entities
        let p1 = Guid::from_raw(111);
        let npc1 = Guid::from_raw(222);

        mgr.assign_entity(p1, 999, true).unwrap();
        mgr.assign_entity(npc1, 999, false).unwrap();

        // Verify state before teardown
        assert_eq!(mgr.len(), 1);
        assert_eq!(mgr.get_entity_instance(p1), Some(999));
        assert_eq!(mgr.get_entity_instance(npc1), Some(999));

        // Test Success Path
        let evicted = mgr.teardown_instance(999).unwrap();

        // Verify return value
        assert_eq!(evicted.len(), 2);
        assert!(evicted.contains(&p1));
        assert!(evicted.contains(&npc1));

        // Verify state after teardown
        assert!(mgr.is_empty());
        assert!(mgr.get_instance(999).is_none());
        assert_eq!(mgr.get_entity_instance(p1), None);
        assert_eq!(mgr.get_entity_instance(npc1), None);

        // Test Error Path
        let result = mgr.teardown_instance(1000);
        assert_eq!(result, Err(NodeInstanceError::NotFound(1000)));
    }

    #[test]
    fn test_deterministic_instance_idle_reap_at() {
        let mut mgr = InstanceDomainManager::new();
        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        let t0 = Instant::now();

        mgr.create_instance_at(700, 1, bounds, Duration::from_secs(30), t0)
            .expect("create instance");

        let p1 = Guid::from_raw(101);
        let npc1 = Guid::from_raw(201);

        // Player enters at t0 + 10s
        let t_join = t0 + Duration::from_secs(10);
        mgr.assign_entity_at(p1, 700, true, t_join).unwrap();
        mgr.assign_entity_at(npc1, 700, false, t_join).unwrap();

        // At t0 + 100s while player is present, must NOT be reaped
        assert!(mgr
            .reap_idle_instances_at(t0 + Duration::from_secs(100))
            .is_empty());

        // Player leaves at t0 + 100s
        let t_leave = t0 + Duration::from_secs(100);
        mgr.remove_entity_at(p1, t_leave);

        // At t_leave + 29s (< 30s idle timeout), must NOT be reaped
        assert!(mgr
            .reap_idle_instances_at(t_leave + Duration::from_secs(29))
            .is_empty());
        assert_eq!(mgr.len(), 1);

        // At t_leave + 30s (>= 30s idle timeout), MUST be reaped deterministically
        let reaped = mgr.reap_idle_instances_at(t_leave + Duration::from_secs(30));
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].0, 700);
        assert!(reaped[0].1.contains(&npc1));
        assert!(mgr.is_empty());
    }
}
