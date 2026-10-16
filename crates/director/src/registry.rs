//! Simulation Node Registry and telemetry tracking.
//!
//! Stores registered simulation nodes, tracks their health heartbeats,
//! and detects expired nodes after timeouts.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use mmo_core::BoundingBox3D;
use mmo_schema::cluster::HeartbeatRequest;

/// Metadata and state for a registered simulation node.
#[derive(Debug, Clone)]
pub struct RegisteredNode {
    pub node_id: u32,
    pub listen_addr: String,
    pub last_heartbeat: Instant,
    pub entity_count: u32,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub tick_duration_ms: f32,
    pub entity_centroid: (f32, f32, f32),
    pub assigned_bounds: Vec<BoundingBox3D>,
}

impl RegisteredNode {
    /// Computes composite load score:
    /// Load = 0.5 * entities + 0.25 * cpu_percent + 0.25 * tick_duration_ms
    pub fn load_score(&self) -> f32 {
        let w_entities = 0.5;
        let w_cpu = 0.25;
        let w_tick = 0.25;
        (self.entity_count as f32 * w_entities)
            + (self.cpu_percent * w_cpu)
            + (self.tick_duration_ms * w_tick)
    }
}

/// Registry of active simulation nodes in the cluster.
#[derive(Debug, Default)]
pub struct NodeRegistry {
    nodes: HashMap<u32, RegisteredNode>,
    next_node_id: u32,
}

