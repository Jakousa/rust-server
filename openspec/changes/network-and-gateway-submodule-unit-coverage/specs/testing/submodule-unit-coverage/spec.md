# Network, Gateway, and Scenario Submodule Unit Coverage Specification

## Purpose
Provides direct, deterministic unit test coverage for error, rollback, anti-spoofing, and boundary branches in modularized `mmo-node`, `mmo-gateway`, and `mmo-loadtest` submodules.

## Requirements

### Requirement: Migration and Ghost Submodule Unit Tests
`crates/node/src/network/migration.rs`, `crates/node/src/network/ghost.rs`, and `crates/node/src/network/client_pool.rs` SHALL include inline unit tests covering `ingest_migrated_entity` validation errors, single/batch lease rollback paths, `GhostStore` and `GhostBroadcaster` boundary calculations, and `GrpcClientPool` channel caching and invalidation.

#### Scenario: Testing ingest_migrated_entity rejection and lease conflict rollback
- **GIVEN** a `TickSimulation` instance with bounded spatial domain
- **WHEN** `ingest_migrated_entity` is called with `position: None` or out-of-bounds coordinates, or `execute_batch_entity_migration_pooled` encounters a pre-locked GUID mid-batch
- **THEN** `ingest_migrated_entity` SHALL return the expected error code `(1, _)` or `(2, _)`, and `execute_batch_entity_migration_pooled` SHALL release all previously acquired leases in the batch and return `Err(MigrationError::LeaseConflict(_))`

### Requirement: GatewayInterceptHandler Anti-Spoofing and Marketplace Fallback Unit Tests
`crates/gateway/src/server/intercept.rs` SHALL include inline unit tests verifying sender GUID anti-spoofing rejection, topic subscription/unsubscription lifecycle (including whisper listener cleanup on `remove_session`), and `UNAUTHORIZED` / `INTERNAL_ERROR` `MarketResponse` generation when unauthenticated or when `marketplace_service` is `None`.

#### Scenario: Intercepting spoofed SocialMessage and unauthenticated MarketListRequest
- **GIVEN** a `GatewayInterceptHandler` where `session_id = 10` is bound to `Guid(100)` and `session_id = 20` is unauthenticated
- **WHEN** `session_id = 10` sends a `SocialMessage` claiming `sender_guid = 999` and `session_id = 20` sends a `MarketListRequest`
- **THEN** the spoofed `SocialMessage` SHALL be intercepted and dropped without forwarding to subscribers, and `session_id = 20` SHALL receive a `MarketResponsePacket` with `error_code == UNAUTHORIZED`
