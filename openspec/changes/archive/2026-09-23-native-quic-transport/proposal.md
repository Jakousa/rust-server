## Why

The Edge Gateway currently terminates client connections using framed TCP transport, which suffers from head-of-line blocking under packet loss and lacks connection migration over dynamic wireless/mobile networks. Upgrading the Edge Gateway to native QUIC UDP transport using `quinn` with TLS 1.3 enables multiplexed streams, 0-RTT connection resumption, immunity to head-of-line blocking between independent streams, and robust packet loss resilience for high-frequency MMO client traffic.

## What Changes

- **Native QUIC UDP Endpoint**: Add `QuicGatewayServer` / `bind_quic` in `mmo_gateway` using `quinn` over UDP, with configurable TLS 1.3 credentials and automatic self-signed certificate generation for local development.
- **Stream Multiplexing**:
  - Accept bidirectional and unidirectional QUIC streams per client connection.
  - Process framed incoming client packets (`Movement`, `Interaction`, `Ping`) without stream cross-blocking.
  - Route downstream broadcast frames (`EntityState`, `EntityDespawn`, `MovementCorrection`) through the client's QUIC stream sinks.
- **Dual-Stack Transport**: Provide clean dual-stack support in the Gateway, allowing nodes and clients to operate over native QUIC UDP while preserving TCP for fallback.
- **Client Helper**: Provide a lightweight `QuicClientSession` in `mmo_gateway` for automated integration testing and client frontends.
- **Integration Tests**: Add end-to-end tests verifying QUIC handshakes, bidirectional packet flow, and AoI broadcasts over QUIC.

## Capabilities

### New Capabilities
- `gateway/quic-transport`: Native UDP-based QUIC transport termination, TLS 1.3 handshake negotiation, stream multiplexing, and connection lifecycle management.

### Modified Capabilities
- `gateway/routing`: Update Client Transport Termination to mandate native QUIC UDP streaming with bidirectional packet exchange.

## Impact

- `crates/gateway/Cargo.toml`: Add `quinn` (0.11), `rcgen` (0.13), and `rustls` dependencies.
- `crates/gateway/src/quic.rs`: New module implementing QUIC endpoint listener, TLS config, and stream loop.
- `crates/gateway/src/server.rs`: Integration of QUIC connection handling alongside TCP.
- `mmo_schema`: No wire protocol changes; packet encoding/decoding operates seamlessly over QUIC byte streams.
