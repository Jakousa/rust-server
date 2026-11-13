# Tasks: WebSocket / TLS Fallback Transport (#189)

- [x] 1. Add `tokio-tungstenite`, `tokio-rustls`, and `futures-util` dependencies to `crates/gateway/Cargo.toml`.
- [x] 2. Implement `crates/gateway/src/websocket.rs` with `WebSocketServer`, optional `TlsAcceptor`, `WsSessionGuard`, `WsConfig` DoS bounds, `EdgeAuthenticator` handshake, `GatewayInterceptHandler`, `PacketDispatcher`, and biased `SessionSink` egress multiplexing.
- [x] 3. Export `websocket` module in `crates/gateway/src/lib.rs`, add `GatewayServer::bind_websocket` and `GatewayServer::bind_websocket_tls` in `crates/gateway/src/server/mod.rs`, and wire `WEBSOCKET_ADDR` / optional TLS in `crates/gateway/src/main.rs`.
- [x] 4. Add integration tests in `crates/gateway/tests/it/websocket.rs` (registered in `crates/gateway/tests/it/main.rs`) covering plain WS E2E lifecycle (movement, delta batch, interaction, ping/pong, coalesced frames, teardown) and WSS TLS E2E + auth enforcement.
- [x] 5. Implement automatic WebTransport-to-WebSocket fallback and `0x4D4D` frame splitting in `web/src/transport.js` and update auto-connect in `web/src/main.js`.
- [x] 6. Add Vitest unit tests in `web/src/__tests__/inspector.test.js` covering WebTransport failure/timeout fallback to WebSocket, explicit `ws://` connection, `sendDatagram`/`sendStream` over WebSocket, `PING`/`PONG` RTT, and multi-frame binary message decoding.
- [x] 7. Run all quality gates: `cargo fmt --all -- --check`, `cargo clippy -p mmo-gateway --all-targets -- -D warnings`, `cargo test -p mmo-gateway`, and `npm --prefix web test -- --run`.
