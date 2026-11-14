# Delta: Gateway Routing Specification

## ADDED Requirements

### Requirement: Topology-Aware Partition Session Selection
The Edge Gateway SHALL synchronize spatial partition assignments from `mmo-director` heartbeats and select initial target simulation nodes using active partition topology when available.

#### Scenario: Gateway synchronizes partition assignments from Director heartbeat
- **GIVEN** `mmo-gateway` is connected to `mmo-director`
- **WHEN** `GatewayHeartbeatResponse` includes updated `topology_epoch` and `partitions`
- **THEN** `SessionRouter` SHALL update its internal partition table and route new sessions according to the active partition topology rather than pinning all sessions to a single static default node.
