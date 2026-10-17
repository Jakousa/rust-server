# Implementation Tasks: Gateway Security Hardening

- [x] 1. Enforce Maximum Packet Frame Payload Size in Codec
  - [x] 1.1 Add `MAX_PACKET_PAYLOAD_SIZE = 64 * 1024` constant to `crates/schema/src/client.rs`.
  - [x] 1.2 Add `PacketTooLarge { size: usize, max: usize }` variant to `ClientCodecError`.
  - [x] 1.3 Update `ClientPacket::decode_frame` to validate `payload_len <= MAX_PACKET_PAYLOAD_SIZE` before checking buffer size.
  - [x] 1.4 Update `ClientPacket::decode_bytes` to validate `payload_len <= MAX_PACKET_PAYLOAD_SIZE`.
  - [x] 1.5 Add unit and property tests verifying oversized payloads return `ClientCodecError::PacketTooLarge` immediately without buffering.
  - [x] 1.6 Verify with `cargo test -p mmo-schema`.

- [x] 2. Convert Session Outbound Streams to Bounded Channels
  - [x] 2.1 Update `SessionSink` struct to use bounded `mpsc::Sender<Bytes>` instead of `mpsc::UnboundedSender<Bytes>`.
  - [x] 2.2 Update `SessionSink::new` and `SessionSink::from` constructors to configure bounded stream channels of capacity 1024.
  - [x] 2.3 Update `handle_connection` (TCP) and `handle_quic_connection` (QUIC) in `crates/gateway/src/server.rs` to use bounded `mpsc::channel(1024)`.
  - [x] 2.4 Update `crates/gateway/src/webtransport.rs` session egress to use bounded `mpsc::channel(1024)`.
  - [x] 2.5 Replace all instances of `sink.stream_tx.send(...)` in `GatewayInterceptHandler` and `GatewayServer` with `sink.stream_tx.try_send(...)` and record `mmo_gateway_session_stream_full_dropped` metrics on backpressure.
  - [x] 2.6 Update `crates/node/tests/it/geo_gateway_handover.rs` and gateway tests to use bounded channels with `SessionSink`.
  - [x] 2.7 Verify with `cargo test -p mmo-gateway` and `cargo test -p mmo-node`.

- [x] 3. Implement Concurrency Limiting & Slowloris Handshake Timeouts
  - [x] 3.1 Introduce `Arc<tokio::sync::Semaphore>` in `crates/gateway/src/main.rs` to limit maximum concurrent active client connections.
  - [x] 3.2 Add connection rejection handling when capacity is reached and increment `mmo_gateway_connections_rejected_max_capacity`.
  - [x] 3.3 Add handshake/first-packet timeout enforcement in `handle_connection` when `authenticator` is None to prevent Slowloris resource exhaustion, incrementing `mmo_gateway_handshake_timeout`.
  - [x] 3.4 Add tests for Slowloris handshake timeout and concurrency limits.

- [x] 4. Quality Gates & End-to-End Verification
  - [x] 4.1 Run `cargo fmt --all -- --check`.
  - [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 4.3 Run `cargo test --workspace`.
