//! Dynamic Instance Registry and Lifecycle Management.
//!
//! Tracks isolated, transient simulation domains across active cluster nodes,
//! allocates instances according to node load, validates bounding boxes,
//! and tracks idle timeouts for resource reclamation.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use mmo_core::BoundingBox3D;
use thiserror::Error;

use crate::registry::NodeRegistry;

/// Errors that can occur during dynamic instance management.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InstanceError {
    #[error("No available simulation nodes to host instance")]
    NoNodesAvailable,

    #[error("Specified preferred node {0} not found in registry")]
    NodeNotFound(u32),

    #[error("Instance {0} not found")]
    InstanceNotFound(u64),

    #[error("Instance {0} already exists")]
    InstanceAlreadyExists(u64),

    #[error("Invalid instance bounding box: {0}")]
    InvalidBoundingBox(String),
}

/// Metadata and state for an active transient simulation domain.
#[derive(Debug, Clone)]
pub struct InstanceDescriptor {
    pub instance_id: u64,
    pub template_id: u32,
    pub assigned_node_id: u32,
    pub bounds: BoundingBox3D,
    pub created_at: Instant,
    pub idle_timeout: Duration,
    pub last_heartbeat: Instant,
    pub last_occupied_at: Instant,
    pub player_count: u32,
    pub total_entity_count: u32,
    pub is_idle: bool,
}

impl InstanceDescriptor {
    /// Checks if this instance has been idle longer than its configured `idle_timeout`.
    pub fn is_expired(&self, now: Instant) -> bool {
        if self.player_count > 0 {
            return false;
        }
        now.duration_since(self.last_occupied_at) >= self.idle_timeout
    }
}

/// Validates that a 3D bounding box is finite and has non-negative extents.
pub fn validate_instance_bounds(bounds: &BoundingBox3D) -> Result<(), InstanceError> {
    if !bounds.min.0.is_finite()
        || !bounds.min.1.is_finite()
        || !bounds.min.2.is_finite()
        || !bounds.max.0.is_finite()
        || !bounds.max.1.is_finite()
        || !bounds.max.2.is_finite()
    {
        return Err(InstanceError::InvalidBoundingBox(
            "Bounding box coordinates must be finite".to_string(),
        ));
    }

    if bounds.min.0 >= bounds.max.0 || bounds.min.1 >= bounds.max.1 || bounds.min.2 >= bounds.max.2
    {
        return Err(InstanceError::InvalidBoundingBox(
            "Bounding box min coordinates must be strictly less than max coordinates (degenerate extents)".to_string(),
        ));
    }

    Ok(())
}

/// Registry of active transient instances in the cluster.
#[derive(Debug, Default)]
pub struct InstanceRegistry {
    instances: HashMap<u64, InstanceDescriptor>,
}

