# Change Proposal: Track 4 Kubernetes Scale & Chaos Testing (10k Bot Ingestion & Failover)

## Context & Motivation
Following the completion of Track 1 (Observability & Telemetry), Track 2 (Dynamic Partitioning & Live Entity Migration), and Track 3 (Distributed PostgreSQL Storage & ACID Inventory), the MMO cluster possesses all architectural capabilities of a distributed game server.

To validate production readiness under real-world MMO workloads, the cluster must undergo large-scale bot ingestion stress testing (1,000 to 10,000 concurrent bot connections) and automated chaos resiliency testing within the local `k3d` Kubernetes cluster. We need empirical verification that:
1. The Edge Gateway can ingest high-throughput client traffic and maintain low RTT ($< 50\text{ ms}$) without packet loss.
2. The simulation nodes sustain the 20 Hz tick budget ($< 45\text{ ms}$ tick time) even under dense hotspot clustering where hundreds of entities congregate in a single spatial cell.
3. The cluster autonomously recovers when a simulation node abruptly crashes (`kill -9` / pod eviction), detecting node blackout within 5 seconds, adopting orphaned spatial partitions on surviving nodes, and maintaining uninterrupted gateway routing.
4. Telemetry is accessible via pre-provisioned Grafana dashboards visualizing tick percentiles, entity density, and gateway throughput out-of-the-box.

## Scope of Change
1. **Enhanced Load Test Engine (`crates/loadtest`)**:
   - Introduce configurable behavior profiles:
     - `RandomWalk`: Standard random exploration within arena boundaries.
     - `HotspotClustering`: Entities converge toward hotspot epicenters (e.g. $(0,0,0)$ town square) to stress AoI interest management and partition splitting.
     - `CombatInteractions`: Periodic interaction packets sent alongside movement updates.
   - Support Prometheus metric export from `mmo-loadtest` so bot performance is scraped by Prometheus.
2. **Containerized Kubernetes Bot Swarm**:
   - Provide Kubernetes Job / Deployment manifests (`k8s/base/loadtest.yaml`) capable of horizontal scale-out (multiple load generator pods) to simulate 1,000 to 10,000 bots without hitting single-pod client socket limits.
   - Configurable target gateway, bot counts, durations, and behavior profiles via environment variables and ConfigMaps.
3. **Automated Chaos Testing Suite (`scripts/chaos.sh`)**:
   - Automated chaos injection script simulating simulation node termination.
   - Verification of Director heartbeat expiration ($\le 5\text{s}$), orphaned partition leaf recovery, and dynamic route failover.
4. **Pre-Configured Grafana Production Dashboard**:
   - Automated dashboard provisioning in `k8s/overlays/local/grafana.yaml` with visual panels:
     - 20 Hz Tick Duration percentiles (p50, p95, p99) per node.
     - Active CCU and entity counts per node.
     - Gateway packet rates (in/out) and buffer drop counters.
     - Snapshot flush latency and database write performance.
5. **Quality Gates & Verification**:
   - Scaled bot ingestion run against the local cluster.
   - Chaos failover verification showing clean orphan partition recovery.
   - Clean `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.

## Non-Goals
- External public cloud deployment (AWS/GCP); testing is targeted at the local `k3d` Kubernetes cluster.
- Complex behavior trees for bots (simple spatial clustering, random walks, and interaction pings provide realistic throughput stress).
