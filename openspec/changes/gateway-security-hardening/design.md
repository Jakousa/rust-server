# Gateway Security Hardening Technical Design

## Architectural Context
The Edge Gateway terminates client transport protocols (TCP, QUIC, WebTransport) and demultiplexes client packets to simulation nodes via gRPC and social channels via pub/sub. To protect the cluster against malicious clients, we must enforce defense-in-depth across the packet codec, session channel buffers, and connection acceptance pipelines.

---

## Technical Components

### 1. Schema Codec Max Frame Size Limit
- **Constant**:
  ```rust
  pub const MAX_PACKET_PAYLOAD_SIZE: usize = 64 * 1024; // 64 KiB
  ```
- **Error Enum Update** (`ClientCodecError` in `crates/schema/src/client.rs`):
  ```rust
  #[error("Packet payload too large: {size} bytes exceeds maximum allowed {max} bytes")]
  PacketTooLarge { size: usize, max: usize },
  ```
- **Decoding Validation** (`decode_frame` and `decode_bytes`):
  In `ClientPacket::decode_frame`:
  ```rust
  let payload_len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
  if payload_len > MAX_PACKET_PAYLOAD_SIZE {
      return Err(ClientCodecError::PacketTooLarge {
          size: payload_len,
          max: MAX_PACKET_PAYLOAD_SIZE,
      });
  }
  let total_frame_size = HEADER_SIZE + payload_len;
  if buf.len() < total_frame_size {
      return Ok(None);
  }
  ```
  In `ClientPacket::decode_bytes`:
  ```rust
  let payload_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
  if payload_len > MAX_PACKET_PAYLOAD_SIZE {
      return Err(ClientCodecError::PacketTooLarge {
          size: payload_len,
          max: MAX_PACKET_PAYLOAD_SIZE,
      });
  }
  ```
- If an invalid header specifies an oversized payload, decoding fails immediately on the 8-byte header without waiting for up to 4 GiB of data to arrive.

### 2. Bounded Reliable Session Channels
- **SessionSink Struct** (`crates/gateway/src/server.rs`):
  ```rust
  pub struct SessionSink {
      /// Bounded channel for unreliable datagrams (e.g. movement, entity deltas).
      pub datagram_tx: mpsc::Sender<Bytes>,
      /// Bounded channel for reliable packets (e.g. interaction results, trades, auth).
      pub stream_tx: mpsc::Sender<Bytes>,
  }
  ```
- Channel capacity for `stream_tx` is set to `STREAM_CHANNEL_CAPACITY = 1024`.
- In `SessionSink::from(datagram_tx: mpsc::Sender<Bytes>)`:
  ```rust
  let (stream_tx, _) = mpsc::channel(1024);
  ```
- In all callers (`crates/gateway/src/server.rs`, `crates/gateway/src/webtransport.rs`, `crates/node/tests/it/geo_gateway_handover.rs`):
  Replace `mpsc::unbounded_channel` with bounded `mpsc::channel(1024)`.
- In `send_bytes_to_session`, `send_packet_to_session`, `broadcast_packet`, and `GatewayInterceptHandler`:
  Replace `sink.stream_tx.send(bytes)` with `sink.stream_tx.try_send(bytes)`.
  If `try_send` encounters `TrySendError::Full`:
  - Increment `mmo_gateway_session_stream_full_dropped`.
  - Increment `mmo_gateway_packets_dropped_total`.
  - When reliable sending fails due to channel exhaustion, the gateway drops the packet and may close the channel or remove the sink to trigger connection shutdown.
- In `handle_connection` (TCP), `handle_quic_connection` (QUIC), and WebTransport:
  Instantiate `let (stream_tx, mut stream_rx) = mpsc::channel::<Bytes>(1024)`.

### 3. TCP Connection Concurrency & Slowloris Mitigation
- **Connection Semaphore**:
  In `crates/gateway/src/main.rs`:
  ```rust
  let connection_semaphore = Arc::new(tokio::sync::Semaphore::new(capacity));
  ```
  Before accepting or spawning a connection handler:
  ```rust
  let permit = match connection_semaphore.clone().try_acquire_owned() {
      Ok(p) => p,
      Err(_) => {
          metrics::counter!("mmo_gateway_connections_rejected_max_capacity").increment(1);
          eprintln!("[Gateway] Connection rejected: maximum capacity ({capacity}) reached");
          continue; // Drops connection immediately
      }
  };
  ```
  The permit is moved into the spawned connection task and dropped automatically when the session terminates.

- **Slowloris Handshake Timeout**:
  In `handle_connection`, if `self.authenticator` is `None`:
  Enforce a fallback handshake timeout (`DEFAULT_HANDSHAKE_TIMEOUT = Duration::from_secs(10)`):
  The first packet MUST be received within 10 seconds of connection establishment. If no packet arrives within 10 seconds, the connection is closed with an error:
  ```rust
  metrics::counter!("mmo_gateway_handshake_timeout").increment(1);
  ```

---

## Concurrency & Deadlock Assessment
- `try_acquire_owned()` on `tokio::sync::Semaphore` is lock-free and never blocks the accept loop.
- `try_send()` on `mpsc::Sender` is non-blocking and will never deadlock writers or broadcast threads.
- Moving the `OwnedSemaphorePermit` into the connection task guarantees RAII release on client disconnect, panic, or network reset.

---

## Observability & Metrics
- `mmo_gateway_connections_rejected_max_capacity`: Counter for rejected connections when semaphore capacity is reached.
- `mmo_gateway_handshake_timeout`: Counter for connections terminated due to Slowloris / handshake timeout.
- `mmo_gateway_session_stream_full_dropped`: Counter for reliable packets dropped due to backpressure in saturated session stream queues.
