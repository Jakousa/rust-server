# Master Product & Engineering Roadmap (`openspec/ROADMAP.md`)

This document is the single source of truth for bringing the **Distributed Rust MMO Server Platform** to 100% production fruition across all MMO genres (Tab-Target RPGs, Voxel/Survival Sandboxes, Tactical/Shooter MMOs, and 6-DoF Space/Vehicular Simulations).

> [!NOTE]
> **Roadmap Status: 100% Complete & Archived (All 10 Bundles Delivered)**
> All 10 foundational OpenSpec groundwork bundles have been successfully implemented, quality-gate verified (`cargo fmt`, `cargo clippy`, `cargo test`), merged into `main`, and archived under `openspec/changes/archive/` with their canonical specifications synced to `openspec/specs/`.
>
> There are currently **no pending or active groundwork bundles** under `openspec/changes/`.
> For new feature proposals or enhancements, follow the OpenSpec proposal lifecycle via the [`openspec-pro-workflow`](../.agents/skills/openspec-pro-workflow/SKILL.md) or [`openspec-propose`](../.agents/skills/openspec-propose/SKILL.md) skills.

---

## Execution Rules for Future Bundles & Changes

1. **Check for Active Bundles**:
   - Check `openspec/changes/`. If new groundwork bundles have been proposed, execute them in dependency order following [`openspec-bundle-executor`](../.agents/skills/openspec-bundle-executor/SKILL.md).
   - If `openspec/changes/` contains only `archive/`, new work must first be proposed via `openspec-propose` or `openspec-pro-workflow`.
2. **Never Skip or Stray from `design.md` and `tasks.md`**:
   - Each OpenSpec bundle is architected against exact file paths and line numbers. Read `proposal.md`, `specs/`, `design.md`, and `tasks.md` in full before editing any code.
3. **Dedicated Feature Branches**:
   - Create a dedicated feature branch `feat/<bundle-name>` from latest `origin/main`. Never commit directly to `main`.
