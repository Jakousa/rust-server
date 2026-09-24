## Purpose

Provides native UDP-based QUIC transport termination, TLS 1.3 handshake negotiation, stream multiplexing, and connection migration support.

## ADDED Requirements

### Requirement: QUIC Endpoint Termination
The Edge Gateway SHALL bind a UDP socket using QUIC with TLS 1.3 and accept incoming client connections with automatic certificate provisioning.

#### Scenario: Client connects to QUIC endpoint
- **WHEN** a client initiates a QUIC connection to the gateway UDP port
- **THEN** the gateway SHALL complete the TLS 1.3 handshake and establish a multiplexed QUIC connection

### Requirement: Multiplexed Stream Handling
The Edge Gateway SHALL accept multiple concurrent streams per QUIC connection, processing client frames without head-of-line blocking between streams.

#### Scenario: Concurrent stream packet delivery
- **WHEN** a client transmits packets across multiple independent QUIC streams concurrently
- **THEN** packet loss on one stream SHALL not block frame processing or downstream broadcasts on other streams

### Requirement: Bidirectional Downstream Streaming
The Edge Gateway SHALL maintain send streams on the QUIC connection to push authoritative server packets (`EntityState`, `EntityDespawn`, `MovementCorrection`) downstream to the client.

#### Scenario: Downstream broadcast over QUIC
- **WHEN** a simulation node produces AoI broadcast packets for a client session connected via QUIC
- **THEN** the gateway SHALL write the encoded binary frames across the client's QUIC send stream without blocking inbound traffic
