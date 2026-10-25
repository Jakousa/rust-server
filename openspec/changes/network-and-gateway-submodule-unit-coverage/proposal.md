# Direct Unit Test Coverage for Split Network, Migration, Ghost, Intercept, and Scenario Submodules Proposal

## Context & Problem Statement
When `crates/node/src/network.rs` and `crates/gateway/src/server.rs` were refactored into focused submodules in Bundle #5, existing unit tests remained in `network/mod.rs` and `server/mod.rs` while several newly extracted submodules have 0 inline `#[cfg(test)]` unit tests:
1. `crates/node/src/network/migration.rs` (470 lines): `ingest_migrated_entity` error branches (missing position, out-of-bounds rejection, ghost removal, component deserialization, AoI observer registration) and `execute_entity_migration_pooled` / `execute_batch_entity_migration_pooled` lease-conflict and missing-entity rollback branches.
2. `crates/node/src/network/ghost.rs` (360 lines): `GhostStore` CRUD/radius queries and `GhostBroadcaster` (`distance_to_bounds`, `is_entity_near_boundary`, `find_boundary_entities`, `prepare_broadcast`).
3. `crates/node/src/network/client_pool.rs` (72 lines): `GrpcClientPool` caching, invalidation, and invalid URI handling.
4. `crates/gateway/src/server/intercept.rs` (308 lines): `GatewayInterceptHandler` anti-spoofing drop counter, channel topic routing (`0..=4` and custom), `remove_session` whisper listener abort, and unauthenticated/unavailable marketplace error responses.
5. `crates/loadtest/src/scenario.rs` (387 lines): `ScenarioManifest` JSON deserialization and step validation unit tests.

## Proposed Solution
Add focused, fast inline `#[cfg(test)] mod tests` blocks to each of these submodules testing all error, rollback, anti-spoofing, and boundary branches deterministically without requiring multi-node TCP/QUIC sockets.

## Scope & Non-Goals
- **In Scope**: `crates/node/src/network/{migration.rs, ghost.rs, client_pool.rs}`, `crates/gateway/src/server/intercept.rs`, and `crates/loadtest/src/scenario.rs`.
- **Non-Goals**: Modifying public APIs or production behavior of these modules.
