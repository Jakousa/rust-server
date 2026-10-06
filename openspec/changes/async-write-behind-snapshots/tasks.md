# Implementation Tasks: Asynchronous Write-Behind Entity State Snapshots

## Task 1: Data Model & Schema Enhancement (`crates/storage` and `crates/core`)
- [x] 1.1 In `crates/storage/src/models.rs`, add `pub state_blob: Option<Vec<u8>>` to `EntitySnapshot` with `#[serde(default)]` and update serialization/deserialization.
- [x] 1.2 In `crates/storage/src/db.rs`, add `state_blob` column migration for both SQLite (`BLOB`) and PostgreSQL (`BYTEA`), ensuring backward-compatible idempotent execution (`ensure_entity_snapshots_blob_column`).
- [x] 1.3 In `crates/storage/src/repository.rs`, update `save_snapshot`, `save_snapshot_batch`, and `get_snapshot` to persist and retrieve the `state_blob` column alongside `attributes_json`.
- [x] 1.4 In `crates/core/src/plugin.rs`, add binary serialization helpers (`serialize_all_binary` and `deserialize_binary_into`) using MessagePack (`rmp_serde`) to encode dynamic components generically with bounded buffer validation.

## Task 2: Lock-Free Bounded Pipeline & Debounced Background Worker (`crates/node`)
- [x] 2.1 In `crates/node/src/snapshotter.rs`, implement `SnapshotPayload` and update `WriteBehindSnapshotter` to provide a bounded MPSC channel (`mpsc::channel(100_000)`) with non-blocking `try_send()`.
- [x] 2.2 Implement debounced coalescing background worker in `WriteBehindSnapshotter` with monotonic version ordering (`incoming.version >= existing.version`), $O(1)$ `std::mem::take` buffer swapping, and priority retention for initial entity spawns when at capacity.
- [x] 2.3 Implement chunked SQL batch flusher with failure retry merge logic on database disconnects.
- [x] 2.4 Add telemetry metrics (`mmo_node_snapshot_dirty_queue`, `mmo_node_snapshot_flush_duration_seconds`, `mmo_node_snapshot_flushed_total`, `mmo_node_snapshot_dropped_channel_full`).

## Task 3: Simulation Tick Integration & Periodic Extraction (`crates/node`)
- [x] 3.1 In `crates/node/src/sim.rs`, integrate an optional bounded `snapshot_sender: Option<mpsc::Sender<SnapshotPayload>>` into `TickSimulation`, with configurable snapshot intervals (e.g., default every 20 ticks / 1 second or dirty triggers).
- [x] 3.2 In `TickSimulation::step`, periodically extract entity states and dispatch binary snapshot payloads into the bounded channel via `try_send()` without taking blocking locks.
- [x] 3.3 In `crates/node/src/main.rs`, wire up `WriteBehindSnapshotter` with `TickSimulation` and spawn the background worker when `DATABASE_URL` is configured.

## Task 4: Unit & Property Tests (Using Local Subagent)
- [x] 4.1 Use local subagent to write unit tests in `crates/storage` verifying binary `state_blob` roundtrips and batch UPSERTs with version conflict resolution.
- [x] 4.2 Use local subagent to write unit tests in `crates/node/src/snapshotter.rs` verifying bounded MPSC channel non-blocking behavior, debouncing, monotonic coalescing, and retry on DB failure.
- [x] 4.3 Update end-to-end integration tests in `crates/node/tests/it/persistence.rs` validating full crash recovery with binary component state blobs.

## Task 5: Quality Gates & Verification
- [x] 5.1 Run full workspace linter: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 5.2 Run workspace test suite: `cargo test --workspace`.
- [x] 5.3 Verify formatting: `cargo fmt --all -- --check`.
- [ ] 5.4 Submit PR and enable auto-merge.