impl NodeRegistry {
    /// Creates a new, empty `NodeRegistry`.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            next_node_id: 1,
        }
    }

    /// Registers a new simulation node, allocating a new node ID.
    pub fn register_node(&mut self, listen_addr: String, bounds: Vec<BoundingBox3D>) -> u32 {
        let node_id = self.next_node_id;
        self.next_node_id += 1;

        let node = RegisteredNode {
            node_id,
            listen_addr,
            last_heartbeat: Instant::now(),
            entity_count: 0,
            cpu_percent: 0.0,
            memory_bytes: 0,
            tick_duration_ms: 0.0,
            entity_centroid: (0.0, 0.0, 0.0),
            assigned_bounds: bounds,
        };

        self.nodes.insert(node_id, node);
        node_id
    }

    /// Registers a simulation node with an optionally requested node ID.
    ///
    /// If `requested_id == 0`, a new node ID is dynamically allocated.
    /// If `requested_id > 0`, that ID is used and `next_node_id` is advanced.
    pub fn register_node_with_id(
        &mut self,
        requested_id: u32,
        listen_addr: String,
        bounds: Vec<BoundingBox3D>,
    ) -> u32 {
        if requested_id == 0 {
            return self.register_node(listen_addr, bounds);
        }

        let node_id = requested_id;
        self.next_node_id = self.next_node_id.max(node_id + 1);

        let node = RegisteredNode {
            node_id,
            listen_addr,
            last_heartbeat: Instant::now(),
            entity_count: 0,
            cpu_percent: 0.0,
            memory_bytes: 0,
            tick_duration_ms: 0.0,
            entity_centroid: (0.0, 0.0, 0.0),
            assigned_bounds: bounds,
        };

        self.nodes.insert(node_id, node);
        node_id
    }

    /// Records telemetry from a node heartbeat.
    ///
    /// Returns `true` if the node was found and updated, or `false` if unknown.
    pub fn record_heartbeat(&mut self, request: &HeartbeatRequest) -> bool {
        if let Some(node) = self.nodes.get_mut(&request.node_id) {
            node.last_heartbeat = Instant::now();
            node.entity_count = request.entity_count;
            node.cpu_percent = request.cpu_percent;
            node.memory_bytes = request.memory_bytes;
            if node.tick_duration_ms == 0.0 {
                node.tick_duration_ms = request.tick_duration_ms;
            } else {
                node.tick_duration_ms =
                    0.8 * node.tick_duration_ms + 0.2 * request.tick_duration_ms;
            }
            if let Some(c) = &request.entity_centroid {
                let mut centroid = (c.x, c.y, c.z);
                if let Some(bound) = node.assigned_bounds.first() {
                    centroid.0 = centroid.0.clamp(bound.min.0, bound.max.0);
                    centroid.1 = centroid.1.clamp(bound.min.1, bound.max.1);
                    centroid.2 = centroid.2.clamp(bound.min.2, bound.max.2);
                }
                node.entity_centroid = centroid;
            }
            true
        } else {
            false
        }
    }

    /// Scans for nodes whose last heartbeat exceeds `timeout_duration`.
    ///
    /// Removes expired nodes from the registry and returns their IDs.
    pub fn check_dead_nodes(&mut self, timeout_duration: Duration) -> Vec<u32> {
        let now = Instant::now();
        let mut dead_nodes = Vec::new();

        self.nodes.retain(|&id, node| {
            let elapsed = now.duration_since(node.last_heartbeat);
            if elapsed > timeout_duration {
                eprintln!(
                    "[Director] Node {} timed out! Elapsed: {:?} > {:?}",
                    id, elapsed, timeout_duration
                );
                dead_nodes.push(id);
                false
            } else {
                true
            }
        });

        dead_nodes.sort_unstable();
        dead_nodes
    }

    /// Explicitly deregisters a node by ID, returning its record if present.
    pub fn deregister_node(&mut self, node_id: u32) -> Option<RegisteredNode> {
        self.nodes.remove(&node_id)
    }

    /// Retrieves an immutable reference to a registered node.
    pub fn get_node(&self, node_id: u32) -> Option<&RegisteredNode> {
        self.nodes.get(&node_id)
    }

    /// Retrieves a mutable reference to a registered node.
    pub fn get_node_mut(&mut self, node_id: u32) -> Option<&mut RegisteredNode> {
        self.nodes.get_mut(&node_id)
    }

    /// Returns a list of all active registered nodes.
    pub fn active_nodes(&self) -> Vec<&RegisteredNode> {
        self.nodes.values().collect()
    }

    /// Returns a list of all active registered node IDs.
    pub fn active_node_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.nodes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Returns the total number of registered nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if no nodes are registered.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Finds the least loaded active node using composite load scoring (`load_score`).
    ///
    /// Ignores node IDs in `excluding`. If all nodes are excluded, falls back to any active node.
    pub fn find_least_loaded_node(&self, excluding: &[u32]) -> Option<u32> {
        let candidate = self
            .nodes
            .values()
            .filter(|n| !excluding.contains(&n.node_id))
            .min_by(|a, b| {
                a.load_score()
                    .partial_cmp(&b.load_score())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.node_id.cmp(&b.node_id))
            })
            .map(|n| n.node_id);

        candidate.or_else(|| {
            self.nodes
                .values()
                .min_by(|a, b| {
                    a.load_score()
                        .partial_cmp(&b.load_score())
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.node_id.cmp(&b.node_id))
                })
                .map(|n| n.node_id)
        })
    }

    /// Adds an assigned bounding box to the specified node.
    pub fn add_assigned_bounds(&mut self, node_id: u32, bounds: BoundingBox3D) {
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.assigned_bounds.push(bounds);
        }
    }

    /// Removes an assigned bounding box from the specified node.
    pub fn remove_assigned_bounds(&mut self, node_id: u32, bounds: &BoundingBox3D) {
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.assigned_bounds.retain(|b| b != bounds);
        }
    }

    /// Sets the last heartbeat timestamp directly (primarily useful for deterministic unit testing).
    pub fn set_last_heartbeat(&mut self, node_id: u32, time: Instant) {
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.last_heartbeat = time;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_registration_and_id_allocation() {
        let mut registry = NodeRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        let bounds1 = vec![BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0))];
        let id1 = registry.register_node("127.0.0.1:50051".to_string(), bounds1.clone());
        assert_eq!(id1, 1);
        assert_eq!(registry.len(), 1);

        let bounds2 = vec![BoundingBox3D::new((100.0, 0.0, 0.0), (200.0, 100.0, 100.0))];
        let id2 = registry.register_node("127.0.0.1:50052".to_string(), bounds2.clone());
        assert_eq!(id2, 2);
        assert_eq!(registry.len(), 2);

        // Requested ID
        let id10 = registry.register_node_with_id(10, "127.0.0.1:50060".to_string(), vec![]);
        assert_eq!(id10, 10);
        assert_eq!(registry.len(), 3);

        // Next automatic ID should be 11
        let id11 = registry.register_node("127.0.0.1:50061".to_string(), vec![]);
        assert_eq!(id11, 11);

        // Verify node properties
        let node1 = registry.get_node(1).expect("node 1 should exist");
        assert_eq!(node1.node_id, 1);
        assert_eq!(node1.listen_addr, "127.0.0.1:50051");
        assert_eq!(node1.entity_count, 0);
        assert_eq!(node1.assigned_bounds, bounds1);

        // Verify active_node_ids
        assert_eq!(registry.active_node_ids(), vec![1, 2, 10, 11]);

        // Explicit deregistration
        let dereg = registry.deregister_node(2);
        assert!(dereg.is_some());
        assert_eq!(dereg.unwrap().node_id, 2);
        assert_eq!(registry.len(), 3);
        assert!(registry.get_node(2).is_none());
    }

    #[test]
    fn test_heartbeat_updates_and_metrics() {
        let mut registry = NodeRegistry::new();
        let id = registry.register_node("127.0.0.1:50051".to_string(), vec![]);

        let hb = HeartbeatRequest {
            node_id: id,
            tick_rate: 20.0,
            entity_count: 75,
            cpu_percent: 23.5,
            memory_bytes: 1024 * 1024 * 128,
            timestamp: 123456789,
            tick_duration_ms: 15.0,
            entity_centroid: Some(mmo_schema::cluster::Vector3 {
                x: 5.0,
                y: 10.0,
                z: 15.0,
            }),
        };

        let updated = registry.record_heartbeat(&hb);
        assert!(updated);

        let node = registry.get_node(id).unwrap();
        assert_eq!(node.entity_count, 75);
        assert!((node.cpu_percent - 23.5).abs() < f32::EPSILON);
        assert_eq!(node.memory_bytes, 1024 * 1024 * 128);
        assert!((node.tick_duration_ms - 15.0).abs() < f32::EPSILON);
        assert_eq!(node.entity_centroid, (5.0, 10.0, 15.0));

        // Unknown node heartbeat should return false
        let unknown_hb = HeartbeatRequest {
            node_id: 999,
            tick_rate: 20.0,
            entity_count: 10,
            cpu_percent: 5.0,
            memory_bytes: 1024,
            timestamp: 123456789,
            tick_duration_ms: 5.0,
            entity_centroid: None,
        };
        assert!(!registry.record_heartbeat(&unknown_hb));
    }

    #[test]
    fn test_dead_node_detection_after_timeout() {
        let mut registry = NodeRegistry::new();
        let id1 = registry.register_node("127.0.0.1:50051".to_string(), vec![]);
        let id2 = registry.register_node("127.0.0.1:50052".to_string(), vec![]);
        let id3 = registry.register_node("127.0.0.1:50053".to_string(), vec![]);

        assert_eq!(registry.len(), 3);

        // Simulate id2 expired 10 seconds ago
        let expired_time = Instant::now() - Duration::from_secs(10);
        registry.set_last_heartbeat(id2, expired_time);

        // Timeout duration is 5 seconds
        let dead = registry.check_dead_nodes(Duration::from_secs(5));
        assert_eq!(dead, vec![id2]);
        assert_eq!(registry.len(), 2);
        assert!(registry.get_node(id2).is_none());
        assert!(registry.get_node(id1).is_some());
        assert!(registry.get_node(id3).is_some());

        // Subsequent check detects no further dead nodes
        let dead_second = registry.check_dead_nodes(Duration::from_secs(5));
        assert!(dead_second.is_empty());
    }

    #[test]
    fn test_find_least_loaded_node() {
        let mut registry = NodeRegistry::new();
        let id1 = registry.register_node("127.0.0.1:50051".to_string(), vec![]);
        let id2 = registry.register_node("127.0.0.1:50052".to_string(), vec![]);
        let id3 = registry.register_node("127.0.0.1:50053".to_string(), vec![]);

        registry.record_heartbeat(&HeartbeatRequest {
            node_id: id1,
            tick_rate: 20.0,
            entity_count: 100,
            cpu_percent: 50.0,
            memory_bytes: 0,
            timestamp: 0,
            tick_duration_ms: 25.0,
            entity_centroid: None,
        });

        registry.record_heartbeat(&HeartbeatRequest {
            node_id: id2,
            tick_rate: 20.0,
            entity_count: 15,
            cpu_percent: 10.0,
            memory_bytes: 0,
            timestamp: 0,
            tick_duration_ms: 5.0,
            entity_centroid: None,
        });

        registry.record_heartbeat(&HeartbeatRequest {
            node_id: id3,
            tick_rate: 20.0,
            entity_count: 45,
            cpu_percent: 20.0,
            memory_bytes: 0,
            timestamp: 0,
            tick_duration_ms: 12.0,
            entity_centroid: None,
        });

        assert_eq!(registry.find_least_loaded_node(&[]), Some(id2));
        assert_eq!(registry.find_least_loaded_node(&[id2]), Some(id3));
        assert_eq!(registry.find_least_loaded_node(&[id2, id3]), Some(id1));
    }

    #[test]
    fn test_assigned_bounds_modification() {
        let mut registry = NodeRegistry::new();
        let id = registry.register_node("127.0.0.1:50051".to_string(), vec![]);

        let bbox1 = BoundingBox3D::new((0.0, 0.0, 0.0), (10.0, 10.0, 10.0));
        let bbox2 = BoundingBox3D::new((10.0, 0.0, 0.0), (20.0, 10.0, 10.0));

        registry.add_assigned_bounds(id, bbox1);
        registry.add_assigned_bounds(id, bbox2);

        let node = registry.get_node(id).unwrap();
        assert_eq!(node.assigned_bounds.len(), 2);

        registry.remove_assigned_bounds(id, &bbox1);
        let node = registry.get_node(id).unwrap();
        assert_eq!(node.assigned_bounds.len(), 1);
        assert_eq!(node.assigned_bounds[0], bbox2);
    }
}

