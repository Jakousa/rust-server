# Tasks: Security Hardening Implementation

- [x] 1. Movement Speed Exploit & Sequence Monotonicity (`crates/node`) <!-- id: 1-movement-validation -->
  - [x] 1.1 Add `time_credit: f32` to `EntityKinematicState` in `crates/node/src/kinematics.rs` initialized to `0.2` on creation.
  - [x] 1.2 Update `compute_server_dt_with_seq` and `process_client_move_with_seq` in `crates/node/src/sim.rs`: replenish `time_credit` by `real_elapsed` capped at `0.2s`, verify `time_credit >= dt_nominal`, reject without draining credit if insufficient, and deduct `time_credit -= dt_nominal` only on successful move validation.
  - [x] 1.3 Enforce strict sequence monotonicity for all movement packets (`sequence > state.last_sequence`), rejecting `sequence = 0` re-use, and auto-advance sequence in `process_client_move`.
  - [x] 1.4 Add unit tests verifying burst rate-limiting (e.g. 100 packets in 200ms cannot exceed bounded distance) and sequence 0 replay rejection.

- [x] 2. Gateway Social Bus Authorization & Anti-Spoofing (`crates/gateway`) <!-- id: 2-social-auth -->
  - [x] 2.1 Enforce session ownership in `GatewayInterceptHandler` for `SocialMessage`: drop packet and increment metric if unbound or if `msg.sender_guid != bound_guid.raw()`.
  - [x] 2.2 Enforce whisper destination and authentication in `SocialSubscribe`: require `bound_guid`, restrict `channel_type == 1` and `channel_name.starts_with("whisper:")` strictly to `bound_guid.raw()`, and reject restricted prefixes in custom channels.
  - [x] 2.3 Add unit tests verifying unauthorized whisper subscriptions and unbound session message spoofing are rejected.

- [x] 3. Epoch Fencing, Gateway Epoch Forwarding & Despawn Tombstoning (`crates/gateway`, `crates/node`) <!-- id: 3-epoch-fencing -->
  - [x] 3.1 Track `session_epoch` in `SessionRouter` (`session_to_epoch: DashMap<u64, u64>`), set in `authenticate_and_register_session`, and pass it in `RoutedPacket` and `ForwardClientPacketRequest`.
  - [x] 3.2 Add `tombstoned_entities: HashSet<Guid>` in `TickSimulation`, record tombstone on `despawn_entity` and `migration.rs`, and clear tombstone on `ingest_migrated_entity` and `hydrate_player_session`.
  - [x] 3.3 Ensure `forward_client_packet` checks `sim.tombstoned_entities.contains(&guid)` and rejects late packets for despawned/migrated entities without re-hydrating duplicate player instances.
  - [x] 3.4 Add unit tests verifying late in-flight movement packets after despawn/migration are dropped.

- [x] 4. Floating-Point `.is_finite()` Sanitization (`crates/schema`, `crates/director`, `crates/node`) <!-- id: 4-float-sanitization -->
  - [x] 4.1 Reject non-finite floats in `Vec3::decode`, `MovementPacket::decode`, and `PlaceObjectPacket::decode` with `ClientCodecError::InvalidData`.
  - [x] 4.2 Guard against non-finite floats in `quantize_yaw_u8`, `quantize_pitch_i8`, and `quantize_delta_i16`.
  - [x] 4.3 Validate finite values in `NodeRegistry::record_heartbeat` and component-wise finite & min <= max in `proto_bbox_to_bounding_box_3d` / `report_split`.
  - [x] 4.4 Reject non-finite caster positions in `execute_remote_ability` and non-finite floats in `GhostStore::upsert_from_proto`.
  - [x] 4.5 Add unit tests for NaN/Inf rejection across codec, Director, ability, and ghost boundaries.

- [x] 5. TTL Integer Overflow Bounds & Bounded Pre-Auth Buffers (`crates/storage`, `crates/director`, `crates/gateway`) <!-- id: 5-ttl-bounds -->
  - [x] 5.1 Enforce bounded duration (max 30 days) and saturating arithmetic in `MarketplaceService::create_listing` and `EscrowCoordinator::create_escrow`.
  - [x] 5.2 Protect against TTL overflow in `MemoryCache::set_session` and `stage_handover`.
  - [x] 5.3 Bound TTL (max 24h) and cap `redeemed_nonces` capacity in `RealmFederationAuthority`.
  - [x] 5.4 Enforce `MAX_PACKET_PAYLOAD_SIZE` cap and explicit codec error abortion on pre-auth buffers in `webtransport.rs` and `server/mod.rs`.
  - [x] 5.5 Add unit tests verifying extreme TTL values and pre-auth buffer overflow protection.

- [x] 6. Quality Gates & Verification <!-- id: 6-quality-gates -->
  - [x] 6.1 Run `cargo fmt --all -- --check`.
  - [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 6.3 Run full workspace test suite `cargo test --workspace`.
  - [x] 6.4 Sync specs to `openspec/specs/` and archive change in `openspec/changes/archive/`.
  - [x] 6.5 Push feature branch, open PR, and enable auto-merge.
