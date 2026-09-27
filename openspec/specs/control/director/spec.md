# Control Director Specification

## Purpose

Maintains cluster topology, tracks simulation node metrics and heartbeats, and commands dynamic octree subdivision and migration.

## Requirements

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

### Requirement: Orphaned Spatial Partition Recovery
The Director SHALL automatically detect and reassign all spatial bounding boxes belonging to an evicted, timed-out, or deregistered simulation node to active surviving nodes.

#### Scenario: Node eviction reassigns orphaned leaves
- **GIVEN** a cluster where a simulation node has been assigned one or more spatial leaf octants
- **WHEN** the node fails to send heartbeats and is evicted by the dead node detector
- **THEN** the Director SHALL reassign every leaf octant previously assigned to the dead node to an active, least-loaded surviving node
- **AND** the Director SHALL update the spatial index and node registry to match the reassignment
- **AND** no coordinate within the world bounds SHALL become unassigned or point to an evicted node.

### Requirement: Generational Topology Epoch Fencing
The Director SHALL maintain a monotonically increasing topology epoch counter that increments upon every spatial partitioning mutation.

#### Scenario: Topology mutation increments epoch
- **WHEN** an octant is split, merged, or recovered from a dead node
- **THEN** the Director SHALL increment its topology epoch counter
- **AND** the new epoch SHALL be recorded in partition metadata to prevent stale routing application.
