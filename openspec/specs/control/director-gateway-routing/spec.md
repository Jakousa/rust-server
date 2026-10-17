# Director & Gateway Failover, Spatial Smoothing, and QUIC Interceptor Specification

## Purpose
Guarantees accurate node address resolution during failover, active background reaping of stale gateways and idle instances, mathematically consistent 3-axis octree boundary smoothing, and uniform QUIC datagram/stream interception in the Edge Gateway.

## Requirements

### Requirement: Registered Node Address Resolution on Failover and Deregistration
When a simulation node fails heartbeat checks or gracefully deregisters, the Director SHALL resolve both the departed node's `listen_addr` and the recipient node's `listen_addr` from `NodeRegistry` and broadcast `ReassignNodeRoutesRequest` to all active gateways.

#### Scenario: Dead node with custom registered listen address fails over
- **GIVEN** Node 1 registered with `listen_addr = "10.42.0.15:50052"` and Node 2 registered with `listen_addr = "10.42.0.16:50052"`
- **WHEN** Node 1 exceeds the heartbeat timeout and its partition is reassigned to Node 2
- **THEN** the Director SHALL emit a `PartitionRecoveryEvent` with `previous_node_address = "10.42.0.15:50052"` and `new_node_address = "10.42.0.16:50052"`
- **AND** broadcast `ReassignNodeRoutesRequest` containing those exact addresses to all active gateways in `GatewayRegistry`

### Requirement: Periodic Instance and Gateway Reaping & Environment Federation Config
The Director daemon SHALL periodically reap idle dynamic instances and stale gateways in its background maintenance loop, and initialize `RealmFederationAuthority` from `REALM_ID` and `FEDERATION_HMAC_SECRET` environment variables when present.

#### Scenario: Environment-configured federation authority
- **GIVEN** `REALM_ID="realm-eu-1"` and `FEDERATION_HMAC_SECRET="super-secret-federation-key-32b"` set in the environment
- **WHEN** `DirectorServiceImpl::new` initializes
- **THEN** its `RealmFederationAuthority` SHALL sign and verify tokens using `"realm-eu-1"` and the configured secret key

### Requirement: 3-Axis Bidirectional Octree Boundary Smoothing and Branch Bounds Consistency
`GlobalOctreeCoordinator::smooth_adjacent_boundaries` SHALL support boundary shifts along the X, Y, and Z axes in either neighbor order (`ba.max == bb.min` or `bb.max == ba.min`), and `GlobalOctreeNode::update_leaf_bounds` SHALL keep parent `Branch::bounds` synchronized to the bounding union of its children so `find_node` routes all coordinates inside shifted leaves accurately.

#### Scenario: Boundary shift across Y/Z axes and reversed neighbor order
- **GIVEN** two adjacent leaf partitions sharing a Y-axis or Z-axis face (or where the primary partition has higher coordinates than the neighbor) with a $\ge 2:1$ entity load imbalance
- **WHEN** `smooth_adjacent_boundaries` evaluates the pair and `update_leaf_bounds` applies the shifted bounds
- **THEN** `GlobalOctreeNode::find_node(pos)` for any point inside the newly expanded strip SHALL return the expanded leaf's `node_id`

### Requirement: QUIC Datagram Interception and Non-Disruptive Bidirectional Streams
The Edge Gateway's QUIC datagram loop SHALL short-circuit `ClientPacket::Ping` after replying with `Pong` and invoke `GatewayInterceptHandler::handle_packet` before forwarding packets to simulation nodes. Accepting a new bidirectional QUIC stream SHALL NOT overwrite an already-active `active_send_stream` or hijack downstream delivery before authentication.

#### Scenario: QUIC datagram Ping and intercepted social packet
- **GIVEN** an authenticated QUIC client session
- **WHEN** the client sends a `Ping` or an intercepted social/marketplace packet via QUIC datagram
- **THEN** the Gateway SHALL handle the packet locally via `Pong` or `intercept_handler` and SHALL NOT dispatch it to the simulation node
