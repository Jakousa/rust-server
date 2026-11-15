# Cloud PoC Deployment & Server Sizing Guide

This document defines the server hardware requirements, network firewall rules, and deployment options for running the **Playable Village & Barter Vertical Slice PoC** on a public cloud VM so players can connect directly from a web browser over WebTransport (HTTP/3).

---

## 1. Recommended Cloud VM Sizing

Because the MMO engine is written in Rust and runs a deterministic 20 Hz (`50ms`) simulation tick, runtime CPU and memory overhead are low. For the full stack (**k3s/Docker + PostgreSQL + Redis + Prometheus/Grafana**), choose a tier based on whether you also compile Docker images directly on the VM:

| Use Case | vCPU | RAM | Storage (NVMe SSD) | Example Instance (Hetzner / DigitalOcean / AWS) | Est. Cost |
| :--- | :---: | :---: | :---: | :--- | :---: |
| **Minimal PoC** (1–30 players + lightweight stack) | **2 vCPU** | **4 GB** | 40 GB NVMe | Hetzner **CX22 / CPX21** or DO Basic 4GB | ~$5–$9 / mo |
| **Recommended PoC + Loadtest** (50–500 bots/players, k3s + Grafana) | **4 vCPU** | **8 GB** | 80 GB NVMe | Hetzner **CPX31 / CAX21** or AWS `t3.large` | ~$9–$16 / mo |
| **Stress Test & On-Host Build** (compiling release Rust images on the VM) | **8 vCPU** | **16 GB** | 160 GB NVMe | Hetzner **CPX41 / CCX23** | ~$18–$32 / mo |

> **Tip:** If you run `docker build` / `cargo build --release` directly on the cloud VM rather than pulling pre-built images from CI, use at least **4 vCPU / 8 GB RAM** (or configure a 4 GB swap file) so `rustc` and link-time optimization (`lto`) do not hit out-of-memory errors.

---

## 2. Operating System & Network Requirements (Critical)

1. **Operating System**:
   - **Ubuntu 24.04 LTS** or **Debian 12 (Bookworm)** (`x86_64` or `ARM64` — the workspace compiles cleanly on both architectures).
2. **Public IPv4 Address & Firewall / Security Group Rules**:
   Browser clients connect via **WebTransport (HTTP/3 over QUIC)**, which runs over **UDP**. Ensure your cloud provider's firewall allows:
   - **`UDP 4433`**: `mmo-gateway` WebTransport endpoint (browser game traffic: movement, entity deltas, barter, and trade).
   - **`TCP 8443`**: `mmo-gateway` WebSocket fallback listener (`WEBSOCKET_ADDR`, automatic browser fallback when UDP/HTTP/3 is blocked).
   - **`TCP 8080`**: `mmo-gateway` TCP listener (`mmo-loadtest` synthetic bot traffic and TCP clients).
   - **`TCP 80` / `TCP 443` (or `TCP 3000`)**: `web/` static frontend asset serving (Vite / Nginx / Caddy) and `/cert-hash` discovery.
   - **`TCP 22`**: SSH administration.
   - *(Do **not** expose internal gRPC ports `50050`, `50051`, `50052` or PostgreSQL `5432` to the public internet — keep them inside the private container/cluster network).*

3. **Linux Kernel UDP Buffer Tuning (`/etc/sysctl.d/99-mmo-quic.conf`)**:
   QUIC / WebTransport benefits from larger kernel UDP receive and send buffers:
   ```ini
   net.core.rmem_max = 26214400
   net.core.wmem_max = 26214400
   net.core.rmem_default = 2097152
   net.core.wmem_default = 2097152
   ```
   Apply with: `sudo sysctl --system`.

---

## 3. How WebTransport TLS Works in a Domainless PoC

WebTransport mandates TLS 1.3, but **a custom domain or Let's Encrypt certificate is not required for the PoC**:
1. On startup, `mmo-gateway` automatically generates a W3C WebTransport-compliant short-lived (< 14 days) self-signed ECDSA certificate (including SAN entries for the server IP/hostname) and computes its SHA-256 fingerprint (`serverCertificateHashes`).
2. The gateway exposes this digest at `GET /cert-hash` (with CORS enabled).
3. When a player opens the browser client (`http://<SERVER_IP>:3000`), the frontend fetches `/cert-hash` automatically and establishes a pinned WebTransport session to `https://<SERVER_IP>:4433`.

