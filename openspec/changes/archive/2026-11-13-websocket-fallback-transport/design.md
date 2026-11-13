# Technical Design: WebSocket / TLS Fallback Transport

## 1. Gateway Architecture (`crates/gateway/src/websocket.rs`)

### Dependencies
- `tokio-tungstenite = "0.26"` (async WebSocket over Tokio streams)
- `tokio-rustls = "0.26"` (optional TLS termination reusing workspace `rustls = "0.23"`)
- `futures-util = "0.3"` (`StreamExt`, `SinkExt` for split `WebSocketStream`)

### Server Structure
`WebSocketServer` mirrors `WebTransportServer`:
- Fields:
  - `listener: TcpListener`
  - `tls_acceptor: Option<tokio_rustls::TlsAcceptor>`
  - `router: Arc<SessionRouter>`
  - `dispatcher: PacketDispatcher`
  - `session_counter: Arc<AtomicU64>`
  - `session_sinks: Arc<DashMap<u64, SessionSink>>`
  - `packet_tx: Option<mpsc::Sender<RoutedPacket>>`
  - `authenticator: Option<Arc<EdgeAuthenticator>>`
  - `default_node_address: Option<String>`
  - `intercept_handler: Option<GatewayInterceptHandler>`
- Builders:
  - `WebSocketServer::bind(...)`
  - `with_tls_config(Arc<rustls::ServerConfig>)`
  - `with_authenticator(...)`, `with_default_node_address(...)`, `with_intercept_handler(...)`
  - `run(self, shutdown_rx)` and `run_background(self)`
- `GatewayServer` integration (`crates/gateway/src/server/mod.rs`):
  - `GatewayServer::bind_websocket(&self, addr: SocketAddr)`
  - `GatewayServer::bind_websocket_tls(&self, addr: SocketAddr, tls_config: Arc<rustls::ServerConfig>)`
- `crates/gateway/src/main.rs`:
  - Read `WEBSOCKET_ADDR` (default `0.0.0.0:8443`).
  - Optional TLS via `WEBSOCKET_TLS=true` (generates self-signed cert via `crate::quic::generate_self_signed_cert` or loads `WEBSOCKET_TLS_CERT` / `WEBSOCKET_TLS_KEY` if provided; default plain `ws://` when `WEBSOCKET_TLS` is unset so local Vite dev and Docker Compose work without browser self-signed WSS rejection).
  - Shutdown `ws_shutdown_tx` alongside `wt_shutdown_tx`.

### Concurrency & Session Lifecycle
1. Accept TCP connection -> spawn per-connection task so slow TLS/WS handshakes do not block the accept loop.
2. If `tls_acceptor` is `Some`, perform `tls_acceptor.accept(tcp_stream).await` wrapped in a handshake timeout (`10s` default) to prevent Slowloris.
3. Configure `WebSocketConfig` with `max_message_size: Some(MAX_WS_BUFFER_SIZE)` and `max_frame_size: Some(MAX_WS_BUFFER_SIZE)` where `MAX_WS_BUFFER_SIZE = 256 * 1024` (256 KiB) to allow coalesced multi-frame binary messages, while `ClientPacket::decode_frame` strictly enforces `MAX_PACKET_PAYLOAD_SIZE` (64 KiB) per individual packet.
4. Upgrade via `tokio_tungstenite::accept_async_with_config(stream, Some(ws_config)).await` (also bounded by handshake timeout).
5. Allocate `session_id = session_counter.fetch_add(1, Ordering::Relaxed)`.
6. Register `SessionSink::new(datagram_tx, stream_tx)` in `session_sinks`, guarded by `WsSessionGuard` (`Drop` removes from `router`, `session_sinks`, `intercept_handler`, aborts `writer_handle`, and updates `mmo_gateway_active_sessions`).
7. Split WebSocket into `(ws_write, ws_read)` plus a high-priority `ctrl_rx` (for RFC 6455 `Message::Pong` replies).
8. Spawn egress writer task:
   ```rust
   loop {
       tokio::select! {
           biased;
           Some(pong) = ctrl_rx.recv() => {
               if ws_write.send(Message::Pong(pong)).await.is_err() { break; }
           }
           Some(bytes) = stream_rx.recv() => {
               if ws_write.send(Message::Binary(bytes)).await.is_err() { break; }
           }
           Some(bytes) = datagram_rx.recv() => {
               if ws_write.send(Message::Binary(bytes)).await.is_err() { break; }
           }
           else => break,
       }
   }
   ```
   Store `writer_handle` in `WsSessionGuard` so it is aborted on session drop.
