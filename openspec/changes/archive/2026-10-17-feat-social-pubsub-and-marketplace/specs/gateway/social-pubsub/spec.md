# Specification: Gateway Social Pub/Sub Event Bus

## Capability: `gateway/social-pubsub`

### Feature: Packet Interception and Simulation Bypass
- **GIVEN** an active client connection on `mmo-gateway` (TCP, QUIC, or WebTransport)
- **WHEN** the client sends a `SocialMessage`, `SocialSubscribe`, or `SocialUnsubscribe` packet
- **THEN** `mmo-gateway` SHALL intercept the packet directly at the gateway layer
- **AND** SHALL NOT forward the packet to the `packet_tx` channel or any `mmo-node` simulation instance
- **AND** the 20 Hz simulation loop of `mmo-node` SHALL remain completely unburdened.

### Feature: Anti-Spoofing Identity Validation
- **GIVEN** an authenticated client session associated with player GUID `P`
- **WHEN** the client transmits a `SocialMessage` claiming `sender_guid = Q` where `Q != P`
- **THEN** the gateway SHALL reject or overwrite `sender_guid` with `P`
- **AND** increment `mmo_gateway_spoofed_social_packets_dropped_total`.

### Feature: Cross-Instance Whisper and Channel Routing
- **GIVEN** Player A connected to Gateway 1 and Player B connected to Gateway 2 (or the same Gateway)
- **WHEN** Player A publishes a whisper to Player B (`target_guid = B`)
- **THEN** the message SHALL be published to topic `whisper:B` on the cluster `SocialBus`
- **AND** Player B's gateway SHALL receive the bus event and transmit a `SocialMessage` packet to Player B's `SessionSink` reliable stream
- **AND** no spatial simulation nodes SHALL participate in the delivery path.

### Feature: Dynamic Channel Subscription Lifecycle
- **GIVEN** a connected client session
- **WHEN** the client issues a `SocialSubscribe` for a channel (e.g. `chat:guild:123` or custom topic)
- **THEN** the gateway subscription manager SHALL register the session's interest in that topic
- **AND** subsequent messages on that topic SHALL be fanned out to that session
- **AND** upon disconnect or `SocialUnsubscribe`, the subscription SHALL be cleanly revoked without leaks.
