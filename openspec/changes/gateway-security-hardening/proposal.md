# Gateway Maximum Frame Size, Bounded Streams & Slowloris Mitigation Proposal

## Context & Problem Statement
Security auditing of `crates/schema/src/client.rs` and `crates/gateway/src/server.rs` identified several Denial-of-Service (DoS) and memory exhaustion vectors in the Edge Gateway:

1. **Unbounded Frame Length**:
   In `ClientPacket::decode_frame` (`crates/schema/src/client.rs:2283`), `payload_len` is extracted directly as a 32-bit unsigned integer without upper-bound validation. An attacker can send a valid 2-byte magic header with a `payload_len` up to 4 GiB, causing the gateway receiver to accumulate bytes in a `BytesMut` buffer indefinitely, leading to memory exhaustion and Out-Of-Memory (OOM) crashes.

2. **Unbounded Outbound Session Channels**:
   In `crates/gateway/src/server.rs:1226` and `1464`, every client session creates an unbounded MPSC channel (`mpsc::unbounded_channel::<Bytes>()`) for downstream reliable packets. If a malicious or slow client pauses reading from the TCP or QUIC connection, the gateway buffers broadcast and social messages in memory indefinitely without any backpressure, creating a major heap exhaustion vector.

3. **Slowloris & Unbounded Connection Concurrency**:
   In `crates/gateway/src/main.rs`, the TCP accept loop accepts client connections without any concurrency ceiling or rate limiting. Furthermore, if JWT edge authentication is disabled or not configured, incoming connections lack handshake timeouts, permitting stalled connections to consume sockets and file descriptors indefinitely (Slowloris attack).

## Proposed Solution
1. **Schema Frame Size Validation**:
   - Introduce `MAX_PACKET_PAYLOAD_SIZE = 64 * 1024` (64 KiB) in `crates/schema/src/client.rs`.
   - Update `ClientPacket::decode_frame` to immediately reject any packet with `payload_len > MAX_PACKET_PAYLOAD_SIZE` by returning `ClientCodecError::PacketTooLarge { size, max }`.
   - Add property tests ensuring arbitrary payload lengths greater than 64 KiB fail immediately without buffering.

2. **Bounded Outbound Streams & Saturation Policy**:
   - Replace `mpsc::unbounded_channel::<Bytes>()` in `SessionSink` and connection handlers with bounded `mpsc::channel(1024)`.
   - Update `SessionSink` to store bounded `mpsc::Sender<Bytes>`.
   - Implement non-blocking `try_send` with telemetry counters (`mmo_gateway_session_stream_full_dropped`).
   - Introduce an active disconnect policy when a slow client's reliable channel remains persistently saturated.

3. **Concurrency Limiter & Handshake Timeout**:
   - Introduce a connection concurrency limiter using `tokio::sync::Semaphore` initialized to gateway capacity (or configured ceiling) to reject connections exceeding limits.
   - Enforce an initial handshake/first-packet timeout (e.g. 10 seconds) on every accepted TCP connection even when authenticator is not configured, mitigating Slowloris resource exhaustion.

## Scope & Non-Goals
- **In Scope**:
  - `crates/schema/src/client.rs`: `MAX_PACKET_PAYLOAD_SIZE`, `ClientCodecError::PacketTooLarge`, bounds checking.
  - `crates/gateway/src/server.rs`: `SessionSink` stream channel bounded conversion, backpressure handling, drop telemetry.
  - `crates/gateway/src/main.rs`: TCP accept loop concurrency limiter (`Arc<Semaphore>`) and Slowloris handshake timeouts.
  - Unit, property, and integration tests verifying packet rejection and channel bounded behavior.
- **Non-Goals**:
  - Replacing the underlying transport engine (QUIC/TCP/WebSockets).
  - Redesigning the social pub/sub bus architecture.
