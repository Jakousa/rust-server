## 1. Load Test Crate Foundation (`crates/loadtest`)

- [x] 1.1 Initialize `crates/loadtest` in the Cargo workspace with dependencies (`tokio`, `clap`, `bytes`, `hdrhistogram`, `rand`, `mmo-schema`, `mmo-core`), and verify with `cargo check -p mmo-loadtest`
- [x] 1.2 Implement virtual client connection session: establish TCP connection to Gateway, frame `ClientPacket` binary wire packets (movement, ping/pong), and decode incoming server frames via `ClientPacket::decode_frame`, and verify via unit tests
- [x] 1.3 Implement 3D kinematic trajectory generator with wander AI and boundary plane crossing trajectories within configured world bounds, and verify with unit tests
- [x] 1.4 Implement SwarmManager and telemetry aggregator tracking p50/p95/p99 latency, packet throughput, and snap-back correction counts, and verify via automated swarm test

## 2. Cluster Entrypoints, Containerization & Topology

- [x] 2.1 Implement full production daemon entrypoint in `crates/node/src/main.rs`: start gRPC worker service (`SimulationWorkerServer`), register with Director, run periodic heartbeat loop, and execute the 20 Hz simulation tick loop (configurable via environment variables `NODE_ID`, `LISTEN_ADDR`, `DIRECTOR_ADDR`)
- [x] 2.2 Implement full production daemon entrypoint in `crates/gateway/src/main.rs`: accept incoming TCP client connections, dispatch packets, and handle dynamic session routes (configurable via `LISTEN_ADDR`, `DIRECTOR_ADDR`)
- [x] 2.3 Implement multi-stage `Dockerfile` with cargo build caching and lean Debian slim runtime packaging all 4 service binaries, and verify image compilation via `docker build`
- [x] 2.4 Create `docker-compose.yml` declaring `mmo-net` bridge network, `director`, `node-1`, `node-2`, `gateway`, and on-demand `loadtest` profile wired with their respective container environment variables, and verify container startup via `docker compose up -d`
- [x] 2.5 Execute full containerized stress run via `docker compose run --rm loadtest` simulating virtual bots crossing octant boundaries, and verify telemetry reports zero drops and clean cluster health
