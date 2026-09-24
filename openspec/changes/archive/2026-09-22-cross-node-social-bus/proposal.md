## Why

Our simulation cluster partitions entities across multiple authoritative nodes based on spatial octree slices. However, social gameplay features—including direct whispers, global chat channels, guild announcements, party grouping, and cross-node interactions—transcend spatial partitions. When a player on Node 1 whispers or invites a player on Node 2, the message cannot be delivered via localized spatial memory.

We need a unified, low-latency **Cross-Node Gameplay & Social Bus** that bridges communication, party coordination, and guild state synchronization across all simulation nodes and edge gateways.

## What Changes

- Define `SocialBus` trait in `crates/storage` (or `crates/core`):
  - Trait supporting publish/subscribe and point-to-point routing of social envelopes (`ChatMessage`, `PartyAction`, `GuildSync`).
  - `MemorySocialBus`: In-memory broadcast channel implementation for unit testing and standalone mode.
  - `RedisSocialBus`: Scalable Redis Pub/Sub implementation for multi-node production clusters.
- Implement Party System in `crates/gameplay/src/party.rs`:
  - `PartyMemberComponent`, `PartyState`, invite lifecycle, leader delegation, and cross-node member sync.
- Implement Social & Chat dispatching in `crates/gameplay/src/social.rs`:
  - Channels: `Global`, `Area`, `Guild`, `Party`, `Whisper`.
  - Content filtering, delivery routing to target player sessions via `SessionCacheBackend`.
- Integrate `SocialBus` into `crates/node` and `crates/gateway`:
  - Gateways push incoming client chat packets onto the bus and relay matching outgoing messages to player streams.
  - Simulation nodes synchronize cross-node party states and guild events.
- Add unit tests and an end-to-end integration test demonstrating cross-node whisper delivery, party coordination, and guild sync across separate simulation instances.

## Capabilities

### New Capabilities
- `communication/social-bus`: Provides low-latency cross-node pub/sub messaging for chat channels, whispers, party management, and guild state replication.
- `gameplay/party`: Provides group creation, member joins/leaves, leader delegation, and party state synchronization across nodes.

## Impact

- Allows players simulated on different nodes to communicate, party up, and share live guild updates seamlessly.
- Leverages existing Redis infrastructure in `docker-compose.yml` and `crates/storage`.
- Preserves full zero-coupling plugin architecture.
