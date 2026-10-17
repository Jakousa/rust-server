# Proposal: Dynamic Instancing, Transient Dungeons & Cross-Cluster Realm Federation

## Problem Statement
The cluster orchestrator (`crates/director`) currently manages a single continuous virtual world partitioned across persistent simulation nodes via an octree spatial domain. Modern MMO architectures require the ability to:
1. Dynamically provision isolated, transient simulation domains (instanced dungeons, battlegrounds, arena matches, private player sanctuaries) with custom bounding boxes, lifecycle management, and automatic reclamation when idle/empty.
2. Coordinate seamless ingress routing through edge gateways (`crates/gateway`) such that a player transfers between the open-world spatial partition and an instance domain without socket disconnection or client teardown.
3. Federate player identities and cross-realm party synchronization / transfer claims across distinct cluster realms via cryptographically verifiable transfer tickets issued by a ticket authority.

All mechanics must adhere strictly to the Prime Directive in `AGENTS.md`: unopinionated, genre-agnostic MMO primitives with zero hardcoded game-specific mechanics in core crates.

## Scope
- **Domain Allocation & Lifecycle**: Director provisions instance domains identified by unique instance GUIDs (`u64`), mapped to target simulation nodes, with configurable idle timeouts and automated resource reclamation upon player evacuation.
- **Instance Simulation Domains in Node**: `TickSimulation` supports hosting both open-world sectors and isolated instance domains, tracking entities per instance and managing instance eviction/teardown hooks.
- **Ingress Route Handover**: Gateways handle route updates to instance node endpoints transparently.
- **Cross-Cluster Federation & Claim Tickets**: Inter-realm transfer token issuer/verifier with HMAC-SHA256 signature verification, preventing tampering and replay attacks.
- **Cluster Protocol Definitions**: Protobuf RPC definitions in `mmo_schema` for instance creation, eviction, realm transfer issuance, and redemption.

## Non-Goals
- Specific dungeon gameplay mechanics (loot tables, mob AI scripts, boss phases) — those belong in plugins (`crates/gameplay`).
- Persistent storage schemas for instance state (instances are strictly ephemeral; persistent player save/load uses existing persistence mechanisms).
- Hardcoded realm topology discovery via external cloud providers.