---

## 4. Running the Stack on the Cloud VM

### Option A: Single-Command Docker Compose (Fastest PoC Setup)
Ideal for a single VM using pre-built images from Docker Hub (`jakousa/mmo-server` and `jakousa/mmo-web`):
```bash
docker compose pull
docker compose up -d
```
The stack launches:
- `postgres:16-alpine` (durable storage for characters and inventories)
- `mmo-director` (cluster topology coordinator via `jakousa/mmo-server`)
- `mmo-village-node` (authoritative village simulation, 3 NPC villagers, village structures, and barter/trade logic via `jakousa/mmo-server`)
- `mmo-gateway` (WebTransport UDP `:4433` + TCP `:8080` + `/cert-hash` `:9091` via `jakousa/mmo-server`)
- `mmo-web` (Nginx serving the compiled `web/dist` browser client on `:80` / `:3000` via `jakousa/mmo-web`)

### Option B: Cloud Kubernetes Cluster (`k8s/overlays/cloud`)
To run the full stack on a public k3s/Kubernetes VM using pre-built images from Docker Hub (`jakousa/mmo-server` and `jakousa/mmo-web`):
```bash
kubectl apply -k k8s/overlays/cloud
```
The overlay orchestrates:
- `mmo-director`, `mmo-gateway`, and `mmo-node` (bootstrapping `mmo-village-node` on Node 1 for authoritative village NPCs and structures)
- `postgres:16-alpine` with durable PVC persistence
- `mmo-web` frontend served on HTTP port `:80` (via Traefik Ingress) and port `:3000` (LoadBalancer)
- Prometheus (`:9090`) and Grafana (`:3001`, `admin`/`admin`) with the pre-provisioned `mmo-overview` dashboard

### Option C: Local K3d Development Cluster (`k8s/overlays/local`)
To run the multi-pod Kubernetes topology locally using `k3d` and locally built Docker images:
- Run `./scripts/cluster.sh up` (or `./scripts/cluster.sh all`).

---

## 5. Live Cloud Load & Soak Validation Benchmarks

Live validation was conducted against the deployed `k3s` cluster (`49.13.149.126` / `rust-server.kousa.dev`, container image `jakousa/mmo-server:sha-0571bd5`) running `mmo-director`, `mmo-gateway`, `mmo-node-0` (node `1`), `mmo-node-1` (node `2`), `postgres-0`, `prometheus`, and `grafana`.

### Ingress & Transport Endpoint Verification
- **TCP Game Protocol (`49.13.149.126:8080`)**: Active (`service/mmo-gateway` `8080:30286/TCP`) and sustained 25-, 65-, and 120-bot `mmo-loadtest` swarms with `0` connection errors and `0` authentication failures.
- **WebTransport / HTTP/3 Certificate Hash (`https://rust-server.kousa.dev/cert-hash`)**: Returned HTTP `200` with `{"cert_hash":"1d:f0:d6:89:82:51:04:f2:64:14:b3:51:1f:6d:4e:db:a8:ff:b0:28:b4:a4:d2:6a:03:ca:22:bf:07:8a:1c:40"}`, matching the SHA-256 fingerprint logged by `mmo-gateway`; `49.13.149.126:4433/UDP` is exposed on `service/mmo-gateway` (`4433:30337/UDP`) and bound by `mmo-gateway`.
- **WebSocket Fallback Gap (`:8443`)**: Although `mmo-gateway` binds `0.0.0.0:8443 (WS/TCP)` inside the pod, port `8443` is not exposed in `service/mmo-gateway` (`4433/UDP, 8080/UDP, 8080/TCP, 9091/TCP, 50050/TCP`) or `ingress/mmo-web` (which routes `/cert-hash` -> `mmo-gateway:9091` and `/` -> `mmo-web:3000`).

### Live `mmo-loadtest` Swarm Benchmarks (`49.13.149.126:8080`, 20 Hz, `RandomWalk`, max speed `9.5 m/s`)

