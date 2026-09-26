# Control Director Specification Delta

## ADDED Requirements

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
