# Dynamic Spatial Partition Rebalancing Specification

## Purpose

Enables dynamic spatial partition rebalancing, hotspot saturation detection, continuous boundary smoothing, and zero-disconnect atomic batch entity migration across cluster simulation nodes.

## Requirements

### Requirement: Node Saturation Evaluation
The Cluster Director shall continuously evaluate incoming simulation node heartbeats and classify an octant as saturated if either entity count or tick execution duration exceeds configured thresholds.

#### Scenario: Saturation detection
- **GIVEN** a registered simulation node reporting heartbeats with `entity_count = E` and `tick_duration_ms = T`
- **WHEN** `E >= split_threshold` (default 500) OR `T >= tick_saturation_ms` (default 35.0ms)
- **THEN** the Director shall flag the node's assigned leaf octant for subdivision
- **AND** the `HeartbeatResponse` shall return `should_split = true` with the current `topology_epoch`

### Requirement: Least-Loaded Child Octant Assignment
When a leaf octant is subdivided into 8 child octants, the Director shall assign the child octants across available cluster nodes to balance computational load.

#### Scenario: Sub-domain provisioning
- **GIVEN** an octant split trigger on node $N_{source}$ with 8 child octants
- **WHEN** available nodes are evaluated in `NodeRegistry`
- **THEN** child octant 0 shall be retained by $N_{source}$ for spatial continuity
- **AND** the remaining 7 child octants shall be distributed across active nodes sorted by composite load score:
  $$\text{Load} = W_{entities} \cdot E + W_{cpu} \cdot \text{CPU}_{\%} + W_{tick} \cdot T_{tick}$$
- **AND** the Director's `topology_epoch` shall increment monotonically

### Requirement: Centroid Validation & Coordinate Clamping
Simulation nodes shall calculate spatial centroids strictly clamped to their assigned spatial bounds, and the Director shall validate and clamp reported centroids before applying boundary smoothing.

#### Scenario: Centroid clamping
- **GIVEN** a simulation node calculating its entity centroid
- **WHEN** individual entity coordinates are aggregated
- **THEN** all entity coordinates shall be validated and clamped within the node's assigned `BoundingBox3D`
- **AND** if any reported centroid coordinate falls outside the node's bounds in `HeartbeatRequest`, the Director shall clamp the coordinate to the partition boundary

### Requirement: Split Cooldown & Anti-Thrashing
The Director shall enforce a split cooldown on sub-domains to prevent runaway recursive subdivision while load stabilizes.

#### Scenario: Cooldown enforcement
- **GIVEN** a leaf octant that was recently split at timestamp $T_{\text{split}}$
- **WHEN** an incoming heartbeat indicates high load ($E \ge 500$ or $T \ge 35\text{ms}$)
- **THEN** no further split shall be triggered on that sub-domain until at least $10.0\text{s}$ has elapsed since $T_{\text{split}}$

### Requirement: Elastic Centroid-Repulsion Boundary Adjustment
Adjacent partition boundaries shall dynamically adjust along coordinate planes towards lower-density nodes, subject to maximum shift constraints and hysteresis damping.

#### Scenario: Boundary smoothing
- **GIVEN** two adjacent leaf octants assigned to node $A$ and node $B$ with clamped entity centroids $C_A$ and $C_B$
- **WHEN** node $A$ entity density exceeds node $B$ entity density by $> 2.0\times$
- **THEN** the separating boundary plane shall shift towards $C_B$ by an elastic displacement:
  $$\Delta d = \min\left(\text{max\_shift\_per\_epoch}, \; \alpha \cdot \frac{E_A - E_B}{E_A + E_B} \cdot \text{distance}(C_A, C_B)\right)$$
- **AND** the displacement shall be damped by hysteresis threshold ($\delta_{\text{hysteresis}} \ge 1.0\text{m}$) to prevent rapid oscillation

### Requirement: Atomic Multi-Entity All-or-Nothing Transfer
When a partition splits or boundaries adjust, entities within the reassigned spatial region shall be migrated to recipient nodes in atomic batches.

#### Scenario: Batch entity transfer
- **GIVEN** a set of $K$ entities ($1 \le K \le 500$) located within a reassigned sub-domain
- **WHEN** `execute_batch_entity_migration` is invoked by the donor node
- **THEN** migration leases shall be acquired atomically under a single write lock for all $K$ entities
- **AND** all entity components, kinematics, and session IDs shall be packaged into a single `BatchMigrateEntityRequest`
- **AND** the request shall be transmitted to the recipient node over a single gRPC call
- **AND** the recipient node shall validate all $K$ entities atomically; if any entity fails validation or insertion, the recipient shall reject the entire batch with `success = false`
- **AND** upon recipient confirmation (`success = true`), the point of no return is reached: donor node commits to despawning the $K$ entities locally without triggering death/combat drop hooks
- **AND** route updates for all $K$ entities shall be dispatched to the Edge Gateway via `BatchRouteUpdateRequest` with background retry on transient gateway network failure

### Requirement: Atomic Rollback on Failure Prior to Ingestion
If the recipient node rejects the batch, fails, or times out, the donor node shall preserve authoritative state.

#### Scenario: Batch migration rollback
- **GIVEN** a pending batch migration of $K$ entities
- **WHEN** the recipient node returns `success = false`, fails to respond within $500\text{ms}$, or connection fails
- **THEN** all $K$ migration leases shall be released on the donor node
- **AND** the donor node shall retain full simulation authority for all $K$ entities
- **AND** the `mmo_batch_migration_rollbacks_total` Prometheus counter shall increment by 1
- **AND** zero entities shall be lost, dropped, or duplicated
