# Specification: Nightly Mutation Testing Suite Hardening

## Requirements

### Requirement 1: Schema Codec Exact Boundary Hardening
The schema decoder tests MUST assert exact boundary conditions on all packet types and payloads.

#### Scenario 1.1: Degenerate Vec3 Normalization
- GIVEN a `Vec3` with length exactly equal to `1e-6`
- WHEN `normalized()` is invoked
- THEN it MUST return `None` (requiring `len > 1e-6`).

#### Scenario 1.2: Lag Compensation Payload Boundary Verification
- GIVEN a `LagCompensationPayload` with direction length equal to `1e-6`
- WHEN `decode()` is invoked
- THEN it MUST return an `InvalidData` error indicating degenerate direction vector.
- AND `encode_to_bytes()` MUST produce a valid non-empty byte buffer matching `encode()`.

#### Scenario 1.3: Social and Market Packet Buffer Boundaries
- GIVEN any encoded `Social*` or `Market*` packet
- WHEN decoded with truncated byte buffers (missing length prefixes or payload bodies)
- THEN `decode()` MUST return `ClientCodecError::BufferTooShort` with exact expected sizes.

---

### Requirement 2: Storage Unit Test Liveness and Determinism
The storage test suite MUST terminate promptly without hanging and assert coordinator boundary invariants.

#### Scenario 2.1: Social Bus Publish Interception
- GIVEN a `MemorySocialBus` subscription
- WHEN publishing an envelope is a no-op (mutated)
- THEN unit tests MUST fail within a bounded timeout (e.g. 1s) rather than waiting indefinitely.

#### Scenario 2.2: Time Helper Lower Bound Invariants
- GIVEN time helper functions `current_time_secs()` and `current_time_ms()` in storage coordinators
- WHEN evaluated in unit tests
- THEN they MUST be asserted to be positive timestamps strictly exceeding epoch year 2023 (`> 1_700_000_000`).

---

### Requirement 3: Mutation Config Declarative Exclusions
The mutation testing configuration MUST exclude environment-dependent mutations that require unhosted external servers (Postgres/Redis) in headless unit test runners.

#### Scenario 3.1: Service Backend Exclusions
- GIVEN mutation targets in `RedisCache::remove_session`, `RedisSocialBus::publish`, or Postgres-specific pool/migration methods in `Database`
- WHEN `cargo-mutants` is executed
- THEN these mutations MUST be excluded via `.cargo/mutants.toml` `exclude_re`.
