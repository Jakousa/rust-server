# simulation/aoi Specification

## Purpose
Tracks per-player spatial visibility in the Sparse Octree, computes dynamic entity visibility deltas, and generates authoritative Area-of-Interest (AoI) broadcast packets.

## Requirements

### Requirement: Player Area-of-Interest Tracking
The simulation node SHALL track active player sessions as AoI observers and query the spatial Octree for all entities within each player's visibility radius.

#### Scenario: Observer queries nearby entities
- **WHEN** an authoritative simulation tick executes for an active player session
- **THEN** the system SHALL perform a 3D spherical or bounding box range query centered on the player position using their effective visibility radius

### Requirement: Visibility Delta State Generation
The simulation node SHALL compare each player's current visible entity set against their previously observed entity set to compute enters, updates, and exits.

#### Scenario: Entity enters visibility radius
- **WHEN** an entity appears within a player's visibility radius that was not present in the previous tick
- **THEN** the system SHALL generate an `EntityState` packet representing the entity's current state and record the entity in the player's observed set

#### Scenario: Entity exits visibility radius or despawns
- **WHEN** an entity that was present in the player's previous observed set is no longer found in the current visibility query
- **THEN** the system SHALL generate an `EntityDespawn` packet targeted at the observing player session and remove the entity from the player's observed set

#### Scenario: Entity moves or updates within visibility radius
- **WHEN** an observed entity's position, velocity, or health changes while remaining within the player's visibility radius
- **THEN** the system SHALL generate an updated `EntityState` broadcast packet for the observing player session

### Requirement: Dynamic Visibility Throttling
The system SHALL dynamically clamp the observer's visibility radius when the number of visible entities exceeds the configured per-observer density budget.

#### Scenario: High entity density throttling
- **WHEN** the count of entities within the player's default visibility radius exceeds the configured maximum entity threshold
- **THEN** the system SHALL reduce the effective visibility radius to cap broadcast packet volume and client bandwidth