/// Status of an edge gateway in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayStatus {
    Healthy,
    Degraded,
    Draining,
    Offline,
}

/// Metadata and state for a registered edge gateway.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayDescriptor {
    pub gateway_id: String,
    pub region: String,
    pub listen_addr: String,
    pub grpc_addr: String,
    pub probe_addr: String,
    pub capacity: u32,
    pub active_sessions: u32,
    pub cpu_percent: f32,
    pub memory_percent: f32,
    pub last_heartbeat: Instant,
    pub status: GatewayStatus,
}

impl GatewayDescriptor {
    pub fn new(
        gateway_id: String,
        region: String,
        listen_addr: String,
        grpc_addr: String,
        probe_addr: String,
        capacity: u32,
    ) -> Self {
        Self {
            gateway_id,
            region,
            listen_addr,
            grpc_addr,
            probe_addr,
            capacity,
            active_sessions: 0,
            cpu_percent: 0.0,
            memory_percent: 0.0,
            last_heartbeat: Instant::now(),
            status: GatewayStatus::Healthy,
        }
    }
}

/// Registry of active edge gateways in the cluster.
#[derive(Debug, Default)]
pub struct GatewayRegistry {
    gateways: HashMap<String, GatewayDescriptor>,
}

impl GatewayRegistry {
    /// Creates a new, empty `GatewayRegistry`.
    pub fn new() -> Self {
        Self {
            gateways: HashMap::new(),
        }
    }

