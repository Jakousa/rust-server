# Node Split Migration Specification

## Purpose
Defines authoritative spatial octant splitting, partition propagation, standby registration safety, and live batch entity migration across `mmo-director` and `mmo-node`.

## ADDED Requirements

### Requirement: Director Split-Loop Protection and Balanced Octant Assignment
`mmo-director` SHALL prevent infinite split loops and distribute child octants across active nodes with routable network addresses.

#### Scenario: Standby node registration does not steal active root partition
- **GIVEN** `Node 1` is active and registered as the owner of the root world bounds `(-100..100)^3`
- **WHEN** `Node 2` registers with default world bounds `(-100..100)^3`
- **THEN** `mmo-director` SHALL preserve `Node 1` as the owner of the root partition and initialize `Node 2` with no assigned bounds awaiting future splits.

#### Scenario: Already-split or sub-minimum bounds are not re-split
- **GIVEN** a leaf bound that has already been split into a branch or whose axis extent is smaller than `MIN_SPLIT_EXTENT` (`25.0`)
- **WHEN** `should_split_node` or `handle_split` is evaluated
- **THEN** `mmo-director` SHALL NOT re-split the branch or increment `topology_epoch`, and SHALL return existing child assignments instead.

#### Scenario: Multi-node split distributes octants and normalizes unspecified addresses
- **GIVEN** `Node 1` and `Node 2` are active in `NodeRegistry` with listen addresses bound to `0.0.0.0:50052`
- **WHEN** `ReportSplit` splits the root bounds
- **THEN** `mmo-director` SHALL distribute the 8 child octants across active nodes and return routable DNS addresses (`mmo-node-{id-1}.mmo-node:50052`) in `SplitReportResponse.partitions` and `assigned_node_addresses`.

### Requirement: Live Entity Batch Migration on Split and Boundary Crossing
Simulation nodes SHALL update local partition ownership and migrate peer-owned entities when topology changes or entities cross partition boundaries.

#### Scenario: Split response triggers batch migration to peer nodes
- **GIVEN** `Node 1` holds movable entities distributed across multiple octants
- **WHEN** `Node 1` receives `SplitReportResponse` assigning octants to `Node 2`
- **THEN** `Node 1` SHALL update `TickSimulation::owned_bounds` and `peer_partitions`, execute `execute_batch_entity_migration_pooled` for entities in `Node 2`'s octants, remove migrated entities from local storage, and update Gateway session routes via `UpdateBatchRoutes`.
