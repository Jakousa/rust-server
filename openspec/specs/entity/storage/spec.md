# Entity Storage Specification

## Purpose

Manages in-memory entity lifecycles, 64-bit structured GUIDs, and dynamic component attachments inside simulation nodes.

## Requirements

### Requirement: 64-bit Structured GUID
The system SHALL assign a 64-bit integer GUID to every entity, encoding 16 bits of Entity Type, 16 bits of Node ID, and 32 bits of Sequence Counter.

#### Scenario: GUID creation and inspection
- **WHEN** a new entity is created on a simulation node
- **THEN** the system SHALL assign an 8-byte GUID whose high 16 bits identify the entity type and node origin

### Requirement: Generational Arena Storage
The simulation node SHALL store active entities in a generational arena (SlotMap), providing O(1) retrieval and protection against ABA reuse bugs.

#### Scenario: O(1) entity lookup
- **WHEN** an entity key is queried on the local node
- **THEN** the system SHALL return a reference to the entity in O(1) time or None if it has despawned or migrated

### Requirement: Dynamic Component Attachment
The system SHALL allow modular components to attach arbitrary typed structs to an Entity via a type map without modifying core entity definitions.

#### Scenario: Attaching modular component
- **WHEN** an extension module registers data (such as Inventory) on an entity
- **THEN** the module SHALL be able to retrieve and mutate that component via `entity.get::<T>()` and `entity.get_mut::<T>()`
