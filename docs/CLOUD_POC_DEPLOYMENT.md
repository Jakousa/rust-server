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
Ideal for a single VM (`docker compose up -d --build`):
- `postgres:16-alpine` (durable storage for characters and inventories)
- `mmo-director` (cluster topology coordinator)
- `mmo-village-node` (authoritative village simulation, 3 NPC villagers, village structures, and barter/trade logic)
- `mmo-gateway` (WebTransport UDP `:4433` + TCP `:8080` + `/cert-hash` `:9091`)
- `mmo-web` (Nginx/Vite serving the compiled `web/dist` browser client on `:80` / `:3000`)

### Option B: K3s / K3d Kubernetes (`k8s/overlays/local`)
To run the full multi-pod Kubernetes topology used by the chaos and e2e test suites (multiple `mmo-node` pods, Prometheus + Grafana dashboards, PodDisruptionBudgets):
- Run `./scripts/cluster.sh up` (or deploy to a single-node `k3s` installation on the VM).
