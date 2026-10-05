# Technical Design: Codebase Health & Idiomatic Rust Best Practices Refactoring

## 1. Domain Error Architecture

### 1.1 Migration Error Type Hierarchy (`crates/node/src/network.rs`)
Currently, migration functions return `Result<bool, String>` or `Result<BatchMigrationReport, String>`. We introduce `MigrationError`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("Entity {0} not found in entity store")]
    EntityNotFound(Guid),

    #[error("Failed to acquire migration lease for GUID {0} (lease held or migrating)")]
    LeaseConflict(Guid),

    #[error("Connection to recipient node failed: {0}")]
    ConnectionFailed(String),

    #[error("Migration RPC error: {0}")]
    RpcFailed(#[from] tonic::Status),

    #[error("Recipient rejected migration: {0}")]
    RecipientRejected(String),

    #[error("Missing required entity position")]
    MissingPosition,

    #[error("Internal error: {0}")]
    Internal(String),
}
```

This error implements `std::error::Error` and allows callers to differentiate between transient network disconnects (which are retriable) and permanent rejection / entity missing errors (which should abort).

### 1.2 Plugin & Codec Error Types (`crates/core/src/plugin.rs`)
Replace string errors in `ComponentCodec` and `ModularPacketHandler`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("Type downcast failed: expected {expected}, actual {actual:?}")]
    DowncastFailed {
        expected: &'static str,
        actual: TypeId,
    },

    #[error("Serialization failed for component '{type_name}': {source}")]
    Serialization {
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("Deserialization failed for component '{type_name}': {source}")]
    Deserialization {
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PacketHandlerError {
    #[error("Target entity {0} not found")]
    EntityNotFound(Guid),

    #[error("Malformed packet payload for module {module_id}: {message}")]
    MalformedPayload {
        module_id: u16,
        message: String,
    },

    #[error("Handler error: {0}")]
    Custom(String),
}
```

### 1.3 Snapshotter Component Restoration Error Propagation (`crates/node/src/snapshotter.rs`)
Update `restore_entity_components`:
```rust
pub fn restore_entity_components(
    entity: &mut mmo_core::Entity,
    snapshot: &EntitySnapshot,
    codecs: &mmo_core::ComponentCodecRegistry,
) -> Result<usize, CodecError>
```
Propagates typed `CodecError` directly instead of lossy `String` error conversion.

---

## 2. Defensive Ingestion & Panic Elimination

### 2.1 Safe Field Extraction in `migrate_batch_entities`
In `crates/node/src/network.rs`, line 475:
```rust
// Before (Panic risk):
let p = entity_req.position.as_ref().unwrap();

// After (Zero-panic, defensive):
let target_pos = match &entity_req.position {
    Some(p) => (p.x, p.y, p.z),
    None => {
        return Ok(tonic::Response::new(BatchMigrateEntityResponse {
            success: false,
            migrated_count,
            error_message: format!("Entity {} missing 3D position vector", entity_req.guid),
        }));
    }
};
```

### 2.2 System Time Duration Arithmetic
In `crates/node/src/main.rs`:
```rust
// Before:
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_millis() as u64;

// After:
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0);
```

In `crates/node/src/snapshotter.rs` (`capture_entity_snapshot`):
```rust
// Before:
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_secs();

// After:
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0);
```

---

## 3. High-Arity Parameter Structs

### 3.1 Node Smoothing Context (`crates/director/src/coordinator.rs`)
Define:
```rust
/// Context for a node participating in spatial boundary smoothing.
#[derive(Debug, Clone, Copy)]
pub struct NodeSmoothingContext {
    pub node_id: u32,
    pub centroid: (f32, f32, f32),
    pub entity_count: u32,
}
```
Refactor `smooth_adjacent_boundaries`:
```rust
pub fn smooth_adjacent_boundaries(
    &mut self,
    primary: NodeSmoothingContext,
    neighbor: NodeSmoothingContext,
    registry: &mut NodeRegistry,
) -> Option<(BoundingBox3D, BoundingBox3D)>
```
Reduces argument count from 8 down to 4 cleanly, improving maintainability and eliminating the need for `#[allow(clippy::too_many_arguments)]`.

---

## 4. Modern Rust Idioms & Clippy Hardening
- Upgrade string formatting to inlined format arguments: `println!("[Node {node_id}] ...")`.
- Replace `as u32` with `u32::from(node_id)` where conversions are lossless.
- Ensure all quality gates pass:
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo fmt --all -- --check`
