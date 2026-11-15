# Tasks: Node Split Migration and Topology-Aware Routing (Issue #235)

- [x] **1. Protocol & Director Fixes (`crates/schema`, `crates/director`)**
  - [x] Add `PartitionAssignment` and response fields in `crates/schema/proto/cluster.proto`.
  - [x] Normalize unspecified listen addresses (`0.0.0.0`, `::`) in `fallback_node_addr`.
  - [x] Prevent standby registration theft in `GlobalOctreeNode::map_bounds` and split loops in `should_split_node` / `handle_split`.
  - [x] Populate `partitions` and `assigned_node_addresses` in `DirectorServiceImpl`.
  - [x] Add Director unit tests covering address normalization, standby registration, balanced split, and split-loop prevention.

- [x] **2. Node Split Execution, Live Migration & Village Bootstrap (`crates/node`, `crates/gameplay`, `k8s/`)**
  - [x] Add `owned_bounds`, `peer_partitions`, `apply_topology_partitions`, `collect_entities_for_peer_partitions`, and `drain_boundary_migrations_by_peer` to `TickSimulation`.
  - [x] Update `runner.rs` to apply split/heartbeat topology, invoke `execute_batch_entity_migration_pooled`, advertise routable addresses, and execute async boundary-crossing migrations.
  - [x] Extract `bootstrap_village_plugins_and_structures` in `crates/gameplay/src/village.rs`, update `mmo_village_node.rs` to only spawn starter NPCs on `NODE_ID == 1`, and update `k8s/base/node-statefulset.yaml`.

- [x] **3. Gateway Topology Routing & E2E Tests (`crates/gateway`, `crates/node/tests`)**
  - [x] Add `PartitionRoute` topology tracking, `resolve_node_for_position`, and `select_node_for_new_session` to `SessionRouter`.
  - [x] Sync Director heartbeat partitions into `SessionRouter` via `GatewayServer::spawn_heartbeat_loop_with_router` and `auth_handshake.rs`.
  - [x] Add unit and multi-node integration tests verifying split migration, Gateway route updates, and split-loop prevention.

- [x] **4. Quality Gates & PR Submission**
  - [x] Run `cargo fmt --all -- --check`
  - [x] Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] Run `cargo test --workspace`
  - [x] Run `openspec validate --all`
  - [x] Review all changes, push feature branch, create PR with `Closes #235`, and enable squash auto-merge.
