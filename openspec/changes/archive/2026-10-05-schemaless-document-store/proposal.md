# Proposal: Schemaless Document Store for Data-Driven Assets

## Motivation & Background
In accordance with the project's Prime Directive (`AGENTS.md`), the core engine crates (`crates/core`, `crates/storage`, `crates/node`, `crates/schema`) must remain strictly **genre-agnostic, unopinionated, and mathematically generalized**. The engine core must not possess hardcoded domain concepts such as "Swords", "Spells", "Armor Tiers", "Laser Blasters", or "Healing Potions".

Enforcing rigid compile-time Rust schemas in engine crates severely constrains game designers and content developers, requiring full engine recompilation and redeployment for every balance pass, item definition, or NPC archetype addition.

To deliver true data-driven game development, the MMO engine requires:
1. A generic, high-performance **Schemaless Document Store (`DataStore`)** in `crates/storage` capable of ingesting arbitrary JSON and MessagePack asset files at startup or runtime.
2. A generic **Dynamic Property Primitive (`DynamicProperties` / `PropertyValue`)** in `crates/core` that enables arbitrary key-value and hierarchical property attachment on game entities.
3. Decoupled **Gameplay Interpretation** in `crates/gameplay` that reads dynamic properties from entities (such as stats, damage modifiers, elemental resistances, and spawner templates) without embedding rigid definitions into the engine core.

---

## Architectural Objectives

1. **Schemaless Asset Ingestion (`mmo-storage`)**:
   - Provide a generic `DataStore` that ingests schemaless formats (JSON and MessagePack) into fast, concurrent in-memory indices.
   - Support file and recursive directory scanning, inferring collection names from directory hierarchy or document metadata.
   - Support multiple document layouts: single document objects, arrays of documents, and dictionary maps keyed by ID.

2. **Dynamic Entity Property Primitives (`mmo-core`)**:
   - Implement `PropertyValue` supporting primitives (`Null`, `Bool`, `Int`, `Float`, `String`, `Array`, `Object`) with seamless bidirectional conversion with `serde_json::Value`.
   - Implement `DynamicProperties` providing typed getters, dotted-path querying (`combat.elements.fire`), merging, and serialization codecs.
   - Integrate `DynamicProperties` natively into `Entity` while preserving generational `SlotMap` performance and zero-cost abstraction.

3. **Data-Driven Pipeline & Gameplay Decoupling (`mmo-gameplay`)**:
   - Provide helper utilities to instantiate or augment entities from ingested `DataStore` definitions (`datastore.apply_to_entity(...)`).
   - Adapt gameplay systems (combat, spawner, items) to optionally interpret dynamic properties, allowing content balance updates purely via JSON/MessagePack file changes.

4. **Strict Genre-Agnostic Compliance**:
   - Core and storage crates remain 100% agnostic to specific game rules, mechanics, or genres.

---

## Non-Goals
- Baking game-specific item, spell, or monster schemas into `crates/core` or `crates/storage`.
- Replacing the relational database (PostgreSQL/SQLite) for dynamic player transaction state (player accounts, character inventories, and container transactions remain strongly tracked in relational tables).
- Distributed multi-master document synchronization across clusters (the document store is a read-heavy, authoritative static asset repository loaded locally per node on startup).
