## 1. QUIC Dependencies and TLS Infrastructure

- [x] 1.1 Add `quinn`, `rcgen`, and `rustls` dependencies to `crates/gateway/Cargo.toml` and verify compilation
- [x] 1.2 Implement self-signed certificate generation (`generate_self_signed_cert`) and QUIC server/client endpoint builders with datagram support in `crates/gateway/src/quic.rs`, verifying with unit tests

## 2. QUIC Gateway Server and Client Harness

- [x] 2.1 Implement `QuicClientSession` helper supporting both unreliable datagram and reliable stream packet delivery with test cert verifier in `crates/gateway/src/quic.rs`, verifying with unit tests
- [x] 2.2 Implement `GatewayServer::bind_quic` and `GatewayServer::handle_quic_connection` handling concurrent datagrams and streams, integrating with `PacketDispatcher` and downstream `session_sinks`, verifying with unit tests

## 3. End-to-End Testing & Verification

- [x] 3.1 Create end-to-end integration test `crates/gateway/tests/quic_gateway_e2e.rs` verifying QUIC handshake, datagram movement, stream interactions, and downstream AoI broadcasts
- [x] 3.2 Run full workspace test suite (`cargo test`) to ensure zero regressions across all crates
