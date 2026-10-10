# Proposal: Fix Nightly Mutation Testing Suite Failures

## Context & Motivation
The scheduled nightly mutation testing workflow (`.github/workflows/mutation-tests.yml`) executes at 03:00 UTC daily across packages `mmo-schema` and `mmo-storage`. In recent nightly runs, both jobs in the matrix have failed consistently with exit code 2:
1. `mmo-schema`: 66 missed mutants, all in `crates/schema/src/client.rs`. Recent additions to the codebase (such as `LagCompensationPayload` in #71 and `Social*`/`Market*` packets in #67) introduced encoding/decoding logic without rigorous buffer boundary, truncation, or degenerate value tests.
2. `mmo-storage`: 107 missed mutants and 1 timeout.
   - The timeout occurs in `MemorySocialBus::publish` when mutated to `Ok(())`, causing indefinite hangs in `rx.recv().await` in unit tests.
   - The missed mutants stem from missing boundary unit tests on coordinators (`TransactionCoordinator`, `EscrowCoordinator`, `MarketplaceRepository`, `CharacterRepository`), un-tested time helper functions, and database/Redis-specific backend paths that cannot execute in headless SQLite unit testing without external database infrastructure.
3. Disk space bloat: Local invocations of `cargo-mutants` copy unignored worktrees (`.worktrees/`), causing out-of-space failures.

## Scope
1. **Harden `mmo-schema` Unit Test Suite**:
   - Add boundary tests for `Vec3::normalized` (`len == 1e-6`).
   - Add comprehensive unit tests for `LagCompensationPayload` (`encode_to_bytes`, degenerate direction vectors, buffer length checks).
   - Add exact boundary tests for `EntityDeltaState` (`attr_len == 256`, `attr_id == u16::MAX as u32`).
   - Add boundary truncation tests for all newly added packet types (`SocialMessagePacket`, `SocialSubscribePacket`, `SocialUnsubscribePacket`, `MarketListingWire`, `MarketListRequestPacket`, `MarketBuyRequestPacket`, `MarketCancelRequestPacket`, `MarketQueryRequestPacket`, `MarketResponsePacket`).
   - Add exact boundary checks for `WorldObjectSpawnPacket`, `WorldObjectDeltaPacket`, `PlaceObjectPacket`, `AuthRequestPacket`, `AuthResponsePacket`.

2. **Harden `mmo-storage` Unit Test Suite & Fix Timeouts**:
   - Wrap `MemorySocialBus` receiver calls in tests with `tokio::time::timeout(Duration::from_millis(100), ...)` to immediately catch no-op mutations instead of hanging.
   - Add time helper assertions for `current_time_secs()` and `current_time_ms()` in `transaction_coord.rs` and `marketplace.rs`.
   - Add unit tests for `Display for ListingStatus`.
   - Add transfer validation, escrow release/refund, marketplace listing/cancel/reap, and trade boundary tests.

3. **Configure Mutation Testing Infrastructure**:
   - Exclude `.worktrees/` in `.gitignore` to prevent recursive copying during `cargo-mutants` execution.
   - Configure `.cargo/mutants.toml` with `additional_cargo_test_args = ["--lib"]` to bypass integration proptests and doc-tests during mutation passes.
   - Add regex exclusions in `.cargo/mutants.toml` for Postgres/Redis connection and ping methods that require external running services not available in unit test environments.

## Non-Goals
- Spinning up live multi-node Redis clusters or Postgres instances inside the minimal unit test step.
- Relaxing the zero-warning or quality gate standards in CI.
