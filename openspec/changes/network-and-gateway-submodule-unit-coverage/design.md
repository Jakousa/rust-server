# Technical Design: Direct Unit Test Coverage for Network, Gateway, and Scenario Submodules

## 1. `crates/node/src/network/migration.rs` Unit Tests

Add `#[cfg(test)] mod tests` at the bottom of `crates/node/src/network/migration.rs`:
1. `test_ingest_migrated_entity_missing_position_and_out_of_bounds`:
   - Verify `ingest_migrated_entity` returns `Err((1, _))` when `req.position.is_none()`.
   - Verify `ingest_migrated_entity` returns `Err((2, _))` when `req.position` lies outside `sim.octree.bounds`.
2. `test_ingest_migrated_entity_removes_ghost_and_restores_state`:
   - Pre-insert `guid` into `sim.ghost_store`, call `ingest_migrated_entity` with `session_id != 0` and `last_migration_tick = 15`, and assert the ghost is removed, the entity is in `entity_store` and `octree`, `kinematic_states` has `last_sequence`, and `aoi` has the registered observer.
3. `test_batch_migration_all_or_nothing_lease_rollback`:
   - Spawn `g1` and `g2`, pre-acquire migration lease on `g2`, call `execute_batch_entity_migration_pooled` for `&[g1, g2]`, and verify it returns `Err(MigrationError::LeaseConflict(g2))` AND releases the lease acquired on `g1`.
4. `test_batch_migration_empty_slice_short_circuits`:
   - Call `execute_batch_entity_migration_pooled` with `&[]` and verify `Ok(BatchMigrationReport { success: true, total_requested: 0, migrated_count: 0, .. })`.

---

## 2. `crates/node/src/network/ghost.rs` & `client_pool.rs` Unit Tests

1. In `crates/node/src/network/ghost.rs` `#[cfg(test)] mod tests`:
   - Test `GhostStore` (`insert_or_update`, `upsert_from_proto`, `update_from_request`, `query_radius`, `remove`, `clear`).
   - Test `GhostBroadcaster` (`distance_to_bounds`, `is_entity_near_boundary` with `boundary_margin <= 0.0` and `> 0.0`, `find_boundary_entities`, and `prepare_broadcast`).
2. In `crates/node/src/network/client_pool.rs` `#[cfg(test)] mod tests`:
   - Test `GrpcClientPool::get_channel` caching (second lookup hits `DashMap`), `invalidate`, and invalid endpoint URI error return.

---

## 3. `crates/gateway/src/server/intercept.rs` Unit Tests

Add `#[cfg(test)] mod tests` at the bottom of `crates/gateway/src/server/intercept.rs`:
1. `test_social_message_anti_spoofing_drops_mismatched_sender`:
   - Bind `session_id = 1` to `Guid::from_raw(100)` in `SessionRouter`. Subscribe `session_id = 2` to `"chat:global"`. Send `ClientPacket::SocialMessage` from `session_id = 1` with `sender_guid = 200` and verify `session_id = 2`'s sink receives nothing.
2. `test_social_subscribe_unsubscribe_and_remove_session`:
   - Verify channel types `0..=4` and custom channel names map to expected topic strings and `remove_session` cleans up whisper subscriptions.
3. `test_market_requests_unauthenticated_and_unavailable_errors`:
   - Verify `MarketListRequest`, `MarketBuyRequest`, `MarketCancelRequest`, and `MarketQueryRequest` emit `MarketResponsePacket` with `UNAUTHORIZED` or `INTERNAL_ERROR` onto the session sink, and `ClientPacket::MarketResponse` is intercepted (`true`) and dropped.

---

## 4. `crates/loadtest/src/scenario.rs` Unit Tests

Add `#[cfg(test)] mod tests` at the bottom of `crates/loadtest/src/scenario.rs`:
- Test `ScenarioManifest` JSON round-trip serialization/deserialization for all `ScenarioStep` variants (`Connect`, `Authenticate`, `Move`, `Ping`, `WaitMs`, `AssertKnownEntity`, `Chat`, `Disconnect`).
