# Specification: WebSocket / TLS Fallback Transport

## Purpose

Defines the Edge Gateway WebSocket (`ws://` / `wss://`) fallback transport, binary `0x4D4D` framing and DoS bounds parity, and automatic browser WebTransport-to-WebSocket fallback for restricted network environments.

## Requirements

### Requirement: Gateway WebSocket Transport Termination
The Edge Gateway SHALL accept WebSocket connections over TCP (`ws://`) and optionally TLS over TCP (`wss://`) on a configurable bind address (`WEBSOCKET_ADDR`).

#### Scenario: Plain WebSocket Client Connection & Packet Routing
- **GIVEN** a running `GatewayServer` with `WebSocketServer` bound on a TCP port
- **WHEN** a WebSocket client connects and sends a binary `ClientPacket::Movement` frame (`0x4D4D` magic header)
- **THEN** the gateway decodes the frame using `ClientPacket::decode_frame`, validates identity via `PacketDispatcher`, and forwards the `RoutedPacket` to the assigned simulation node.

#### Scenario: TLS WebSocket (WSS) Connection
- **GIVEN** a `WebSocketServer` configured with a `rustls::ServerConfig` TLS acceptor
- **WHEN** a TLS WebSocket client completes the TLS + WebSocket upgrade handshake
- **THEN** binary frames are exchanged over the encrypted stream identically to plain WebSocket.

#### Scenario: Biased Egress Multiplexing (`SessionSink`)
- **GIVEN** an active WebSocket session registered in `session_sinks`
- **WHEN** both `stream_tx` (reliable) and `datagram_tx` (ephemeral) have queued `ClientPacket` frames
- **THEN** the WebSocket writer prioritizes `stream_tx` over `datagram_tx` and transmits each frame as a `Message::Binary` payload.

#### Scenario: Authentication & Handshake Enforcement
- **GIVEN** a `GatewayServer` configured with an `EdgeAuthenticator`
- **WHEN** a WebSocket client connects and fails to send a valid `ClientPacket::AuthRequest` within `auth_timeout()`
- **THEN** the gateway increments `mmo_gateway_auth_failures_total` and closes the WebSocket connection without registering a session route.

#### Scenario: Clean Teardown & Guarded Resource Cleanup
- **GIVEN** an active WebSocket session
- **WHEN** the client disconnects or sends `Message::Close`
- **THEN** `WsSessionGuard` removes the session from `SessionRouter`, `session_sinks`, and `GatewayInterceptHandler`, and updates `mmo_gateway_active_sessions`.

### Requirement: Binary Framing & DoS Bounds Parity
WebSocket transport SHALL enforce the exact `0x4D4D` binary framing and payload size bounds used by TCP and WebTransport.

#### Scenario: Coalesced Multi-Frame Binary Message
- **GIVEN** a WebSocket `Message::Binary` containing two concatenated `0x4D4D`-framed packets (including LEB128 varint payloads)
- **WHEN** received by the gateway or browser client
- **THEN** both packets are decoded and processed in order.

#### Scenario: Oversized or Text Message Rejection
- **GIVEN** an active WebSocket session
- **WHEN** a client sends an oversized WebSocket message (> `MAX_PACKET_PAYLOAD_SIZE + HEADER_SIZE`) or a `Message::Text` frame
- **THEN** the gateway rejects the invalid frame, increments `mmo_gateway_framing_errors_total` (or closes on protocol/size violation), and does not panic or OOM.

### Requirement: Browser Automatic WebTransport-to-WebSocket Fallback
The web frontend (`web/src/transport.js`) SHALL automatically fall back from WebTransport to WebSocket when WebTransport is unavailable or fails handshake.

#### Scenario: WebTransport Unavailable or Blocked
- **GIVEN** a browser environment where `WebTransport` is undefined or `new WebTransport(url).ready` rejects or times out
- **WHEN** `WebTransportClient.connect(url, certHash)` is called
- **THEN** the client automatically derives the fallback WebSocket URL, establishes a `WebSocket` connection with `binaryType = "arraybuffer"`, sets `transportMode = "websocket"`, and notifies status `"connected"` with `mode: "websocket"`.

#### Scenario: Explicit WebSocket URL
- **GIVEN** a `ws://` or `wss://` URL passed to `WebTransportClient.connect(url)`
- **WHEN** `connect` is invoked
- **THEN** the client connects directly via `WebSocket` without attempting `WebTransport`.
