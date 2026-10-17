# Proposal: Edge Authentication, JWT/OIDC, & Secure Session Handoff

## Problem Statement

Currently, the `mmo-gateway` edge transport layer (TCP, QUIC, and WebTransport) does not enforce explicit admission control upon incoming connections. When clients connect, sessions are provisioned monotonically or pre-registered, and received packets are dispatched directly into `SessionRouter`.

Without a zero-trust edge authentication layer:
1. **DDoS & Resource Exhaustion**: Malicious clients can open thousands of transport streams or flood UDP datagrams into the edge gateway, forcing expensive frame decoding and table lookups.
2. **Packet Spoofing & Identity Hijacking**: Untrusted clients can spoof entity GUIDs in movement or interaction packets without cryptographic proof of identity.
3. **Simulation Cluster Exposure**: Unauthenticated traffic can propagate across backhaul pools into simulation nodes, bypassing cluster security boundaries.

## Proposed Solution

Introduce a cryptographic zero-trust admission control layer at the edge gateway:
1. **Edge Authentication Handshake (`crates/schema`)**:
   - Introduce binary protocol packets `AuthRequest` (opcode `0x0011`) and `AuthResponse` (opcode `0x0012`).
   - Clients supply an OIDC/OAuth2 standard JWT token (or cross-gateway handover ticket) upon connection.
2. **Cryptographic Token Verification (`crates/gateway`)**:
   - Provide an extensible `TokenValidator` verifying standard JWT claims (`iss`, `aud`, `exp`, `nbf`, `sub`), supporting HMAC-SHA256 (HS256) and public-key cryptography (EdDSA/RS256).
   - Support seamless reconnects and migrations via `HandoverTicket` validation.
3. **Zero-Cost Packet Filtering at the Edge**:
   - All newly accepted transport connections (TCP, QUIC, WebTransport) start in an `Unauthenticated` state.
   - Any UDP datagram received prior to successful authentication is dropped immediately via an atomic check with near-zero compute cost (no memory allocations, no frame parsing, no routing table lookups).
   - Reliable streams reject any non-auth packet prior to authentication and enforce an authentication timeout window.
4. **Secure Session Provisioning & Handoff**:
   - On successful authentication, the gateway cryptographically verifies the token, provisions an internal session ID, maps the player's `Guid` to the routing table, assigns the authoritative simulation node, and returns `AuthResponse`.
   - Subsequent client traffic is smoothly admitted and routed.

## Scope & Non-Goals

### In Scope
- Framing and serialization of `AuthRequest` and `AuthResponse` in `crates/schema`.
- Extensible `JwtValidator` and `EdgeAuthenticator` in `crates/gateway` supporting configurable keys, claims, and handover tickets.
- Integration into TCP `handle_connection`, QUIC `handle_quic_connection`, and WebTransport `WebTransportServer`.
- Zero-cost datagram filtering and stream gatekeeping prior to authentication.
- Prometheus metrics for authentication attempts, successes, failures, and dropped unauthenticated packets.
- Comprehensive unit and end-to-end integration tests.

### Non-Goals
- Hardcoding specific third-party identity providers (e.g. Google, Discord, Steam); instead, accept standard OIDC/JWT tokens from any provider.
- Implementing an OAuth2 authorization server / identity provider within the game engine. Token issuance is assumed to occur via an external auth service or web portal.
- Changing game-specific entity schemas or player progression data.
