# Tasks: Geo-Distributed Edge Gateways & Anycast Routing

## Phase 1: Protobuf Schema & Cluster RPCs (`mmo-schema`)
- [x] 1.1 In `crates/schema/proto/cluster.proto`, define `RegisterGatewayRequest`, `RegisterGatewayResponse`, `GatewayHeartbeatRequest`, and `GatewayHeartbeatResponse`. *(Eligible for local subagent delegation)*
- [x] 1.2 In `crates/schema/proto/cluster.proto`, define `TakeoverSessionRouteRequest`, `TakeoverSessionRouteResponse`, `RevokeGatewaySessionRequest`, `RevokeGatewaySessionResponse`, and `MultiGatewayRouteBroadcastRequest`. *(Eligible for local subagent delegation)*
- [x] 1.3 Add RPC declarations to `DirectorService`, `NodeService`, and `GatewayService` in `crates/schema/proto/cluster.proto`. *(Eligible for local subagent delegation)*
- [x] 1.4 Recompile protobufs and verify generated code in `crates/schema`. *(Eligible for local subagent delegation)*

## Phase 2: Gateway Identity & Director Gateway Registry (`mmo-director`, `mmo-gateway`)
- [x] 2.1 In `crates/director/src/registry.rs`, implement `GatewayDescriptor` and `GatewayRegistry` tracking active gateways, regions, capacities, and heartbeats.
- [x] 2.2 In `crates/director/src/service.rs`, implement `RegisterGateway` and `GatewayHeartbeat` handlers with stale gateway reaper task (3.5s timeout).
- [x] 2.3 In `crates/gateway/src/server.rs`, implement gateway heartbeat client loop to periodically ping Director.
- [x] 2.4 Write unit tests in `crates/director` verifying gateway registration, heartbeat updates, and stale timeout transitions. *(Eligible for local subagent delegation)*

## Phase 3: Anycast Latency & Health Probing with Reflection Defense (`mmo-gateway`, `mmo-core`)
- [x] 3.1 In `crates/gateway/src/probe.rs`, implement zero-allocation UDP probe server enforcing 64-byte request padding to eliminate DDoS reflection amplification. *(Eligible for local subagent delegation)*
- [x] 3.2 Wire probe listener into `crates/gateway/src/main.rs` with configurable probe port (`PROBE_ADDR`).
- [x] 3.3 Write unit tests in `crates/gateway` validating probe request parsing, 64-byte padding enforcement, response format, and concurrent probe load handling. *(Eligible for local subagent delegation)*

## Phase 4: Persistent Backhaul Multiplexing (`mmo-gateway`, `mmo-node`)
- [x] 4.1 In `crates/gateway/src/backhaul.rs`, implement `BackhaulPool` managing persistent, multiplexed QUIC data-plane streams and gRPC control channels with keepalive pings.
- [x] 4.2 In `crates/gateway/src/server.rs`, integrate `BackhaulPool` into `PacketDispatcher` for non-blocking packet forwarding across persistent pooled streams.
- [x] 4.3 Add circuit breaker and backpressure handling when a simulation node backhaul link experiences packet loss or timeouts.
- [x] 4.4 Write integration tests in `crates/gateway` verifying backhaul multiplexing, channel pooling, and failover behavior. *(Eligible for local subagent delegation)*

## Phase 5: Cross-Gateway Handover & Takeover Protocol (`mmo-gateway`, `mmo-node`, `mmo-storage`)
- [x] 5.1 In `crates/gateway/src/handover.rs`, implement `HandoverTicket` creation with HMAC-SHA256 signing, constant-time verification (`subtle::ConstantTimeEq`), and monotonic session epoch validation. *(Eligible for local subagent delegation)*
- [x] 5.2 Implement token-bucket rate limiter (1 op per 5s, burst 2) in `crates/gateway/src/handover.rs` to protect against Anycast BGP route flapping and cache contention.
- [x] 5.3 In `crates/node/src/network.rs`, implement `TakeoverSessionRoute` handler in `NodeService` to switch an entity's downstream egress sink to the new gateway and enforce upstream packet epoch validation (dropping frames where `packet.epoch < node.session_epoch`).
- [x] 5.4 In `crates/gateway/src/server.rs`, implement `RevokeGatewaySession` handler in `GatewayService` to cleanly close old local client sockets without triggering entity despawn, and implement cache CAS rollback on node RPC timeout.
- [x] 5.5 Write end-to-end integration tests validating roaming handover between two gateways without entity disruption, upstream stale packet drop on old gateway delay, and rollback on node timeout.

## Phase 6: End-to-End Verification & Quality Gates
- [x] 6.1 Execute full test suite `cargo test --workspace`.
- [x] 6.2 Execute linter checks `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 6.3 Execute code formatting checks `cargo fmt --all -- --check`.
- [x] 6.4 Verify multi-gateway topology and roaming handover in the local cluster environment (`scripts/cluster.sh`).
