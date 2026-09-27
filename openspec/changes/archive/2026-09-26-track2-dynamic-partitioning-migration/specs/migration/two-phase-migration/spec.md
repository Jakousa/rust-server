# Capability Specification: Two-Phase Entity Migration Protocol

## Domain: Entity Migration

### Feature: Authoritative Live Entity Handovers

---

#### Scenario: Phase 1 Lease Acquisition & Strict State Freeze
- **GIVEN** an active entity on a donor simulation node determined to migrate to a recipient node
- **WHEN** migration is initiated
- **THEN** the donor node acquires a migration lease marking the entity status as `Migrating`
- **AND** client movement packets, `RemoteCommand` interactions, and tick systems received during `Migrating` status are strictly frozen (interactions rejected or deferred with `Vetoed`) to ensure total state immutability while the simulation lock is released during async network I/O
- **AND** the donor node extracts entity kinematics (position, velocity, yaw, pitch, last sequence, last migration tick) and serializes all attached component data into a `MigrateEntityRequest`.

#### Scenario: Phase 2 Recipient Adoption & Acknowledgment
- **GIVEN** a recipient simulation node receiving a valid `MigrateEntityRequest`
- **WHEN** the recipient validates that the entity's coordinates fall within its authoritative partition bounds
- **THEN** the recipient inserts the entity into its `EntityStore` and `Octree`
- **AND** restores all serialized components, kinematic tracking states (`last_sequence`), and `last_migration_tick` cooldown
- **AND** registers an AoI observer if an active client session ID is attached
- **AND** returns a successful `MigrateEntityResponse` to the donor.

#### Scenario: Gateway Route Commit on Handover Confirmation
- **GIVEN** the donor simulation node receives a successful `MigrateEntityResponse` from the recipient
- **WHEN** the acknowledgment is verified
- **THEN** the donor node (or recipient upon confirmed transfer) issues the `UpdateRoute` gRPC request to the Edge Gateway
- **AND** the donor node cleanly despawns the local entity from its `EntityStore` and `Octree`
- **AND** removes the entity's kinematic tracking state and releases the migration lease
- **AND** does NOT dispatch player death or client disconnect events.

#### Scenario: Zero-Duplication Failure Rollback & Verification
- **GIVEN** an in-flight migration where the gRPC call fails or times out before receiving acknowledgment
- **WHEN** the failure is detected by the donor node
- **THEN** the donor node verifies whether the recipient adopted the entity before rolling back
- **AND** if unadopted, the donor node clears the `Migrating` lease, restores the entity to active status, and clamps the entity to the interior of the donor partition boundary
- **AND** the Edge Gateway route is not updated, guaranteeing sole authoritative ownership on the donor node with zero duplicate entities.
