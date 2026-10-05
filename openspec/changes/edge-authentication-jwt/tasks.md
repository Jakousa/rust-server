# Implementation Tasks: Edge Authentication, JWT/OIDC, & Session Handoff

- [x] 1. Protocol Schemas & Framing (`crates/schema`)
  - [x] 1.1 Add `Opcode::AuthRequest` (0x0011) and `Opcode::AuthResponse` (0x0012) to `crates/schema/src/client.rs`.
  - [x] 1.2 Define `AuthRequestPacket` and `AuthResponsePacket` structs and error code constants (`auth_error_codes`).
  - [x] 1.3 Implement binary `encode` and `decode` for `AuthRequestPacket` and `AuthResponsePacket`.
  - [x] 1.4 Wire `AuthRequest` and `AuthResponse` into `ClientPacket` enum and match arms (`opcode`, `encode`, `decode_bytes`).
  - [x] 1.5 Add proptest and roundtrip unit tests in `crates/schema/src/client.rs` and `crates/schema/tests/packet_proptest.rs`.
  - [x] 1.6 Verify `cargo test -p mmo-schema`.

- [x] 2. Cryptographic JWT Validator & Edge Authenticator (`crates/gateway`)
  - [x] 2.1 Add `jsonwebtoken` and `serde` dependencies to `crates/gateway/Cargo.toml`.
  - [x] 2.2 Create `crates/gateway/src/auth.rs` with `JwtClaims`, `TokenValidator`, and `EdgeAuthenticator`.
  - [x] 2.3 Implement signature and claims verification (HS256, EdDSA/RS256, expiration, issuer, audience), explicitly forbidding the `none` algorithm.
  - [x] 2.4 Implement `HandoverTicket` verification fallback for migrating cross-gateway sessions.
  - [x] 2.5 Add unit tests for valid tokens, expired tokens, tampered signatures, algorithm `none` rejection, and wrong issuer/audience.
  - [x] 2.6 Verify `cargo test -p mmo-gateway --lib auth`.

- [x] 3. Anti-Spoofing & Initial Node Resolution in Gateway (`crates/gateway`)
  - [x] 3.1 Update `PacketDispatcher::dispatch_with_router` to enforce identity binding: reject packets where `acting_guid != router.get_guid_by_session(session_id)`.
  - [x] 3.2 Add `GatewayError::IdentityMismatch` error variant and `mmo_gateway_spoofed_packets_dropped_total` metric.
  - [x] 3.3 Add `default_node_address` and initial node resolution flow to `GatewayServer`.
  - [x] 3.4 Verify unit tests for anti-spoofing in `server.rs`.

- [x] 4. Gateway Server Zero-Trust Admission & Filtering (`crates/gateway`)
  - [x] 4.1 Integrate `EdgeAuthenticator` into `GatewayServer` with `with_authenticator` builder method.
  - [x] 4.2 Implement atomic admission control (`Ordering::Acquire`/`Release`) and zero-cost datagram filtering in QUIC `handle_quic_connection`.
  - [x] 4.3 Implement authentication handshake and timeout on reliable QUIC streams.
  - [x] 4.4 Implement atomic admission control and datagram filtering in `WebTransportServer`.
  - [x] 4.5 Implement stream authentication in TCP `handle_connection`.
  - [x] 4.6 Instrument authentication and dropped packet Prometheus metrics.
  - [x] 4.7 Verify `cargo check -p mmo-gateway`.

- [x] 5. End-to-End Integration Tests & Verification (`crates/gateway/tests/it`)
  - [x] 5.1 Write integration tests in `crates/gateway/tests/it/auth.rs`.
  - [x] 5.2 Test unauthenticated datagram dropping with zero forwarding.
  - [x] 5.3 Test full handshake lifecycle: connect -> `AuthRequest` -> `AuthResponse` -> session routed -> movement accepted.
  - [x] 5.4 Test rejection of expired, tampered, and algorithm-none tokens with connection termination.
  - [x] 5.5 Test anti-spoofing rejection when a client tries to move an entity GUID not bound to its session.
  - [x] 5.6 Test authentication timeout for idle unauthenticated clients.
  - [x] 5.7 Verify `cargo test --workspace`.

- [x] 6. Quality Gates & OpenSpec Sync
  - [x] 6.1 Run `cargo fmt --all -- --check`.
  - [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 6.3 Sync delta specs to `openspec/specs/gateway/edge-authentication/spec.md`.