| Swarm Size | Duration | Auth Rate (sess/s) | Sent (pkts/s) | Recv (pkts/s) | Recv Bandwidth | RTT p50 | RTT p95 | RTT p99 | RTT Max | Snapbacks | Conn / Auth Errors |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **25 bots** (`10s`) | `10.29 s` | `25` (`2.4/s`) | `5,243` (`509.7/s`) | `100,361` (`9,756.2/s`) | `5,277.78 kbps` (`6,786,517 B`) | `133.50 ms` | `182.66 ms` | `218.50 ms` | `233.98 ms` | `554` | `0` / `0` |
| **65 bots** (`25s`, `stagger-ms 15`) | `26.05 s` | `65` (`2.5/s`) | `34,629` (`1,329.1/s`) | `902,734` (`34,647.7/s`) | `18,743.84 kbps` (`61,045,581 B`) | `139.26 ms` | `216.83 ms` | `253.95 ms` | `260.86 ms` | `2,220` | `0` / `0` |
| **120 bots** (`20s`, `stagger-ms 10`) | `21.47 s` | `120` (`5.6/s`) | `50,676` (`2,360.5/s`) | `1,885,455` (`87,824.2/s`) | `47,496.82 kbps` (`127,460,810 B`) | `141.18 ms` | `207.10 ms` | `289.79 ms` | `420.86 ms` | `4,956` | `0` / `0` |

### Cluster Behavior, Bottlenecks & Findings
- **Elastic Spatial Partitioning & Cross-Node Migration**: Exceeding the node entity split threshold triggered `mmo-director` octree splits (`mmo_director_topology_epoch` incremented `2` -> `4` after the 65-bot run and `4` -> `6` after the 120-bot run; `mmo-node-1` logged `[Node 2] Split reported, assigned nodes: [2, 1, ...]`). Across the 65- and 120-bot runs, cumulative successful batch migrations reached `69` (`mmo-node-1` -> `mmo-node-0`) and `44` (`mmo-node-0` -> `mmo-node-1`), while `mmo_gateway_batch_route_updates_total` reached `97` (`mmo_gateway_route_updates_total = 156`).
- **Simulation Tick Budget & `ObserverConfig` Delta Compression Bottleneck**:
  - At **65 bots**, both nodes stayed comfortably within the `50 ms` (20 Hz) frame budget (`mmo-node-0` p50 `0.0530 ms`, p95 `1.0694 ms`, p99 `2.1095 ms`, max `9.6449 ms`; `mmo-node-1` p50 `0.0917 ms`, p95 `4.9921 ms`, p99 `8.5716 ms`, max `13.9578 ms`).
  - At **120 bots**, because `ObserverConfig::default().enable_delta_compression` is `false` in `mmo-node`, live village nodes broadcast individual uncompressed `EntityState` (`0x0005`) frames rather than `EntityDeltaBatch` (`0x0009`) frames (`Entity Deltas Recv: 0`). Combined with co-located bot spawning around `(0, 0, 5)`, downstream traffic grew quadratically to `87,824.2 pkts/s` (`47,496.82 kbps`), pushing `mmo-node-1` p90/p99/max tick duration to `55.52 ms` (above the `50 ms` 20 Hz budget), while `mmo-node-0` stayed at `1.06 ms` p99 (`4.56 ms` max).
  - Because `crates/loadtest` counts route handovers via `EntityDeltaBatch.server_tick` transitions across nodes, `Route Handovers` reads `0` when a target cluster emits legacy `EntityState` packets; cluster handovers were confirmed directly via Prometheus migration/route counters and end-to-end via the `cloud_soak_validation` integration suite with delta compression enabled.
- **Expected Snapbacks & Persistence Throughput**: Bots spawn at `(0, 0, 0)` (intersecting the Village Well collider while `mmo-village-node` defaults player spawns to `(0, 0, 5)`) and execute unconstrained 3D `RandomWalk` steps, triggering expected authoritative movement corrections. Write-behind persistence drained cleanly (`mmo_node_snapshot_dirty_queue = 0` on both nodes post-run, flush p50 `4.09 ms` / `4.43 ms` and p99 `12.22 ms` / `13.46 ms`).

### Deterministic Multi-Node Split, Migration, Failover & Persistence Test Suite
- `cargo test -p mmo-loadtest --test cloud_soak_validation` validates concurrent JWT bot swarms against a local `GatewayServer` and multi-node `TickSimulation` topology with `enable_delta_compression = true`, exercising `execute_batch_entity_migration`, `GatewayService::reassign_node_routes` failover, client-observed route handovers, and `WriteBehindSnapshotter` recovery via `sqlite::memory:` (`2 passed; 0 failed` in `2.31s`).

