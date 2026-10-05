# Schemaless Document Store Specification

## Purpose
Provides high-performance in-memory ingestion and indexing of schemaless asset definitions (JSON and MessagePack), dynamic property attachment for simulation entities, and runtime content iteration without Rust backend recompilation.

---

## Requirements

### Requirement: Schemaless Asset Ingestion at Startup
The storage tier SHALL provide a generic `DataStore` that ingests schemaless asset documents in JSON and MessagePack (`.mpk`/`.msgpack`) formats from file paths and directory trees into memory upon node startup.

#### Scenario: Recursive directory ingestion with error accumulation
- **GIVEN** a directory hierarchy containing JSON and MessagePack asset files categorized in subfolders (e.g. `data/items/`, `data/monsters/`)
- **WHEN** `data_store.load_directory(...)` is called
- **THEN** all valid documents SHALL be parsed and registered into in-memory collections mapped to their respective subfolder or document collection identifier
- **AND** invalid files SHALL be recorded in an error list without aborting ingestion of valid files
- **AND** file extension checks SHALL be case-insensitive (e.g. `.JSON`, `.MsgPack`).

#### Scenario: Multi-document ingestion
- **GIVEN** a file containing an array of asset objects or a map keyed by document ID
- **WHEN** the file is ingested by the `DataStore`
- **THEN** each individual entry SHALL be indexed as an independent `DataDocument` addressable by its ID.

---

### Requirement: Wait-Free In-Memory Document Query and Retrieval
The `DataStore` SHALL provide thread-safe, wait-free concurrent O(1) retrieval of documents by collection and document ID, collection enumeration, and collection-isolated indexing.

#### Scenario: Query document by collection and ID
- **GIVEN** an ingested collection `"weapons"` containing document `"iron_sword"`
- **WHEN** `data_store.get("weapons", "iron_sword")` is called
- **THEN** it SHALL return an `Arc<DataDocument>` containing its arbitrary schemaless properties without lock contention or cache-line bouncing.

#### Scenario: Query non-existent document
- **GIVEN** a query for a non-existent document ID
- **WHEN** `data_store.get("items", "non_existent")` is called
- **THEN** it SHALL return `None`.

#### Scenario: Isolated collection namespaces
- **GIVEN** two different collections `"items"` and `"monsters"`, both containing an ID `"101"`
- **WHEN** querying `"items"` for `"101"` and `"monsters"` for `"101"`
- **THEN** each query SHALL return its respective collection's document without collision or overwrite.

---

### Requirement: Dynamic Entity Property Attachment
The core engine SHALL provide `DynamicProperties` and `PropertyValue` primitives enabling simulation entities to attach, read, modify, and serialize arbitrary designer-defined properties without static Rust type declarations in core crates.

#### Scenario: Populating entity properties from ingested definition
- **GIVEN** an entity in `EntityStore` and an ingested document in `DataStore` with arbitrary keys (e.g. `{"durability": 150, "rarity": "rare", "element": "fire"}`)
- **WHEN** `data_store.apply_to_entity(collection, id, entity)` is executed
- **THEN** the entity SHALL have all properties from the document attached to its dynamic properties store
- **AND** `entity.get_property_str("element")` SHALL return `Some("fire")`
- **AND** `entity.get_property_i64("durability")` SHALL return `Some(150)`.

#### Scenario: Dotted-path property queries with defensive traversal
- **GIVEN** a nested property hierarchy on an entity (e.g. `{"stats": {"elemental": {"fire": 25.5}}}`)
- **WHEN** `entity.properties.get_path("stats.elemental.fire")` is called
- **THEN** it SHALL resolve and return the nested `PropertyValue::Float(25.5)`
- **AND** if an intermediate path segment is a non-object scalar or missing (e.g. `entity.properties.get_path("stats.non_existent.value")`), it SHALL gracefully return `None` without panic.

---

### Requirement: Content Iteration Without Rust Backend Recompilation
Game designers SHALL be able to modify, add, or delete asset definitions in JSON/MessagePack data files and load them into a running or restarted simulation node without modifying or recompiling Rust source code.

#### Scenario: Balancing game values via JSON modification
- **GIVEN** an item asset file defining an attack bonus of `10`
- **WHEN** the designer edits the JSON file to set the attack bonus to `15` and reloads it into `DataStore`
- **THEN** newly instantiated entities referencing that definition SHALL reflect the new value `15` without recompilation of any engine crates.

---

### Requirement: Prime Directive Genre-Agnostic Engine Core
All core engine primitives (`crates/core`, `crates/storage`) SHALL remain 100% free of game-specific or genre-specific assumptions, terminology, or schemas.

#### Scenario: Absence of game-specific schemas in core
- **WHEN** inspecting types and methods in `crates/core` and `crates/storage`
- **THEN** no types or methods SHALL refer to game-specific concepts (such as swords, spells, potions, or RPG attributes)
- **AND** all dynamic mechanics SHALL be expressed through generic key-value, nested document, and pluggable component abstractions.
