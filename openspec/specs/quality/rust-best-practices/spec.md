# Codebase Health & Idiomatic Rust Best Practices Specification

## Purpose

Enforces codebase health, idiomatic Rust best practices, strongly typed domain error propagation, zero-panic defensive ingestion, and ergonomic parameter structures across the distributed server platform.

## Requirements

### Requirement: Migration Error Enumeration
All node-to-node entity migration functions SHALL return strongly-typed error enums instead of raw strings.

#### Scenario: Migration error matching
- **GIVEN** an entity migration process (`execute_entity_migration` or `execute_batch_entity_migration`)
- **WHEN** any error occurs (entity not found in store, lease conflict, network transport error, or recipient rejection)
- **THEN** the function SHALL return `Result<_, MigrationError>` where `MigrationError` is an exhaustive enum derived with `thiserror::Error` allowing pattern matching on the exact failure cause

### Requirement: Codec & Packet Handler Error Enumeration
Component serialization and modular packet routing SHALL return typed domain errors.

#### Scenario: Codec and handler error handling
- **GIVEN** a component serialization/deserialization or modular packet dispatch operation
- **WHEN** a payload is invalid, downcast fails, or handler encounters an unknown entity
- **THEN** the system SHALL return `Result<_, CodecError>` or `Result<_, PacketHandlerError>` with structured context rather than allocating arbitrary error strings

### Requirement: Snapshotter Native Error Propagation
The write-behind snapshotter SHALL propagate native database storage errors.

#### Scenario: Native error propagation
- **GIVEN** a dirty entity snapshot flush to persistence
- **WHEN** the underlying database repository fails
- **THEN** `flush_dirty_entities` SHALL propagate `Result<usize, StorageError>` directly preserving SQL error codes and diagnostics

### Requirement: Safe Ingestion Pass in Batch Migration
The gRPC receiver for batch entity migration SHALL NOT invoke `.unwrap()` on optional fields.

#### Scenario: Safe gRPC ingestion
- **GIVEN** a `BatchMigrateEntityRequest` received over gRPC
- **WHEN** any entity request lacks required fields (such as `position`)
- **THEN** the receiver SHALL gracefully fail the batch validation pass, return `BatchMigrateEntityResponse { success: false, ... }`, and NOT panic

### Requirement: Safe Clock and Timestamp Arithmetic
Daemon loops in simulation nodes and director SHALL NOT unwrap duration calculations.

#### Scenario: Clock skew handling
- **GIVEN** a periodic heartbeat or telemetry dispatch loop
- **WHEN** calculating elapsed milliseconds from `SystemTime::now()`
- **THEN** clock skew or leap seconds SHALL be handled gracefully without process termination

### Requirement: Parameter Struct for Boundary Smoothing
Boundary smoothing calculations between adjacent nodes SHALL take structured context objects.

#### Scenario: High-arity refactoring
- **GIVEN** two adjacent nodes participating in boundary plane adjustment
- **WHEN** invoking `GlobalOctreeCoordinator::smooth_adjacent_boundaries`
- **THEN** the function SHALL accept `NodeSmoothingContext` structs bundling node ID, clamped centroid, and entity count, avoiding high arity function signatures
