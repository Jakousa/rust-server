## Purpose

Maintains cluster topology, tracks simulation node metrics and heartbeats, and commands dynamic octree subdivision and migration.

## ADDED Requirements

### Requirement: Node Registration and Health Monitoring
The Director SHALL maintain a registry of active simulation nodes and detect node failures via periodic heartbeats.

#### Scenario: Node sends heartbeat
- **WHEN** a simulation node reports tick metrics, entity count, and CPU usage
- **THEN** the Director SHALL update node health status and refresh its liveness lease

### Requirement: Global Octree Authority
The Director SHALL serve as the canonical authority for the global Sparse Octree mapping, assigning leaf octants to specific simulation nodes.

#### Scenario: Slice reassignment on split
- **WHEN** an octant requires subdivision due to high density
- **THEN** the Director SHALL generate 8 child octants, assign them to least-loaded nodes, and notify Edge Gateways of the new routing topology
