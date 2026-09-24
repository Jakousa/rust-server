## MODIFIED Requirements

### Requirement: Client Transport Termination
The Edge Gateway SHALL accept client connections over native QUIC UDP transport and deserialize binary game packets from multiplexed bidirectional or unidirectional streams.

#### Scenario: Client connects to gateway
- **WHEN** a client establishes a native QUIC connection over UDP with the Edge Gateway
- **THEN** the gateway SHALL negotiate the TLS 1.3 session and maintain the active client connection streams
