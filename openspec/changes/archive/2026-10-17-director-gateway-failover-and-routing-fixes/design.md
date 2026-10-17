# Technical Design: Director & Gateway Failover, Octree Boundary Routing & QUIC Interceptors

## 1. Registered Node Address Resolution (`crates/director`)

### 1.1 `PartitionRecoveryEvent` (`crates/director/src/coordinator.rs:360-365`)
Add address fields to `PartitionRecoveryEvent`:
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionRecoveryEvent {
    pub bounds: BoundingBox3D,
    pub previous_node_id: u32,
    pub new_node_id: u32,
    pub previous_node_address: String,
    pub new_node_address: String,
    pub epoch: u64,
}
```
Inside `GlobalOctreeCoordinator::recover_orphaned_partitions` (`crates/director/src/coordinator.rs:766-815`) or with an optional address map `dead_addrs: &HashMap<u32, String>`:
- Before `NodeRegistry::check_dead_nodes` evicts timed-out nodes in `DirectorServiceImpl::check_and_recover_dead_nodes` (`crates/director/src/service.rs:141-183`), capture a `HashMap<u32, String>` mapping `node_id -> node.listen_addr.clone()` from `reg.list_nodes()` (or update `recover_orphaned_partitions` / `check_and_recover_dead_nodes` to populate `previous_node_address` and `new_node_address`).
- Helper fallback function `fn fallback_node_addr(node_id: u32, registered: Option<&str>) -> String`:
  - If `registered` is `Some(addr)` and `!addr.trim().is_empty()`, return `addr.to_string()`.
  - Otherwise return `format!("mmo-node-{}.mmo-node:50052", node_id.saturating_sub(1))`.
- In `DirectorServiceImpl::broadcast_route_reassignments` (`crates/director/src/service.rs:247-259`), use `ev.previous_node_address.clone()` and `ev.new_node_address.clone()` directly in `ReassignNodeRoutesRequest`.
- In `DirectorServiceImpl::deregister_node` (`crates/director/src/service.rs:607-653`):
  - Capture the deregistering node's `listen_addr` from `reg.get_node(req.node_id)` before calling `reg.deregister_node(req.node_id)`.
  - Populate `previous_node_address` and `new_node_address` on any resulting `PartitionRecoveryEvent`s.
  - After releasing locks on `registry` and `coordinator`, if `!events.is_empty()`, call `self.broadcast_route_reassignments(&events, "").await;`.

---

## 2. Background Reapers & Federation Environment Config (`crates/director`)

### 2.1 `crates/director/src/main.rs` (`lines 24-56`)
- Read `GATEWAY_DEAD_TIMEOUT_MS` from env (default `5000` ms).
- Inside the `tokio::spawn` background detector loop (`lines 39-56`):
  ```rust
  let recovery_events = svc_clone.check_and_recover_dead_nodes(dead_timeout).await;
  if !recovery_events.is_empty() {
      svc_clone
          .broadcast_route_reassignments(&recovery_events, &gateway_grpc_addr)
          .await;
  }
  let stale_gateways = svc_clone.check_and_recover_stale_gateways(gateway_timeout).await;
  if !stale_gateways.is_empty() {
      tracing::warn!(?stale_gateways, "Evicted stale edge gateways");
  }
  let reaped_instances = svc_clone.reap_idle_instances().await;
  if reaped_instances > 0 {
      tracing::info!(reaped_instances, "Reaped idle dynamic instances");
  }
  ```

### 2.2 `crates/director/src/service.rs` (`lines 66-135`)
- Add helper `fn federation_authority_from_env() -> RealmFederationAuthority`:
  - `let realm_id = std::env::var("REALM_ID").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "realm-default".to_string());`
  - `let secret = std::env::var("FEDERATION_HMAC_SECRET").ok().filter(|s| s.len() >= 16).map(|s| s.into_bytes()).unwrap_or_else(|| b"cluster-realm-federation-default-secret-key".to_vec());`
  - `RealmFederationAuthority::new(realm_id, secret).expect("valid federation authority")`
- Call `Self::federation_authority_from_env()` in `DirectorServiceImpl::new`, `DirectorServiceImpl::with_gateway_registry`, and `DirectorServiceImpl::default_service`.

---

## 3. Octree 3-Axis Boundary Smoothing, Branch Union Bounds, and Removing `spatial_index` (`crates/director/src/coordinator.rs`)

### 3.1 Remove Unused `spatial_index: Octree`
- In `crates/director/src/coordinator.rs`:
  - Remove `pub spatial_index: Octree` from `GlobalOctreeCoordinator` (`line 394`).
  - Remove `let spatial_index = Octree::new(root_bounds);` in `new` (`line 443`).
  - Remove dummy `Guid::new(EntityType::Player, ...)` insertions/removals in `assign_initial_node` (`lines 497-498`), `handle_split` (`lines 617-630`), `handle_merge` (`lines 728-734, 745-746`), and `recover_orphaned_partitions` (`lines 790-801`).
  - Clean up unused imports (`EntityType, Guid, Octree`) at `line 6`.

### 3.2 Recompute Parent `Branch::bounds` in `GlobalOctreeNode::update_leaf_bounds` (`lines 329-355`)
- Add helper `fn union_bounds(a: BoundingBox3D, b: BoundingBox3D) -> BoundingBox3D`:
  ```rust
  BoundingBox3D::new(
      (a.min.0.min(b.min.0), a.min.1.min(b.min.1), a.min.2.min(b.min.2)),
      (a.max.0.max(b.max.0), a.max.1.max(b.max.1), a.max.2.max(b.max.2)),
  )
  ```
- In `GlobalOctreeNode::update_leaf_bounds` (`lines 329-355`):
  - Do not short-circuit on `if !bounds.intersects(old_bounds)` alone if a child might have been shifted previously; recurse into `children`.
  - When any child returns `true`, recompute `*bounds` of the `Branch` as the union of `children.iter().map(|c| c.bounds())` before returning `true`.
- Similarly in `assign_leaf_node` (`lines 296-325`) and `split_at` (`lines 159-189`), ensure `Branch` traversal checks all children if needed.

### 3.3 Generalize `smooth_adjacent_boundaries` Across X, Y, Z and Both Directions (`lines 646-707`)
- Two leaves `ba` (load `na`, centroid `ca`) and `bb` (load `nb`, centroid `cb`) share a planar boundary along axis `axis` (`0 = X`, `1 = Y`, `2 = Z`) if the other two axes have matching `(min, max)` within `0.001` and either:
  - **Case Forward (`ba` is lower, `bb` is upper)**: `(ba_max - bb_min).abs() < 0.001`
  - **Case Reverse (`bb` is lower, `ba` is upper)**: `(bb_max - ba_min).abs() < 0.001`
- Check load imbalance `if na < 2.0 * nb && nb < 2.0 * na { return None; }`.
- Clamp `ca` to `ba` and `cb` to `bb` on all 3 axes.
- Let `(lower_min, split, upper_max, lower_load, upper_load, lower_c, upper_c)` represent the lower and upper partitions along `axis`:
  - `let imbalance = (lower_load - upper_load) / (lower_load + upper_load);`
  - `let dist = (upper_c - lower_c).abs().max(1.0);`
  - `let shift = (imbalance * dist * 0.1).clamp(-10.0, 10.0);`
  - `let new_split = split + shift;`
  - Verify `new_split > lower_min + 10.0 && new_split < upper_max - 10.0`.
  - Construct `new_ba` and `new_bb` with the updated coordinate along `axis`, call `self.root.update_leaf_bounds(ba, new_ba)` and `self.root.update_leaf_bounds(bb, new_bb)`, increment `self.epoch += 1`, and return `Some(RebalanceAction::BoundaryShift { ... })`.

---

## 4. QUIC Datagram Interception & Stream Guard (`crates/gateway/src/server.rs`)

### 4.1 QUIC Datagram Loop (`lines 1713-1739`)
In `GatewayServer::handle_quic_connection`:
```rust
while let Ok(Some(packet)) = ClientPacket::decode_frame(&mut dgram_buf) {
    if let ClientPacket::Ping(ref ping) = packet {
        let pong = ClientPacket::Pong(PongPacket {
            client_timestamp_ms: ping.client_timestamp_ms,
            server_timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        let _ = conn_dgram.send_datagram(pong.encode_frame());
        continue;
    }

    if intercept_handler.handle_packet(session_id, &packet).await {
        continue;
    }

    let dispatcher = self.dispatcher.clone();
    tokio::spawn(async move {
        let _ = dispatcher.dispatch_packet(session_id, packet).await;
    });
}
```

### 4.2 QUIC Bidirectional Stream Handling (`lines 1744-1925`)
- When `connection.accept_bi()` yields `Ok((send, mut recv))`:
  - Do **not** unconditionally overwrite `*active_send_stream.lock().await = Some(send);` at line 1747!
  - Instead, wrap the stream's own `send` in a local `let mut stream_send = send;` (or `Arc<tokio::sync::Mutex<quinn::SendStream>>`) inside the spawned bi-stream task.
  - Use `stream_send.write_all(&resp).await` directly for `AuthResponse` (lines 1812, 1852, 1866) and stream `Pong` (line 1901) on that stream.
  - Only if authentication succeeds (or if `authenticator.is_none()`) AND `active_stream.lock().await.is_none()`, populate `*active_stream.lock().await = Some(...)` (or let `downstream_handle` lazily open a server-to-client stream via `conn_writer.open_bi().await` whenever `active_send_stream` is `None`, which is already implemented at lines 1666-1673!).
  - Notice lines 1663-1673 in `downstream_handle`:
    ```rust
    let mut lock = active_stream_writer.lock().await;
    if lock.is_none() {
        if let Ok((send, _recv)) = conn_writer.open_bi().await {
            *lock = Some(send);
        }
    }
    ```
    And if `send.write_all(&bytes).await` returns `Err(_)`, set `*lock = None;` so failed streams are evicted!
    If `accept_bi()` writes its own `AuthResponse` and `Pong` directly to its own `mut send` half (and only installs `send` into `active_stream` if `lock.is_none()` after authentication), unauthenticated stream opens can never overwrite `active_send_stream`, and concurrent client streams never clobber each other!
