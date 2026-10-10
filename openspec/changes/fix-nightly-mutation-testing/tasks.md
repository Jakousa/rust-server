# Implementation Tasks: Fix Nightly Mutation Testing Suite

- [x] 1. Update infrastructure and configuration <!-- id: 1 -->
  - [x] 1.1 Add `.worktrees/` to `.gitignore` <!-- id: 1.1 -->
  - [x] 1.2 Update `.cargo/mutants.toml` with `additional_cargo_test_args = ["--lib"]` and exclusions for unviable Postgres/Redis runtime backend methods <!-- id: 1.2 -->
  - [x] 1.3 Verify `.cargo/mutants.toml` parsing with `cargo mutants --list --package mmo-schema` <!-- id: 1.3 -->

- [x] 2. Harden `mmo-schema` codec test suite <!-- id: 2 -->
  - [x] 2.1 Add exact threshold boundary test for `Vec3::normalized` at `len == 1e-6` <!-- id: 2.1 -->
  - [x] 2.2 Add unit tests for `LagCompensationPayload` (`encode_to_bytes`, degenerate direction at `1e-6`, truncation) <!-- id: 2.2 -->
  - [x] 2.3 Add exact boundary tests for `EntityDeltaState::decode` (`attr_len == 256`, `attr_id == u16::MAX as u32`) <!-- id: 2.3 -->
  - [x] 2.4 Add exact boundary and truncation tests for `WorldObjectSpawnPacket`, `WorldObjectDeltaPacket`, `PlaceObjectPacket`, `AuthRequestPacket`, `AuthResponsePacket` <!-- id: 2.4 -->
  - [x] 2.5 Add truncation and boundary tests for `SocialMessagePacket`, `SocialSubscribePacket`, `SocialUnsubscribePacket` <!-- id: 2.5 -->
  - [x] 2.6 Add truncation and boundary tests for `MarketListingWire`, `MarketListRequestPacket`, `MarketBuyRequestPacket`, `MarketCancelRequestPacket`, `MarketQueryRequestPacket`, `MarketResponsePacket` <!-- id: 2.6 -->
  - [x] 2.7 Verify `cargo test -p mmo-schema` passes cleanly <!-- id: 2.7 -->

- [x] 3. Harden `mmo-storage` test suite & resolve timeouts <!-- id: 3 -->
  - [x] 3.1 Wrap `MemorySocialBus` receiver calls in `social.rs` with `tokio::time::timeout(Duration::from_secs(1))` <!-- id: 3.1 -->
  - [x] 3.2 Add time helper tests for `current_time_secs()` and `current_time_ms()` in `transaction_coord.rs` and `marketplace.rs` <!-- id: 3.2 -->
  - [x] 3.3 Add unit test covering all variants of `ListingStatus` `Display` implementation in `models.rs` <!-- id: 3.3 -->
  - [x] 3.4 Add boundary validation tests for `TransactionCoordinator`, `EscrowCoordinator`, and `MarketplaceRepository` <!-- id: 3.4 -->
  - [x] 3.5 Verify `cargo test -p mmo-storage` passes cleanly <!-- id: 3.5 -->

- [x] 4. Run workspace quality gates and verify mutation testing <!-- id: 4 -->
  - [x] 4.1 Run `cargo fmt --all -- --check` <!-- id: 4.1 -->
  - [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings` <!-- id: 4.2 -->
  - [x] 4.3 Run `cargo test --workspace` <!-- id: 4.3 -->

- [ ] 5. PR creation and auto-merge <!-- id: 5 -->
  - [x] 5.1 Commit and push to `fix/nightly-mutation-testing` <!-- id: 5.1 -->
  - [x] 5.2 Open PR and enable auto-merge <!-- id: 5.2 -->
  - [ ] 5.3 Monitor CI check suite until merged <!-- id: 5.3 -->
