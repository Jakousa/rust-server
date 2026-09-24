# Cross-Node Gameplay & Social Bus Design

## Architecture Overview

To enable communication and cooperative gameplay across spatially partitioned simulation nodes, this design introduces a cluster-wide **Social Bus** layer.

```
       [Client A (Player 1)]                        [Client B (Player 2)]
                 |                                            |
                 v                                            v
     +-----------------------+                    +-----------------------+
     |   Edge Gateway 1      |                    |    Edge Gateway 2     |
     +-----------------------+                    +-----------------------+
                 |                                            |
                 +--------------------+  +--------------------+
                                      |  |
                                      v  v
+-----------------------------------------------------------------------------------------------+
|                                  CLUSTER SOCIAL BUS                                           |
|       (Redis Pub/Sub in production cluster, MemoryBroadcast in tests / single node)           |
|                                                                                               |
|   Channels:                                                                                   |
|   - `chat:global`                  - `chat:guild:<guild_id>`                                  |
|   - `chat:party:<party_id>`        - `whisper:<target_guid>`                                  |
|   - `party:events`                 - `guild:sync:<guild_id>`                                  |
+-----------------------------------------------------------------------------------------------+
                 ^                                            ^
                 |                                            |
     +-----------------------+                    +-----------------------+
     |   Simulation Node 1   |                    |   Simulation Node 2   |
     | (Simulates Player 1)  |                    | (Simulates Player 2)  |
     +-----------------------+                    +-----------------------+
```

---

## 1. The `SocialBus` Trait & Backends (`crates/storage/src/social.rs`)

```rust
#[async_trait::async_trait]
pub trait SocialBus: Send + Sync {
    /// Publishes a message envelope to a specific topic channel.
    async fn publish(&self, channel: &str, message: &SocialEnvelope) -> Result<(), SocialBusError>;

    /// Subscribes to a channel, receiving an asynchronous stream/receiver of messages.
    async fn subscribe(&self, channel: &str) -> Result<tokio::sync::broadcast::Receiver<SocialEnvelope>, SocialBusError>;
}
```

### Social Envelopes
- `ChatMessage`: `channel: ChatChannel (Global, Area, Guild, Party, Whisper)`, `sender: Guid`, `sender_name: String`, `target: Option<Guid>`, `text: String`, `timestamp: u64`.
- `PartyAction`: `Invite { party_id, inviter, invitee }`, `Accept { party_id, member }`, `Leave { party_id, member }`, `Disband { party_id }`.
- `GuildSync`: `guild_id: u64`, `action: GuildSyncAction (VaultUpdated, MemberAdded, MemberRankChanged)`.

---

## 2. Party System Plugin (`crates/gameplay/src/party.rs`)

### Components & State
- `PartyMemberComponent`: `pub party_id: u64`, `pub is_leader: bool`.
- `PartyState`:
  ```rust
  pub struct PartyState {
      pub party_id: u64,
      pub leader: Guid,
      pub members: Vec<Guid>,
      pub max_members: usize, // e.g. 5
  }
  ```
- `PartyManager`:
  Handles formation, joins, leaves, and integrates with `SocialBus` to broadcast `PartyAction`s.

---

## 3. Gateway & Node Integration

- Gateways maintain active client connection streams. When a `whisper:<player_guid>` or `chat:party:<party_id>` envelope arrives over the `SocialBus`, the Gateway sends a FlatBuffers frame to the connected client.
- When an entity on Node 1 performs a party invite or guild vault update, it publishes to the `SocialBus`. Node 2 receives the event and updates its local entity component state in lock-step.
