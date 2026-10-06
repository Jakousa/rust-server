## Purpose

Specifies the asynchronous write-behind entity snapshot pipeline, binary component blob serialization, and background SQL batch UPSERT mechanism.

## Requirements

### Requirement: Lock-Free State Extraction and Queueing
The simulation tick loop SHALL periodically extract entity snapshots from active entities and dump serialized binary state blobs into a non-blocking MPSC channel without performing database I/O or holding blocking mutexes.

#### Scenario: Non-blocking state dispatch during simulation tick
- **GIVEN** a running simulation node ticking at 20 Hz (50ms interval)
- **WHEN** the snapshot interval triggers or an entity is marked dirty
- **THEN** the simulation node SHALL serialize the entity's dynamic components into a binary state blob
- **AND** enqueue a `SnapshotPayload` into the lock-free MPSC channel within $< 5\,\mu\text{s}$ per entity without stalling the tick deadline

### Requirement: Generic Opaque Component Blob Persistence
The entity snapshot data schema SHALL remain generic and genre-agnostic, storing entity components as opaque serialized binary blobs (`state_blob`) and backward-compatible structured attributes (`attributes_json`), without game-specific schema assumptions.

#### Scenario: Generic component serialization and deserialization
- **GIVEN** an entity with dynamic components registered in `ComponentCodecRegistry`
- **WHEN** capturing an `EntitySnapshot`
- **THEN** the system SHALL serialize components into an opaque MessagePack byte sequence stored in `state_blob`
- **AND** on node restart, the system SHALL deserialize the opaque blob back into the entity's `ComponentStore` preserving component data fidelity

### Requirement: Background Worker Debouncing and Coalescing
The background snapshot worker SHALL dequeue snapshot payloads from the MPSC channel and coalesce updates for identical entity GUIDs within a debounce window before writing to SQL storage.

#### Scenario: Coalescing rapid movement updates
- **GIVEN** an entity updating its coordinates across consecutive 20 Hz ticks
- **WHEN** multiple snapshots for the same GUID arrive before the flusher drains the buffer
- **THEN** the worker SHALL coalesce them into the latest revision and discard superseded intermediate states
- **AND** only execute a single database UPSERT with the most recent version

### Requirement: Chunked Batch Database UPSERT with Failure Resilience
The storage repository SHALL persist snapshot batches using multi-row chunked `INSERT ... ON CONFLICT DO UPDATE` transactions against SQLite and PostgreSQL. If the database returns an error, pending records SHALL be retained in memory for automatic retry on the subsequent flush cycle.

#### Scenario: Database transient degradation recovery
- **GIVEN** an active write-behind snapshotter worker
- **WHEN** a batch flush to the SQL database fails due to connection loss or timeout
- **THEN** the snapshotter SHALL retain the failed entity snapshots in the coalescing buffer
- **AND** retry writing them on the next cycle without crashing the simulation node
