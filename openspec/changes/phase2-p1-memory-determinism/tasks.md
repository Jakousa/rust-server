## 1. Zero-Copy Interaction Wire Codec

- [x] 1.1 Update `InteractionPacket.payload` in `crates/schema/src/client.rs` from `Vec<u8>` to `bytes::Bytes`, updating decode/encode framing to use zero-copy `copy_to_bytes` on `impl Buf`, update `ClientPacket::decode_frame` to freeze frames as `Bytes`, and verify with `cargo test -p mmo-schema`.
- [x] 1.2 Update all call sites and tests constructing or consuming `InteractionPacket` across `crates/gateway`, `crates/loadtest`, and `crates/node`, and verify compilation with `cargo check --workspace`.

## 2. AoI Allocations & Selection Optimization

- [x] 2.1 Refactor `AoiManager::throttle_density` in `crates/node/src/aoi.rs` to use $O(N)$ `select_nth_unstable_by` strictly guarded by `if candidates.len() > budget`, preventing out-of-bounds panics when candidate count is within budget.
- [x] 2.2 Equip `AoiManager` with reusable internal scratch buffers for entity candidate collection and visibility delta diffing, and verify with `cargo test -p mmo-node -- aoi`.

## 3. Event-Driven Despawn Cleanup & Clamped Simulation Loop

- [x] 3.1 Refactor `TickSimulation::despawn_entity` to push despawned GUIDs into `TickSimulation::event_queue`, and update `SpawnerPlugin` and `ThreatPlugin` in `crates/gameplay` to prune dead/despawned entities reactively (checking only a spawner's own tracked entities rather than scanning all world entities). Verify with `cargo test -p mmo-gameplay`.
- [x] 3.2 Implement a clamped fixed-step tick accumulator in `TickSimulation::run_ticks` and `crates/node/src/main.rs` using a monotonic `Instant::now()` accumulator loop that caps catch-up ticks at `MAX_CATCHUP_TICKS = 3` and yields to Tokio, preventing tick starvation loops. Verify with simulation tick tests.

## 4. Storage Hardening & Bounded Snapshot Queue

- [x] 4.1 Configure `PRAGMA journal_mode=WAL` and `PRAGMA synchronous=NORMAL` on SQLite connection pools in `crates/storage/src/db.rs`, and verify with `cargo test -p mmo-storage`.
- [x] 4.2 Enforce a maximum dirty queue capacity (`max_dirty_capacity = 50_000`) with update coalescing for existing GUIDs and chunked batch writes (`chunk_size = 500`) on `WriteBehindSnapshotter` in `crates/node/src/snapshotter.rs`, and verify with `cargo test -p mmo-node -- snapshotter`.

## 5. End-to-End Verification & Workspace Quality

- [x] 5.1 Run `cargo test --workspace` to ensure all unit, cluster, and e2e integration tests pass.
- [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings` to verify zero warnings.

