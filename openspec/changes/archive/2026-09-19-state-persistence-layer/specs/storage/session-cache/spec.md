## Purpose

Provides low-latency distributed caching for active sessions, player presence, and fast entity state migration during node handoffs.

## ADDED Requirements

### Requirement: Distributed Session Presence
The session cache SHALL store active session tokens, mapping session IDs to player GUIDs and current authoritative node addresses with automatic TTL lease renewals.

#### Scenario: Session registration and lookup
- **WHEN** an Edge Gateway establishes an authenticated client session
- **THEN** it SHALL register a session entry in the distributed cache with a configurable expiration lease

### Requirement: Fast Handover State Transfer
When an entity migrates across an octree boundary, the source node SHALL stage serialized component state into the cache for atomic retrieval by the target node.

#### Scenario: Staging migration payload
- **WHEN** an entity crosses into a neighboring node's octant
- **THEN** the source node SHALL serialize entity components into the handover cache and signal the target node to acquire the entity state