impl InstanceRegistry {
    /// Creates a new, empty `InstanceRegistry`.
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
        }
    }

    /// Returns the number of currently active instances.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Returns true if the registry contains no instances.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Retrieves an instance descriptor by its `instance_id`.
    pub fn get_instance(&self, instance_id: u64) -> Option<&InstanceDescriptor> {
        self.instances.get(&instance_id)
    }

    /// Allocates an instance domain onto a simulation node.
    ///
    /// If `preferred_node_id > 0`, attempts to host the instance on that node if it exists in `node_registry`.
    /// Otherwise, selects the least-loaded simulation node from `node_registry`.
    pub fn allocate_instance(
        &mut self,
        instance_id: u64,
        template_id: u32,
        bounds: BoundingBox3D,
        idle_timeout_seconds: u32,
        preferred_node_id: u32,
        node_registry: &NodeRegistry,
    ) -> Result<InstanceDescriptor, InstanceError> {
        validate_instance_bounds(&bounds)?;

        if self.instances.contains_key(&instance_id) {
            return Err(InstanceError::InstanceAlreadyExists(instance_id));
        }

        let assigned_node_id = if preferred_node_id > 0 {
            if node_registry.get_node(preferred_node_id).is_some() {
                preferred_node_id
            } else {
                return Err(InstanceError::NodeNotFound(preferred_node_id));
            }
        } else {
            node_registry
                .find_least_loaded_node(&[])
                .ok_or(InstanceError::NoNodesAvailable)?
        };

        let now = Instant::now();
        let timeout_secs = if idle_timeout_seconds == 0 {
            60 // default 60s idle timeout
        } else {
            idle_timeout_seconds
        };

        let descriptor = InstanceDescriptor {
            instance_id,
            template_id,
            assigned_node_id,
            bounds,
            created_at: now,
            idle_timeout: Duration::from_secs(timeout_secs as u64),
            last_heartbeat: now,
            last_occupied_at: now,
            player_count: 0,
            total_entity_count: 0,
            is_idle: true,
        };

        self.instances.insert(instance_id, descriptor.clone());
        Ok(descriptor)
    }

    /// Records telemetry from an instance heartbeat and updates its idle status.
    pub fn record_heartbeat(
        &mut self,
        instance_id: u64,
        player_count: u32,
        total_entity_count: u32,
        is_idle: bool,
    ) -> Result<bool, InstanceError> {
        let instance = self
            .instances
            .get_mut(&instance_id)
            .ok_or(InstanceError::InstanceNotFound(instance_id))?;

        let now = Instant::now();
        instance.last_heartbeat = now;
        instance.player_count = player_count;
        instance.total_entity_count = total_entity_count;
        instance.is_idle = is_idle;

        if player_count > 0 {
            instance.last_occupied_at = now;
        }

        Ok(instance.is_expired(now))
    }

    /// Terminates and unregisters an instance domain.
    pub fn terminate_instance(&mut self, instance_id: u64) -> Option<InstanceDescriptor> {
        self.instances.remove(&instance_id)
    }

    /// Evicts all instances hosted on the specified simulation nodes (e.g. after a node crash).
    pub fn evict_instances_on_nodes(&mut self, dead_node_ids: &[u32]) -> Vec<InstanceDescriptor> {
        let mut evicted = Vec::new();
        self.instances.retain(|_, desc| {
            if dead_node_ids.contains(&desc.assigned_node_id) {
                evicted.push(desc.clone());
                false
            } else {
                true
            }
        });
        evicted
    }

    /// Scans for and removes all expired idle instances, returning their descriptors.
    pub fn reap_idle_instances(&mut self) -> Vec<InstanceDescriptor> {
        let now = Instant::now();
        let mut reaped = Vec::new();
        self.instances.retain(|_, desc| {
            if desc.is_expired(now) {
                reaped.push(desc.clone());
                false
            } else {
                true
            }
        });
        reaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounds_validation() {
        let valid = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
        assert!(validate_instance_bounds(&valid).is_ok());

        let nan_bounds = BoundingBox3D {
            min: (f32::NAN, 0.0, 0.0),
            max: (100.0, 100.0, 100.0),
        };
        assert!(validate_instance_bounds(&nan_bounds).is_err());

        let inverted_bounds = BoundingBox3D {
            min: (100.0, 0.0, 0.0),
            max: (50.0, 100.0, 100.0),
        };
        assert!(validate_instance_bounds(&inverted_bounds).is_err());

        let degenerate_bounds = BoundingBox3D {
            min: (50.0, 0.0, 0.0),
            max: (50.0, 100.0, 100.0),
        };
        assert!(validate_instance_bounds(&degenerate_bounds).is_err());
    }

    #[test]
    fn test_instance_allocation_and_reap() {
        let mut node_reg = NodeRegistry::new();
        node_reg.register_node("127.0.0.1:50051".to_string(), vec![]);

        let mut inst_reg = InstanceRegistry::new();
        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));

        let desc = inst_reg
            .allocate_instance(1001, 1, bounds, 1, 0, &node_reg)
            .expect("should allocate");
        assert_eq!(desc.instance_id, 1001);
        assert_eq!(desc.assigned_node_id, 1);

        // Immediate reap should not expire
        let reaped = inst_reg.reap_idle_instances();
        assert!(reaped.is_empty());

        // Fast-forward last_occupied_at to simulate expiration
        if let Some(inst) = inst_reg.instances.get_mut(&1001) {
            inst.last_occupied_at = Instant::now() - Duration::from_secs(2);
        }

        let reaped = inst_reg.reap_idle_instances();
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].instance_id, 1001);
        assert!(inst_reg.is_empty());
    }

    #[test]
    fn test_evict_instances_on_dead_nodes() {
        let mut node_reg = NodeRegistry::new();
        let n1 = node_reg.register_node("127.0.0.1:50051".to_string(), vec![]);
        let n2 = node_reg.register_node("127.0.0.1:50052".to_string(), vec![]);

        let mut inst_reg = InstanceRegistry::new();
        let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (10.0, 10.0, 10.0));

        inst_reg
            .allocate_instance(10, 1, bounds, 60, n1, &node_reg)
            .unwrap();
        inst_reg
            .allocate_instance(20, 1, bounds, 60, n2, &node_reg)
            .unwrap();

        let evicted = inst_reg.evict_instances_on_nodes(&[n1]);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].instance_id, 10);
        assert_eq!(inst_reg.len(), 1);
        assert!(inst_reg.get_instance(20).is_some());
    }
}
