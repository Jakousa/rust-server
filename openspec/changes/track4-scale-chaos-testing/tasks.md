# Implementation Tasks: Track 4 Kubernetes Scale & Chaos Testing

## Task List

- [x] 1. **Bot Swarm Generator Enhancements (`crates/loadtest`)**
  - [x] 1.1 Add `BehaviorProfile` enum (`RandomWalk`, `HotspotClustering`, `CombatInteraction`) in `crates/loadtest/src/trajectory.rs`.
  - [x] 1.2 Implement hotspot steering calculation converging towards configurable epicenter `(x, y, z)` within radius $R$.
  - [x] 1.3 Add interaction packet generation support in `crates/loadtest/src/swarm.rs`.
  - [x] 1.4 Update CLI arguments in `crates/loadtest/src/main.rs` to expose `--behavior`, `--hotspot-x`, `--hotspot-y`, `--hotspot-z`, and `--hotspot-radius`.
  - [x] 1.5 Implement jitter-tolerant `compute_server_dt` with sequence-aware floor in `crates/node/src/sim.rs` to prevent false-positive speed-hack snapbacks during high load.
  - [x] 1.6 Verify with unit tests in `crates/loadtest` and `crates/node`.

- [x] 2. **Kubernetes Bot Swarm Manifests & Route Failover Propagation**
  - [x] 2.1 Create `k8s/base/loadtest.yaml` Job / Deployment manifest with configurable replicas, resources, and environment variables targeting `mmo-gateway:8080`.
  - [x] 2.2 Add `reassign_node_routes` on `SessionRouter` in `crates/gateway/src/router.rs` and wire Director orphan recovery notification to prevent traffic blackholing on node crash.
  - [x] 2.3 Add `loadtest.yaml` to `k8s/base/kustomization.yaml` (optional/commented or standalone resource).

- [x] 3. **Production Grafana Dashboards**
  - [x] 3.1 Create `k8s/overlays/local/dashboards/mmo-overview.json` with panels for 20 Hz tick duration percentiles (p50, p95, p99), active CCU per node, gateway packet rates, and buffer drops.
  - [x] 3.2 Update `k8s/overlays/local/grafana.yaml` with dashboard provider ConfigMap and dashboard definition ConfigMap mounting into Grafana.
  - [x] 3.3 Apply to cluster and verify dashboard loads in Grafana (`http://localhost:3000`).

- [x] 4. **Automated Chaos Testing Suite (`scripts/chaos.sh`)**
  - [x] 4.1 Write `scripts/chaos.sh` automating:
    - Pre-flight cluster readiness checks.
    - Scale ingestion bot launch.
    - Sudden node crash injection (`mmo-node-0` pod termination).
    - Polling Director logs for orphan partition recovery within 5 seconds.
    - Asserting StatefulSet resurrection of the node and successful re-registration.
  - [x] 4.2 Make `scripts/chaos.sh` executable and run against live cluster.

- [x] 5. **Scale Benchmark & Quality Gates**
  - [x] 5.1 Run 1,000+ bot scale load test and verify p99 tick times remain $< 45\text{ ms}$.
  - [x] 5.2 Run workspace tests: `cargo test --workspace`.
  - [x] 5.3 Run linter checks: `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 5.4 Update `ROADMAP.md` and commit/push changes to `origin/main`.
