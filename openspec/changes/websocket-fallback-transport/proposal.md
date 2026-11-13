# Proposal: WebSocket / TLS over TCP Fallback Transport (#189)

## Context & Motivation
The Edge Gateway (`crates/gateway`) currently serves browser clients via HTTP/3 WebTransport over UDP (default port `4433`). In restricted network environments (corporate HTTP proxies, strict firewalls, or sandbox containers), outbound UDP to non-standard ports is often blocked or returns ICMP Port Unreachable, causing WebTransport handshakes to fail or hang.

## Scope
1. **Gateway WebSocket / TLS Transport (`crates/gateway`)**:
   - Add `crates/gateway/src/websocket.rs` implementing `WebSocketServer` over TCP with optional `tokio-rustls` TLS (`ws://` and `wss://`).
   - Multiplex both ephemeral (`datagram_tx`) and reliable (`stream_tx`) `SessionSink` channels onto a single WebSocket binary stream with priority for reliable packets (`tokio::select! { biased; ... }`).
   - Use the exact same `0x4D4D` magic header + `ClientPacket::decode_frame` / `encode` framing, `PacketDispatcher`, `EdgeAuthenticator` handshake, `GatewayInterceptHandler`, and `SessionRouter` cleanup as `WebTransportServer` and raw TCP `handle_connection`.
   - Expose `GatewayServer::bind_websocket` and `GatewayServer::bind_websocket_tls` and wire `WEBSOCKET_ADDR` (default `0.0.0.0:8443`) and optional TLS env configuration in `crates/gateway/src/main.rs`.
2. **Web Frontend Automatic Fallback (`web/src/transport.js`, `web/src/main.js`)**:
   - Extend `WebTransportClient` to attempt WebTransport first (with a bounded handshake timeout) and automatically fall back to WebSocket (`ws://` or `wss://`) when WebTransport is unsupported, times out, or fails to connect.
   - Support direct `ws://` / `wss://` URLs without attempting WebTransport first.
   - Ensure binary framing parity (`0x4D4D` 8-byte header, LEB128 varint payloads, multi-frame coalescing support) across both WebTransport and WebSocket modes.
   - Report active transport mode (`"webtransport"` vs `"websocket"`) in status notifications.

## Non-Goals
- Replacing WebTransport as the primary low-latency browser transport when UDP/HTTP3 is available.
- Implementing a custom HTTP/1.1 reverse proxy or multiplexing WebSocket on the same port as raw TCP (`LISTEN_ADDR` `8080`).
- Modifying core genre-agnostic engine crates (`crates/core`, `crates/node`, `crates/director`, `crates/storage`).