9. Ingress & Auth:
   - If `authenticator` is `Some(auth)`:
     - Loop reading `ws_read.next()` under `tokio::time::timeout(auth.auth_timeout(), ...)` until `ClientPacket::decode_frame(&mut buf)` yields a packet.
     - Validate `ClientPacket::AuthRequest`, call `authenticate_and_register_session(...)`.
     - Send `AuthResponse` via `stream_tx` (on error, sleep 15ms and return).
   - If `authenticator` is `None`:
     - Auto-register default player route if `default_node_address` is `Some`.
     - Handle `AuthRequest` in loop identically to `WebTransportServer`.
   - Main ingress loop:
     - `Message::Binary(data)` -> check `buf.len() + data.len() <= MAX_WS_BUFFER_SIZE`, append to `buf`, loop `ClientPacket::decode_frame(&mut buf)`.
     - `ClientPacket::Ping(ping)` -> reply `ClientPacket::Pong(...)` via `datagram_tx` (or `stream_tx`).
     - `intercept_handler.handle_packet(session_id, &pkt)` -> `dispatcher.dispatch_packet(session_id, pkt)` -> `packet_tx.send(routed)`.
     - `Message::Ping(payload)` -> enqueue `payload` onto `ctrl_tx` to echo `Message::Pong`.
     - `Message::Pong(_)` -> no-op.
     - `Message::Close(_)` -> break cleanly.
     - `Message::Text(_)` -> increment `mmo_gateway_framing_errors_total` and break.

## 2. Web Frontend Architecture (`web/src/transport.js` & `web/src/main.js`)

### `deriveWebSocketUrl(url, options)`
- If `url` starts with `ws://` or `wss://`, return it as-is.
- Parse `url` with `new URL(url)`:
  - `wsPort = options.wsPort ?? 8443`
  - `protocol = (typeof window !== "undefined" && window.location?.protocol === "https:") ? "wss:" : "ws:"`
  - Return `${protocol}//${parsed.hostname}:${wsPort}`.

### `WebTransportClient` Fallback Flow
- Track `this.transportMode = null` (`"webtransport"` | `"websocket"`), `this.ws = null`.
- `connect(url, certHashHex = "", options = {})`:
  - If `url.startsWith("ws://") || url.startsWith("wss://")`, call `_connectWebSocket(url, options)`.
  - Otherwise try `_connectWebTransport(url, certHashHex, options.wtTimeoutMs ?? 4000)`.
    - Attach `this.transport.closed.catch(() => {})` before awaiting `ready` so late QUIC timeout rejections do not cause unhandled promise rejections, and race `this.transport.ready` against a timeout timer (`clearTimeout` on settle; on timeout call `this.transport.close()`).
    - If `WebTransport` is undefined or `_connectWebTransport` throws/times out:
      - Clean up any partial `this.transport`.
      - Derive `wsUrl = options.wsUrl || deriveWebSocketUrl(url, options)`.
      - Call `_connectWebSocket(wsUrl, options)`.
- `_connectWebSocket(wsUrl, options)`:
  - Create `new WebSocket(wsUrl)`, set `ws.binaryType = "arraybuffer"`.
  - Await `open` (with `options.wsTimeoutMs ?? 5000` timeout).
  - On `message`: convert `ArrayBuffer` to `Uint8Array`, `trackIngress(bytes.byteLength)`, and decode all complete `0x4D4D` frames in the message (`[0x4D, 0x4D, opcode: u16 LE, payload_len: u32 LE]`) passing each frame to `handleIncomingPacket(frame)`.
  - On `close` / `error`: clear `pingInterval`, set `connected = false`, notify `"disconnected"`.
  - Set `this.transportMode = "websocket"`, `this.connected = true`, notify `"connected"`, start `startPingLoop()`.
- `sendDatagram(data)` & `sendStream(data)`:
  - If `this.transportMode === "websocket"`, send `data` via `this.ws.send(data)` when `readyState === 1` (`OPEN`).
  - Otherwise use existing WebTransport `datagramWriter` / `streamWriter`.
- `disconnect()`:
  - Close `this.ws` if present, close `this.transport` if present, reset `this.transportMode = null`.
- `web/src/main.js`:
  - Auto-connect on `DOMContentLoaded` unconditionally (since WebSocket fallback works even when `typeof WebTransport === "undefined"`).