    /// Registers or updates a gateway in the registry.
    pub fn register_gateway(&mut self, descriptor: GatewayDescriptor) {
        self.gateways
            .insert(descriptor.gateway_id.clone(), descriptor);
    }

    /// Updates heartbeat metrics for an active gateway.
    pub fn update_heartbeat(
        &mut self,
        gateway_id: &str,
        active_sessions: u32,
        cpu_percent: f32,
        memory_percent: f32,
    ) -> bool {
        if let Some(gateway) = self.gateways.get_mut(gateway_id) {
            gateway.active_sessions = active_sessions;
            gateway.cpu_percent = cpu_percent;
            gateway.memory_percent = memory_percent;
            gateway.last_heartbeat = Instant::now();
            gateway.status = if cpu_percent > 80.0 || memory_percent > 80.0 {
                GatewayStatus::Degraded
            } else {
                GatewayStatus::Healthy
            };
            true
        } else {
            false
        }
    }

    /// Gets a reference to a registered gateway by its ID.
    pub fn get_gateway(&self, gateway_id: &str) -> Option<&GatewayDescriptor> {
        self.gateways.get(gateway_id)
    }

    /// Returns all active, non-offline gateways.
    pub fn get_active_gateways(&self) -> Vec<GatewayDescriptor> {
        self.gateways
            .values()
            .filter(|g| g.status != GatewayStatus::Offline)
            .cloned()
            .collect()
    }

    /// Returns all gateways in a specific geographical region.
    pub fn get_gateways_by_region(&self, region: &str) -> Vec<GatewayDescriptor> {
        self.gateways
            .values()
            .filter(|g| g.region == region && g.status != GatewayStatus::Offline)
            .cloned()
            .collect()
    }

    /// Manually sets the last heartbeat timestamp (for testing).
    #[cfg(test)]
    pub fn set_last_heartbeat(&mut self, gateway_id: &str, time: Instant) {
        if let Some(g) = self.gateways.get_mut(gateway_id) {
            g.last_heartbeat = time;
        }
    }

