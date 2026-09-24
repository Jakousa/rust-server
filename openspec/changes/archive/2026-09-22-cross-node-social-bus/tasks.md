## 1. Social Bus Abstractions (`crates/storage/src/social.rs`)

- [x] 1.1 Implement `SocialEnvelope`, `ChatMessage`, `PartyAction`, and `GuildSync` data models
- [x] 1.2 Implement `SocialBus` trait with `MemorySocialBus` and `RedisSocialBus` backends
- [x] 1.3 Add unit tests verifying pub/sub broadcast and point-to-point whisper filtering

## 2. Party Gameplay Plugin (`crates/gameplay/src/party.rs`)

- [x] 2.1 Implement `PartyMemberComponent`, `PartyState`, and `PartyManager` lifecycle
- [x] 2.2 Implement `PartyPlugin` registering component codecs and modular packet handlers for party actions
- [x] 2.3 Add unit tests verifying party invites, accepts, leaves, and leader delegation

## 3. Cross-Node Integration & Verification

- [x] 3.1 Wire `SocialBus` into `crates/gateway` for relaying client chat messages and routing whispers
- [x] 3.2 Wire `SocialBus` into `crates/gameplay` for party and guild event replication
- [x] 3.3 Create end-to-end integration test verifying cross-node whisper delivery, party formation, and guild synchronization across disparate simulation nodes