4. **Strict Quality Gates Before Every PR**:
   - `cargo fmt --all -- --check`
   - `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings allowed)
   - `cargo test --workspace`
5. **Sync Specs & Archive on Completion**:
   - Copy/merge `openspec/changes/<bundle-name>/specs/.../spec.md` into `openspec/specs/.../spec.md` and `git mv openspec/changes/<bundle-name> openspec/changes/archive/YYYY-MM-DD-<bundle-name>` (either in the implementation PR or immediately upon merge).

---

## Delivered Groundwork Bundles

### Phase 1: Core Runtime, Director/Gateway, Gameplay & Client-WASM Bug Fixes (P0)

| Order | OpenSpec Bundle Directory | GitHub Issue(s) | PR | Status | Target Crates | Summary of Scope |
| :---: | :--- | :---: | :---: | :---: | :--- | :--- |
| **1** | [`director-gateway-failover-and-routing-fixes`](changes/archive/2026-10-17-director-gateway-failover-and-routing-fixes/) | #102 | [#116](https://github.com/Jakousa/rust-server/pull/116) | Completed | `mmo-director`, `mmo-gateway` | Resolve dead/new node `listen_addr` from `NodeRegistry` on failover & deregistration; wire `reap_idle_instances` & `check_and_recover_stale_gateways` in `director/src/main.rs`; read `REALM_ID`/`FEDERATION_HMAC_SECRET` from env; 3-axis bidirectional Octree boundary smoothing with parent `Branch::bounds` union updates; QUIC datagram `Ping` + `intercept_handler` & safe `active_send_stream` handling. |
| **2** | [`gameplay-trade-ability-and-wasm-fixes`](changes/archive/2026-10-17-gameplay-trade-ability-and-wasm-fixes/) | #103 | [#116](https://github.com/Jakousa/rust-server/pull/116) | Completed | `mmo-gameplay` | Fix `TradeManager::execute_atomic_swap` item ownership verification, giver item removal, weight checks, and currency settlement; pre-validate `ability.range` and `ActionQueue` busy state before deducting mana/cooldowns; enforce single-guild membership in `GuildManager`; sync `Octree`, `AttributeSet`, and `EntityDiedEvent` in WASM `host_api.rs`; wire `EncumbranceHookHandler` and `DamageProcessingSystem`. |
| **3** | [`client-wasm-and-voxel-persistence`](changes/archive/2026-10-17-client-wasm-and-voxel-persistence/) | #104 | [#117](https://github.com/Jakousa/rust-server/pull/117) | Completed | `mmo-client-wasm`, `mmo-plugin-voxel`, `web/` | Fix 64-bit GUID and object ID `f32` mantissa truncation in WASM render buffers; clear buffer and return `Err` on malformed frames in `process_packet`; prevent keyframe deltas from wiping in-flight client prediction; deduplicate RLE & mesh helpers; wire `VoxelTickSystem` to `VoxelWriteBehindService`, `EventQueue`, and `ExcavationRateLimiter` pruning. |

---

### Phase 2: Storage & Core Modularity, Deduplication & High-Frequency Performance (P1)

| Order | OpenSpec Bundle Directory | GitHub Issue(s) | PR | Status | Target Crates | Summary of Scope |
| :---: | :--- | :---: | :---: | :---: | :--- | :--- |
| **4** | [`storage-dialect-deduplication`](changes/archive/2026-10-17-storage-dialect-deduplication/) | #98 | [#118](https://github.com/Jakousa/rust-server/pull/118) | Completed | `mmo-storage` | Consolidate ~1,500 lines of duplicated SQLite vs. PostgreSQL methods across `escrow_coord.rs`, `marketplace.rs`, `transaction_coord.rs`, and `repository.rs` using `with_dialect_tx!`, `Database::sql`, `compute_free_slots`, and shared time helpers; propagate snapshot errors; gate `test_helpers`. |
| **5** | [`node-gateway-core-modularity-and-perf`](changes/archive/2026-10-18-node-gateway-core-modularity-and-perf/) | #100, #101, #105 | [#119](https://github.com/Jakousa/rust-server/pull/119) | Completed | `mmo-node`, `mmo-gateway`, `mmo-core`, `mmo-schema`, `mmo-loadtest` | Split `network.rs` and `server.rs` god modules; add `GrpcClientPool` (`DashMap`) and MPSC client input queue in `mmo-node`; unify TCP/QUIC/WebTransport auth handshake; unify single/batch migration; consolidate Ray-AABB slab math onto `BoundingBox3D` and enforce `collision_layers`; bucket `EventQueue` by `TypeId`; eliminate per-tick `Vec` allocations in `AttributeSet::tick` and `SpatialHistoryBuffer::push_frame`; wire `BotSession` sequencing/acks and non-recursive `TrajectoryController` in `mmo-loadtest`; harden `u16` string casts in `mmo-schema`. |

---

### Phase 3: The "100% Genre-Agnostic MMO Substrate" Primitives (P1/P2)

| Order | OpenSpec Bundle Directory | GitHub Issue(s) | PR | Status | Target Crates | Summary of Scope |
| :---: | :--- | :---: | :---: | :---: | :--- | :--- |
| **6** | [`genre-agnostic-core-and-indexed-attributes`](changes/archive/2026-10-18-genre-agnostic-core-and-indexed-attributes/) | #96 | [#120](https://github.com/Jakousa/rust-server/pull/120) | Completed | `mmo-core`, `mmo-node`, `mmo-schema`, `mmo-gameplay` | Sever `mmo-gameplay` dependency from `mmo-node` `[dependencies]`; introduce `AttributeId(u8)` fast-indexed attributes, `Entity::state_flags`, `ScriptReloadable` trait, and generalized `CooldownTracker` / `ActionDefinition` in `mmo-core`; support extensible `InteractionType::Custom(u8)` in `mmo-schema`. |
| **7** | [`bitwise-visibility-phasing-and-stealth`](changes/archive/2026-10-18-bitwise-visibility-phasing-and-stealth/) | #110 | [#121](https://github.com/Jakousa/rust-server/pull/121) | Completed | `mmo-core`, `mmo-node` | Add `phase_mask: u64`, `visibility_mask: u64`, `stealth_level: u8`, and `detection_level: u8` (`Entity::can_observe`) to `Entity` and `WorldObject`; filter AoI candidates prior to density throttling and emit immediate despawn packets when an entity enters stealth or shifts phase. |
| **8** | [`pluggable-kinematics-and-hierarchy`](changes/archive/2026-10-18-pluggable-kinematics-and-hierarchy/) | #111 | [#122](https://github.com/Jakousa/rust-server/pull/122) | Completed | `mmo-core`, `mmo-node` | Add pluggable `MotionIntegrator` (`DiscreteGround` and `Newtonian6DoF`), volumetric `CollisionWorld::sweep_test` during movement validation, server-authorized `ImpulseAllowance` for knockbacks/dashes, `parent_guid` relative transform hierarchies (`attach_child`, `propagate_hierarchy_transforms`) for moving vehicles/ships, and atomic parent-child group migration. |
| **9** | [`spatial-navgrid-pathfinding`](changes/archive/2026-10-18-spatial-navgrid-pathfinding/) | #112 | [#123](https://github.com/Jakousa/rust-server/pull/123) | Completed | `mmo-core`, `mmo-node`, `mmo-gameplay` | Replace single-corner fallback in `AabbCollisionWorld::find_path` with bounded 3D/2.5D A* search and line-of-sight string-pulling; expose `collision_world` on `TickContext`; wire `FsmBrainSystem` to navigate NPCs around multi-wall obstacles. |
| **10** | [`character-lifecycle-and-e2e-cluster-suite`](changes/archive/2026-10-18-character-lifecycle-and-e2e-cluster-suite/) | #81, #85 | [#124](https://github.com/Jakousa/rust-server/pull/124) | Completed | `mmo-node`, `mmo-gateway`, `mmo-loadtest`, `k8s/` | Implement `hydrate_player_session` (restoring saved position, rotation, and components on login) and `disconnect_player_session` (capturing final snapshot and despawning on logout); convert `k8s/base/postgres.yaml` to `StatefulSet` with PVC and harden PodSecurity + PDBs across `k8s/base/`; add declarative `ScenarioRunner` (`--scenario`) in `mmo-loadtest`. |
