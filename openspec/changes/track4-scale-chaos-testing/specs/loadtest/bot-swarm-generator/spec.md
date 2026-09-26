# Bot Swarm Load Generator Specification

## Purpose
Specifies configurable load simulation behaviors, metrics aggregation, and Kubernetes orchestration for high-density bot swarms generating client traffic against the MMO Edge Gateway.

## Requirements

### Requirement: Bot Behavior Profiles
The load generator MUST support selectable behavior profiles:
- `RandomWalk`: Smooth random Brownian-style motion bounded by arena dimensions.
- `HotspotClustering`: Bots steer toward a configurable epicenter $(X_c, Y_c, Z_c)$ within radius $R_{\text{hotspot}}$, clustering entity density in a small area of the world to test spatial partitioning and AoI broadcast limits.
- `CombatInteractions`: Bots periodically emit `ClientPacket::Interaction` packets (targeting nearby entities) interspersed with movement packets.

#### Scenario: Hotspot Clustering
- **GIVEN** a swarm configured with `--behavior hotspot --hotspot-radius 50.0`
- **WHEN** bots advance through simulation steps
- **THEN** their positions converge toward the epicenter $(0, 0, 0)$ within $50\text{ meters}$, creating an AoI density hotspot without triggering speed hack rejections.

### Requirement: Granular Latency & RTT Percentile Tracking
The load test engine MUST record packet round-trip time (RTT) for every ping/pong exchange and compute min, p50, p95, p99, and max latencies.

#### Scenario: RTT Metric Aggregation
- **GIVEN** 500 bots sending 1 Hz pings over 10 seconds
- **WHEN** the test completes
- **THEN** `MetricsSnapshot` outputs exact counts for sent/received packets, p50 RTT, p99 RTT, and server snapbacks.

### Requirement: Kubernetes Bot Ingestion Job Manifest
A Kubernetes manifest MUST allow spinning up headless bot worker pods targeting the in-cluster gateway (`http://mmo-gateway.mmo:8080`).

#### Scenario: Scaled Deployment
- **GIVEN** `k8s/base/loadtest.yaml` applied with `replicas: 2` and `BOT_COUNT: 500`
- **WHEN** the pods run
- **THEN** 1,000 total bots concurrently stream movement and ping packets to the gateway without pod crashes or socket starvation.
