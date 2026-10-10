# Design Document: Nightly Mutation Testing Suite Hardening

## Architecture Overview
Mutation testing verifies that the test suite actually asserts the correctness of the code by systematically introducing synthetic faults (mutants). If tests pass despite a mutant being introduced, the mutant survives ("missed"), indicating a test gap.

### Root Cause Analysis of Nightly Failures
1. **Schema Packet Codecs (`crates/schema/src/client.rs`)**:
   - Recent features added new packet structures (`LagCompensationPayload`, `SocialMessagePacket`, `SocialSubscribePacket`, `SocialUnsubscribePacket`, `MarketListingWire`, `MarketListRequestPacket`, `MarketBuyRequestPacket`, `MarketCancelRequestPacket`, `MarketQueryRequestPacket`, `MarketResponsePacket`).
   - Happy-path round-trip tests verify serialization, but omit negative boundary assertions:
     - Buffer truncated before reading length fields.
     - Buffer truncated before reading payload fields.
     - Payload length set to 0 or exact minimum size.
     - Degenerate floating-point thresholds (`1e-6` precision).
   - In `EntityDeltaState::decode`, boundary values `attr_len == 256` and `attr_id == u16::MAX as u32` were not tested.

2. **Storage Test Invariants & Timeouts (`crates/storage`)**:
   - `MemorySocialBus`: Tests used `rx.recv().await.unwrap()`. When `publish` was replaced with `Ok(())`, the receiver hung until runner timeout. Wrapping with `tokio::time::timeout(Duration::from_secs(1), ...)` converts timeouts into fast test failures, killing the mutant safely without flakiness on loaded runners.
   - Time helpers: Functions returning `current_time_secs()` and `current_time_ms()` had no assertions in `transaction_coord.rs` and `marketplace.rs`. Adding `assert!(secs > 1_700_000_000)` and `assert!(ms > 1_700_000_000_000)` kills mutations replacing them with `0`, `1`, or `-1`.
   - Coordinators: Validation checks (e.g. self-transfers, empty item lists, price/duration 0) need explicit negative assertions.
   - External services: Unit tests execute against SQLite and in-memory backends without live Postgres or Redis daemons. Code branches dedicated to `RedisCache`, `RedisSocialBus`, and Postgres connection management cannot execute without live external services, so they must be declared in `.cargo/mutants.toml` exclusions.

3. **Performance & Infrastructure Optimization**:
   - `additional_cargo_test_args = ["--lib"]` in `.cargo/mutants.toml` speeds up mutation evaluation by skipping documentation tests and multi-iteration integration proptests (`tests/packet_proptest.rs`).
   - Adding `.worktrees/` to `.gitignore` prevents cargo-mutants from creating multi-gigabyte temporary workspace copies.

## Security & Anti-Cheat Considerations
- Ensuring packet decoding enforces strict buffer boundary checks prevents buffer over-read panics and denial-of-service vulnerabilities in network gateway actors.
- Enforcing positive bounds checks in lag compensation prevents zero-division and invalid floating-point raycasts.
