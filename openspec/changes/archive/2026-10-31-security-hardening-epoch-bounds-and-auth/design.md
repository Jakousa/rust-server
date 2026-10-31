# Architecture & Technical Design: Security Hardening

## Architecture Overview

This design addresses five high-severity security vectors spanning client wire decoding, gateway routing & pubsub authorization, node simulation kinematics & epoch fencing, and director telemetry & federation bounds.

```mermaid
flowchart TD
    Client["Client (Untrusted)"]
    GW["Edge Gateway"]
    Node["Simulation Node"]
    Dir["Cluster Director"]
    Store["Storage Tier"]

    Client -->|1. Sanitized Packets (is_finite, size cap)| GW
    GW -->|2. Authorized Social Topics & Injected Session Epoch| Node
    GW -->|3. ForwardClientPacket with true epoch| Node
    Node -->|4. Time-Credit Kinematics & Sequence Monotonicity| Node
    Node -->|5. Explicit Tombstones on Migration/Despawn| Node
    Node -->|6. Sanitized Heartbeat & Split Bounds| Dir
    GW & Dir -->|7. Checked TTL & Bounded Nonces| Store
```

---

## Detailed Component Design

### 1. Kinematic Time-Credit Replenishing Bucket & Sequence Monotonicity (`crates/node`)
- **Vulnerability**: In `crates/node/src/sim.rs:1243-1246`, `compute_server_dt_with_seq` resets `last_move_time = now` on every packet, and if `elapsed < TICK_DT_SEC`, it grants `(0.05 * seq_delta.max(1)).min(0.2)`. A client sending 100 packets in 200ms receives 5.0 seconds of movement budget. Furthermore, `sequence > 0` condition allows `sequence = 0` to bypass monotonicity checks completely.
- **Solution**:
  - Extend `EntityKinematicState` in `crates/node/src/kinematics.rs` with `time_credit: f32`.
  - When an entity state is created: initialize `time_credit: f32 = 0.2` (allowing 4 ticks burst tolerance on initial movement).
  - When a packet arrives at server time `now`:
    - `real_elapsed = now.duration_since(state.last_move_time).as_secs_f32()`.
    - `state.last_move_time = now`.
    - `state.time_credit = (state.time_credit + real_elapsed).min(0.2)`.
    - Nominal sequence duration required: `let dt_nominal = (Self::TICK_DT_SEC * (seq_delta.max(1) as f32)).min(0.2)`.
    - If `state.time_credit < dt_nominal`, reject the move immediately with snap-back (rate-limited) **without** deducting `state.time_credit`.
    - When movement passes validation: deduct `state.time_credit -= dt_nominal`.
    - This mathematically guarantees that no matter how many packets a client bursts, the total distance moved cannot exceed `max_speed * (real_elapsed_time + MAX_BURST_ALLOWANCE) * tolerance_multiplier`.
  - **Sequence Monotonicity Check**:
    - For initialized entities, if `sequence <= state.last_sequence`, the packet is immediately rejected with snap-back.
    - `sequence = 0` is subject to the exact same monotonicity check (preventing replay attacks).
    - `process_client_move` (convenience helper for tests/internal logic) automatically advances sequence: `state.last_sequence + 1`.

### 2. Social Bus Authorization & Anti-Spoofing (`crates/gateway`)
- **Vulnerability**: `GatewayInterceptHandler` permits unbound sessions to publish `SocialMessage` with spoofed `sender_guid`, and permits any session to subscribe to `whisper:{target_id}` for arbitrary victim player GUIDs.
- **Solution**:
  - In `GatewayInterceptHandler`:
    - For `SocialMessage`: Lookup session in `self.router.get_guid_by_session(session_id)`. If `None`, or if `bound_guid.raw() != msg.sender_guid`, drop the message, increment `mmo_gateway_spoofed_social_packets_dropped_total`, and return `true`.
    - For `SocialSubscribe`:
      - Require session to have a valid `bound_guid` from `self.router.get_guid_by_session(session_id)`. If unbound, drop the subscription.
      - If subscribing to `channel_type == 1` (`whisper`), ensure `sub.target_id == bound_guid.raw()`.
      - If subscribing via `sub.channel_name`:
        - If channel name starts with `"whisper:"`, ensure it strictly equals `format!("whisper:{}", bound_guid.raw())`.
        - Reject custom channel names that attempt to impersonate `"whisper:"` or internal prefixes.
      - If validation fails, drop the subscription request.

### 3. Session Epoch Propagation & Migration Tombstoning (`crates/gateway` & `crates/node`)
- **Vulnerability**:
  - `crates/gateway/src/main.rs:120` hardcodes `session_epoch: 0` in `ForwardClientPacketRequest`.
  - On entity despawn and migration, `session_epochs.remove(&guid)` deletes the epoch entry, causing `is_valid_upstream_epoch` to default to `0`. Any late in-flight packet with `session_epoch >= 0` passes and re-hydrates the entity.