    /// Scans for stale gateways exceeding `timeout`, marks them `Offline`, and returns their IDs.
    pub fn remove_stale_gateways(&mut self, timeout: Duration) -> Vec<String> {
        let now = Instant::now();
        let stale_gateways: Vec<String> = self
            .gateways
            .iter()
            .filter(|(_, g)| {
                g.status != GatewayStatus::Offline && now.duration_since(g.last_heartbeat) > timeout
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in &stale_gateways {
            if let Some(gateway) = self.gateways.get_mut(id) {
                gateway.status = GatewayStatus::Offline;
            }
        }

        stale_gateways
    }

    /// Explicitly deregisters a gateway from the registry.
    pub fn deregister_gateway(&mut self, gateway_id: &str) -> Option<GatewayDescriptor> {
        self.gateways.remove(gateway_id)
    }

    /// Returns the total number of registered gateways.
    pub fn gateway_count(&self) -> usize {
        self.gateways.len()
    }
}

#[cfg(test)]
mod gateway_tests {
    use super::*;

    #[test]
    fn test_gateway_registration_and_retrieval() {
        let mut registry = GatewayRegistry::new();
        let gw = GatewayDescriptor::new(
            "gw-us-1".to_string(),
            "us-east".to_string(),
            "127.0.0.1:8080".to_string(),
            "127.0.0.1:50050".to_string(),
            "127.0.0.1:8081".to_string(),
            1000,
        );
        registry.register_gateway(gw.clone());

        assert_eq!(registry.gateway_count(), 1);
        assert_eq!(registry.get_gateway("gw-us-1"), Some(&gw));
        assert_eq!(registry.get_active_gateways().len(), 1);
    }

    #[test]
    fn test_gateway_heartbeat_and_degradation() {
        let mut registry = GatewayRegistry::new();
        let gw = GatewayDescriptor::new(
            "gw-eu-1".to_string(),
            "eu-central".to_string(),
            "127.0.0.1:8080".to_string(),
            "127.0.0.1:50050".to_string(),
            "127.0.0.1:8081".to_string(),
            1000,
        );
        registry.register_gateway(gw);

        assert!(registry.update_heartbeat("gw-eu-1", 450, 85.0, 40.0));
        let updated = registry.get_gateway("gw-eu-1").unwrap();
        assert_eq!(updated.active_sessions, 450);
        assert_eq!(updated.status, GatewayStatus::Degraded);
    }

    #[test]
    fn test_gateway_regional_filtering() {
        let mut registry = GatewayRegistry::new();
        let gw1 = GatewayDescriptor::new(
            "gw-us-1".to_string(),
            "us-east".to_string(),
            "127.0.0.1:8080".to_string(),
            "127.0.0.1:50050".to_string(),
            "127.0.0.1:8081".to_string(),
            1000,
        );
        let gw2 = GatewayDescriptor::new(
            "gw-eu-1".to_string(),
            "eu-central".to_string(),
            "127.0.0.1:8082".to_string(),
            "127.0.0.1:50052".to_string(),
            "127.0.0.1:8083".to_string(),
            1000,
        );
        registry.register_gateway(gw1.clone());
        registry.register_gateway(gw2.clone());

        let us = registry.get_gateways_by_region("us-east");
        assert_eq!(us.len(), 1);
        assert_eq!(us[0].gateway_id, "gw-us-1");

        let eu = registry.get_gateways_by_region("eu-central");
        assert_eq!(eu.len(), 1);
        assert_eq!(eu[0].gateway_id, "gw-eu-1");
    }

    #[test]
    fn test_gateway_stale_detection_and_offline() {
        let mut registry = GatewayRegistry::new();
        let gw = GatewayDescriptor::new(
            "gw-stale-1".to_string(),
            "ap-southeast".to_string(),
            "127.0.0.1:8080".to_string(),
            "127.0.0.1:50050".to_string(),
            "127.0.0.1:8081".to_string(),
            1000,
        );
        registry.register_gateway(gw);

        // Fast-forward last heartbeat using virtual time
        let stale_time = Instant::now() - Duration::from_secs(10);
        registry.set_last_heartbeat("gw-stale-1", stale_time);

        let stale = registry.remove_stale_gateways(Duration::from_secs(3));
        assert_eq!(stale, vec!["gw-stale-1"]);

        let g = registry.get_gateway("gw-stale-1").unwrap();
        assert_eq!(g.status, GatewayStatus::Offline);
        assert!(registry.get_active_gateways().is_empty());
    }
}
