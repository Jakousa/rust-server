## Context

The Edge Gateway terminates client connections and multiplexes game packets across authoritative simulation nodes via `SessionRouter`. It currently provides TCP connection listening (`bind` and `handle_connection`) with split reader/writer tasks and downstream session sinks (`session_sinks`). The wire protocol packets (`Movement`, `EntityState`, `EntityDespawn`, `Interaction`) use compact binary framing with `ClientPacket::decode_frame`.

TCP suffers from head-of-line blocking when packets drop on lossy networks and cannot seamlessly migrate connections when client IPs change. Upgrading to native QUIC UDP transport resolves both issues. See `proposal.md` for motivation.

## Goals / Non-Goals

**Goals:**
- Implement native QUIC UDP transport endpoint (`QuicEndpoint` / `bind_quic`) using `quinn` with TLS 1.3.
- Provide automatic self-signed certificate generation using `rcgen` for seamless development and automated testing, with support for production TLS certificates.
- Eliminate head-of-line blocking by combining unreliable QUIC Datagrams (for high-frequency movement and entity state sync) with reliable QUIC Streams (for transactional interactions and chat).
- Integrate QUIC connection handling with existing `PacketDispatcher`, `SessionRouter`, and downstream `session_sinks`.
- Maintain dual-transport capability: Edge Gateway can bind both TCP and QUIC listeners simultaneously.
- Provide a `QuicClientSession` helper for tests and future visual client integration with a bypass certificate verifier for local testing.

**Non-Goals:**
- Modifying internal cluster gRPC communication (node-to-director or node-to-node).
- WebTransport HTTP/3 encapsulation (native QUIC is optimal for standalone native clients like Bevy; browser WASM support can build upon this later).
- Custom congestion control algorithms (leveraging Quinn's default BBR / Cubic).

## Decisions

### 1. `quinn` + `rcgen` + `rustls` Architecture
- We introduce `crates/gateway/src/quic.rs`:
  - `QuicConfig`: Server TLS certificates, private keys, idle timeouts (default 30s), keep-alive intervals.
  - Certificate generation helper: `generate_self_signed_cert(&[&str])` producing DER-encoded cert and private key using `rcgen`.
  - Endpoint builder: `create_quic_server(addr: SocketAddr, certs: Vec<CertificateDer>, key: PrivateKeyDer) -> quinn::Endpoint` with `max_datagram_frame_size(Some(VarInt::from_u32(1400)))` enabled on `TransportConfig`.

### 2. Hybrid Datagram & Stream Architecture (Eliminating Head-of-Line Blocking)
- Rather than funneling all packets into a single ordered stream (which would suffer head-of-line blocking), we adopt a hybrid transport model:
  1. **QUIC Datagrams (`send_datagram` / `read_datagram`)**:
     - Used for high-frequency, ephemeral state: `Movement` (52 bytes), `EntityState` (60 bytes), `EntityDespawn` (8 bytes), `Ping`, `Pong`.
     - Zero head-of-line blocking: dropped movement/state packets never block subsequent updates.
  2. **Reliable QUIC Streams (`accept_bi` / `open_bi`)**:
     - Used for critical transactional game actions: `Interaction`, `InteractionResult`, and chat messages where guaranteed delivery and ordering are mandatory.
- In `handle_quic_connection`:
  - The connection runs concurrent event loops via `tokio::select!`:
    - Loop 1: Inbound QUIC datagrams $\rightarrow$ decode frame $\rightarrow$ dispatch packet.
    - Loop 2: Inbound stream chunks $\rightarrow$ decode frame $\rightarrow$ dispatch packet.
    - Loop 3: Downstream session sink $\rightarrow$ write datagrams for ephemeral updates and stream for transactional packets.

### 3. `QuicClientSession` Test & Client Harness
- A test and client utility allowing clients to connect to the QUIC gateway:
  ```rust
  pub struct QuicClientSession {
      pub connection: quinn::Connection,
      pub send_stream: Option<quinn::SendStream>,
      pub recv_stream: Option<quinn::RecvStream>,
      pub read_buf: BytesMut,
  }
  ```
- Implements `connect(server_addr, server_name)` with `rustls::client::danger::ServerCertVerifier` to accept development certificates in test mode.
- Exposes:
  - `send_datagram(&self, pkt: &ClientPacket) -> Result<()>`
  - `read_datagram(&self) -> Result<ClientPacket>`
  - `send_packet_stream(&mut self, pkt: &ClientPacket) -> Result<()>`
  - `recv_packet_stream(&mut self) -> Result<ClientPacket>`

## Risks / Trade-offs

- **[Certificate Verification in Local Tests]** $\rightarrow$ Mitigated by self-signed certificate generation with a dedicated test verifier in `QuicClientSession` that accepts the local test certificate.
- **[UDP Datagram MTU Constraints]** $\rightarrow$ Mitigated because game sync packets (`Movement`: 52B, `EntityState`: 60B) are well under standard 1200-byte path MTU. Any oversized custom payloads fall back to reliable streams.
- **[Stream Exhaustion]** $\rightarrow$ Mitigated by using datagrams for frequent updates, keeping stream count minimal.
