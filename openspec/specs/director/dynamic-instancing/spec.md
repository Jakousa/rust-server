# Dynamic Instancing & Realm Federation Specification

## Purpose
Defines ephemeral instance domain provisioning, idle instance teardown, transparent gateway route handoffs, and HMAC-signed cross-cluster realm federation transfer claims.

## Requirements

### Requirement: Ephemeral Instance Domain Provisioning
The cluster director MUST allow dynamically allocating isolated simulation instance domains onto active simulation nodes based on node load or explicit target requests.

#### Scenario: Allocate isolated instance domain
- **GIVEN** a cluster with registered simulation nodes
- **WHEN** an instance allocation request is submitted with a template ID, unique instance ID, and custom bounding box
- **THEN** the Director selects the optimal simulation node (least loaded) or targets the specified node, registers the instance domain, and returns the assigned node address and instance metadata.

### Requirement: Instance Inactivity & Automatic Teardown
The director and simulation nodes MUST monitor instance occupancy and automatically reclaim resources when instances are vacant past an idle timeout threshold.

#### Scenario: Vacant instance idle timeout teardown
- **GIVEN** an active instance domain with zero active player entities
- **WHEN** the idle duration exceeds the configured `idle_timeout_seconds`
- **THEN** the simulation node initiates instance teardown, cleans up internal spatial indexes, and notifies the Director to unregister the instance domain.

### Requirement: Gateway Instance Route Handoff
Edge gateways MUST support transparent ingress routing transitions when players cross instance boundaries or enter portals.

#### Scenario: Atomic session route update to instance node
- **GIVEN** a connected player session routed to an open-world simulation node
- **WHEN** a route update arrives pointing the session to an instance node
- **THEN** the gateway updates its routing table atomically without closing the client socket or terminating the transport session.

### Requirement: Cross-Cluster Realm Federation & Transfer Claims
The cluster federation registry MUST issue cryptographically signed, short-lived transfer claim tickets authorizing cross-realm player migration.

#### Scenario: Signed cross-realm transfer claim issuance and verification
- **GIVEN** a player initiating a cross-realm transfer or cross-cluster party hop
- **WHEN** a transfer claim is requested from the realm federation authority
- **THEN** the Director signs an HMAC-SHA256 transfer claim containing source realm, target realm, player GUID, epoch, and expiration timestamp
- **AND** the destination realm verifies the signature, prevents replay attacks via nonce/epoch checks, and admits the player.
