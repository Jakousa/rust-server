## Purpose

Defines cluster-wide cross-node communication for direct messaging, global/guild/party chat channels, and group synchronization across simulation nodes and edge gateways.

## ADDED Requirements

### Requirement: Cross-Node Social Message Bus
The cluster SHALL provide a pluggable `SocialBus` supporting topic-based pub/sub and point-to-point dispatch across simulation nodes and edge gateways.

#### Scenario: Publishing global chat message
- **WHEN** a player on Simulation Node 1 sends a global chat message
- **THEN** the `SocialBus` SHALL broadcast the message to all subscriber nodes and edge gateways in sub-millisecond latency

#### Scenario: Point-to-point whisper routing
- **WHEN** Player A whispers Player B residing on a different simulation node
- **THEN** the `SocialBus` SHALL route the whisper envelope directly to the gateway connection terminating Player B's session

### Requirement: Cross-Node Party Management
The system SHALL coordinate party formation, invitations, joins, leaves, and disbanding across disparate simulation nodes.

#### Scenario: Cross-node party invite and join
- **WHEN** Player 1 on Node 1 invites Player 2 on Node 2 to a party
- **THEN** Player 2 SHALL receive the invitation and, upon acceptance, both nodes SHALL update their local `PartyMemberComponent` with the synchronized party roster

### Requirement: Distributed Guild Event Synchronization
The system SHALL replicate real-time guild events (roster changes, member promotions, vault deposits) across all nodes housing guild members.

#### Scenario: Synchronizing guild vault updates
- **WHEN** a guild member on Node 1 deposits an item into the shared guild vault
- **THEN** the `SocialBus` SHALL broadcast a vault update event so other guild members on Node 2 immediately reflect the updated vault contents
