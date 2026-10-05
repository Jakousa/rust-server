# Proposal: Geo-Distributed Edge Gateways & Anycast Routing

## Motivation & Background
Players connecting to the distributed virtual world are geographically dispersed across continents (Americas, Europe, Asia-Pacific). In the current single-region gateway topology:
1. Gateways assume local co-location with simulation clusters or single static route endpoints.
2. Simulation nodes only store a single optional `gateway_addr` for routing entity migrations, preventing dynamic multi-gateway notification across geographically separated points of presence (PoPs).
3. Client packets sent over high-latency WAN connections directly to centralized clusters suffer from elevated round-trip times (RTT), high packet loss, and jitter during TLS 1.3 / QUIC handshakes.
4. When a player's route shifts due to BGP Anycast convergence, cellular/Wi-Fi handover, or regional gateway failover, there is no cross-gateway session handover mechanism: the previous session drops, triggering entity despawn, or requires jarring re-authentication.

To achieve world-class low latency and carrier-grade edge availability, the server platform requires **Geo-Distributed Edge Gateways & Anycast Routing**.

---

## Architectural Objectives

1. **Geo-Distributed Gateway Ingress & Health Probing (`mmo-gateway`, `mmo-core`)**:
   - Assign each Edge Gateway instance a unique `gateway_id` and geographical `region` identifier (e.g. `us-east`, `eu-central`, `ap-southeast`).
   - Implement an Anycast Latency & Health Probe (UDP and HTTP ping responder) reporting real-time gateway load factor, capacity, and health status for Anycast/DNS latency steering.

2. **Director Gateway Registry & Global Topology (`mmo-director`, `mmo-schema`)**:
   - Introduce gateway lifecycle management in `cluster.proto`: `RegisterGatewayRequest`, `GatewayHeartbeatRequest`, and `DeregisterGatewayRequest`.
   - Maintain a live `GatewayRegistry` in the Director tracking active edge gateways, regional distribution, and capacity.
   - Support multi-gateway route broadcast so simulation nodes and directors can dispatch route invalidations and migrations to all relevant regional gateways.

3. **Persistent Backhaul Multiplexing & Connection Pooling (`mmo-gateway`, `mmo-node`)**:
   - Implement `BackhaulPool` in `crates/gateway` maintaining persistent, multiplexed QUIC/gRPC communication channels between edge gateways and regional simulation clusters.
   - Paced packet aggregation, framing, and channel reuse eliminating per-packet TCP/TLS connection overhead across the WAN backhaul.
   - Backhaul health monitoring tracking RTT, jitter, and automatic failover upon link degradation.

4. **Seamless Cross-Gateway Session Handover Protocol (`mmo-gateway`, `mmo-storage`, `mmo-schema`)**:
   - Issue cryptographically signed, constant-time verifiable `HandoverTicket` records to connected clients upon authentication.
   - Allow roaming clients to present their ticket to a new Edge Gateway upon network path re-route.
   - Execute an atomic 3-way handover:
     1. New Gateway validates ticket and claims session in `SessionCacheBackend` / Director.
     2. New Gateway notifies authoritative simulation node via gRPC (`TakeoverSessionRouteRequest`), updating downstream egress routing to the new gateway without dropping simulation state.
     3. Old Gateway receives revocation notice (`RevokeSessionRequest`) and terminates its local connection socket cleanly without sending an entity despawn signal.
   - Implement token-bucket rate limiting and replay defense via monotonic epoch validation to prevent session hijacking.

5. **Local Subagent Delegation**:
   - In accordance with `AGENTS.md`, delegate repetitive boilerplate (field definitions, getters, builders), protobuf message codecs, database/cache queries, and unit tests to the local Qwen coder model via MCP `ask_qwen`.

---

## Non-Goals
- BGP routing daemon implementations inside the Rust application (Anycast BGP is handled at the network/infrastructure layer; the engine provides probes, endpoints, and topology metadata).
- Hardcoding specific cloud provider edge services (e.g., Cloudflare Workers, AWS Global Accelerator); the architecture is pure Rust, protocol-driven, and vendor-neutral.
- Client-authoritative location overrides during gateway handovers.