- **Solution**:
  - **Gateway**:
    - In `SessionRouter`, maintain `session_to_epoch: DashMap<u64, u64>`.
    - In `authenticate_and_register_session`, register `session_epoch` in `SessionRouter`.
    - Add `session_epoch: u64` to `RoutedPacket`, populated during packet dispatch from `SessionRouter`.
    - In `crates/gateway/src/main.rs`, pass `packet.session_epoch` in `ForwardClientPacketRequest`.
  - **Node**:
    - Decouple cluster node migration from gateway session epoch takeovers.
    - In `TickSimulation`, add `pub tombstoned_entities: HashSet<Guid>`.
    - When an entity is despawned or migrated away in `despawn_entity` and `migration.rs`:
      - Insert into `self.tombstoned_entities.insert(guid)`.
      - Do not reset `session_epochs` to 0.
    - When an entity is received or legitimately restored in `ingest_migrated_entity` or `hydrate_player_session`:
      - Remove from `self.tombstoned_entities.remove(&guid)`.
    - In `forward_client_packet`:
      - If `!sim.entity_store.contains_guid(guid)`:
      - If `sim.tombstoned_entities.contains(&guid)`: drop packet, increment `mmo_node_stale_epoch_packets_dropped_total`, and return error indicating entity has migrated or despawned.
      - Initial player auto-hydration only proceeds if entity is NOT tombstoned.

### 4. Floating-Point Sanitization (`crates/schema`, `crates/director`, `crates/node`)
- **Vulnerability**: Missing `.is_finite()` checks allow `NaN` and `±Inf` to bypass range checks, poison EWMA telemetry, and corrupt spatial data.
- **Solution**:
  - `crates/schema/src/client.rs`:
    - `Vec3::decode`: check `x.is_finite() && y.is_finite() && z.is_finite()`. Return `Err(ClientCodecError::InvalidData(...))` if false.
    - `MovementPacket::decode`: check `yaw.is_finite() && pitch.is_finite()`.
    - `PlaceObjectPacket::decode`: check `pos`, `rot` (4 floats), and `scale` for finiteness.
  - `crates/schema/src/quantize.rs`:
    - In `quantize_yaw_u8`: change `if yaw.is_nan()` to `if !yaw.is_finite() { return 0; }`.
    - In `quantize_pitch_i8`: add `if !pitch.is_finite() { return 0; }`.
    - In `quantize_delta_i16`: add `if !val.is_finite() { return 0; }`.
  - `crates/director/src/registry.rs`:
    - In `NodeRegistry::record_heartbeat`: validate `request.cpu_percent.is_finite()`, `request.tick_duration_ms.is_finite()`, and centroid coordinates before updating node state.
  - `crates/director/src/service.rs`:
    - In `proto_bbox_to_bounding_box_3d`: ensure `min` and `max` coordinates are finite and `min <= max` component-wise.
    - In `report_split`: validate `original_bounds` and every `child_bounds` with `validate_instance_bounds`.
  - `crates/node/src/network/service.rs`:
    - In `execute_remote_ability`: check `pos.x.is_finite() && pos.y.is_finite() && pos.z.is_finite()`. If false, return `RemoteAbilityOutOfRange` error.
  - `crates/node/src/network/ghost.rs`:
    - In `GhostStore::upsert_from_proto`: check `pos`, `vel`, `yaw`, and `pitch` for finiteness. If non-finite, reject the record.

### 5. TTL Integer Overflow Bounds & Bounded Handshake Buffers (`crates/storage`, `crates/director`, `crates/gateway`)
- **Vulnerability**: Large TTL inputs cause integer overflow in `(ttl_secs as i64 * 1000)`. Pre-auth stream buffers can grow without bound and ignore decoding errors.
- **Solution**:
  - In `MarketplaceService::create_listing`: cap `duration_secs` to `MAX_LISTING_DURATION_SECS` (30 days = 2,592,000s) and compute `expires_at_epoch_ms` using `now_ms.saturating_add(duration_ms)`.
  - In `EscrowCoordinator::create_escrow`: cap `ttl_seconds` to `MAX_ESCROW_TTL_SECS` (30 days) and compute `expires_at_ms` using `now_ms.saturating_add(ttl_ms)`.
  - In `MemoryCache::set_session` and `stage_handover`: cap `ttl_secs` to `MAX_CACHE_TTL_SECS` (30 days) and use `Instant::now().checked_add(Duration::from_secs(ttl_secs)).unwrap_or_else(...)`.
  - In `RealmFederationAuthority`:
    - Cap `ttl` in `issue_claim` to `MAX_CLAIM_TTL` (24 hours). Use `saturating_add`.
    - In `verify_claim`: cap `redeemed_nonces` capacity (e.g. 100,000 entries); when full, evict expired nonces or enforce a hard cap.
  - In `GatewayServer` pre-auth handshake loops (`crates/gateway/src/webtransport.rs` and `crates/gateway/src/server/mod.rs`):
    - Explicitly match `ClientPacket::decode_frame(&mut buf)`:
      - `Ok(Some(pkt)) => return Ok(pkt)`
      - `Ok(None) => {}` (await more bytes)
      - `Err(e) => { metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "malformed_frame").increment(1); return Err(GatewayError::Codec(e)); }`
    - If `buf.len() > MAX_PACKET_PAYLOAD_SIZE` (65,536 bytes) before authentication succeeds, immediately abort and drop the connection.
