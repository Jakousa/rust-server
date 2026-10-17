# Architectural and Code Review: feat-social-pubsub-and-marketplace

## Summary of Findings
The proposal introduces a highly effective, decoupled architecture that successfully offloads non-spatial processing from the core simulation loop (`mmo-node`). By terminating social pub/sub and marketplace services at `mmo-gateway`, the design avoids burdening 20 Hz spatial physics ticks with chat and economy events. The design correctly leverages generic containers and identifiers, strictly adhering to the unopinionated Prime Directive defined in `AGENTS.md`.

However, there are several critical flaws concerning DashMap lock ordering, unbound channel DoS risks, transaction atomicity, and container ownership validation that must be addressed before implementation begins.

## Critical Architectural Assessment

1. **Deadlock Risk in Gateway DashMaps (Concurrency)**
   Section 2.3 defines `GatewaySubscriptionManager` with two DashMaps (`topic_subscribers` and `session_topics`). In `SessionDropGuard`, removing a session requires reading `session_topics` and mutating `topic_subscribers`. If locks are nested (e.g., holding a reference to `session_topics` while modifying `topic_subscribers`), it creates a severe deadlock risk against incoming messages that lock `topic_subscribers` first.

2. **DoS Vulnerability via Unbounded Channels (Security & Concurrency)**
   The design specifies `SessionSink.stream_tx` as an "unbounded reliable channel". Exposing an unbounded channel to network clients is a critical DoS vulnerability. A slow or malicious client that stops reading TCP/QUIC packets will cause the gateway to queue messages indefinitely, leading to memory exhaustion (OOM) and gateway crashes.

3. **Escrow Late-Binding & Atomicity (Edge Cases)**
   The design proposes using the escrow system for market listings. However, `create_escrow` in `escrow_coord.rs` requires a `beneficiary_container_id` at creation time, which is unknown when a listing is created (the buyer is unknown). Furthermore, if `MarketplaceRepository` creates an escrow and *then* inserts a `market_listings` row as two separate steps, a server crash between them will leave the item orphaned in the escrow container with no associated listing to recover it.

4. **Marketplace Container Ownership Validation (Security & Anti-Cheat)**
   While social packets mention `sender_guid` validation, the marketplace design does not explicitly demand ownership validation of `src_container_id` and `buyer_container_id`. Because `transaction_coord.rs` blindly processes transfers, the `MarketplaceRepository` MUST strictly verify that the authenticated session `Guid` owns the source and payment containers. Without this, malicious actors could forge packets to list or spend items from other players' containers.

5. **Listing Expiration Trigger (Edge Cases & Blackout Scenarios)**
   The spec notes listings will be refunded "when the listing expires past its TTL", but doesn't define the execution trigger. Relying on lazy evaluation during queries won't automatically refund items to offline players. The system requires an active background reaper task.

## Concrete Recommendations / Adjustments

1. **Avoid DashMap Deadlocks**: Update the design and tasks for `GatewaySubscriptionManager` to strictly forbid nested DashMap locks. During disconnects, clone the session's topic list from `session_topics`, drop the lock, and then iterate the cloned list to remove the session from `topic_subscribers`.
2. **Bounded Reliable Stream**: Change `SessionSink.stream_tx` from unbounded to a generously sized bounded channel (e.g., `mpsc::channel(10240)`). Implement a policy where if the channel saturates (returns `TrySendError::Full`), the gateway intentionally drops the sluggish client to protect server memory.
3. **Atomic Marketplace Listings**: Bypass the restrictive `create_escrow` function for listings. Instead, `MarketplaceRepository` should execute a single, monolithic ACID database transaction that simultaneously moves the item to the market's inventory container and inserts the `market_listings` row.
4. **Strict Ownership Checks**: Update Task 2.3 to explicitly require fetching the container's `owner_id` from the database and verifying it matches the authenticated session's GUID before initiating any listing or buy transaction.
5. **Add Background Reaper Task**: Add a new task (e.g., Task 2.5) to implement a background asynchronous sweeper loop in `MarketplaceService` that periodically queries expired `market_listings`, transitions them to `Expired`, and atomically returns the items to their respective sellers.

## Final Verdict
**APPROVED WITH MINOR ADJUSTMENTS**
(Please integrate the recommendations above into the design and task list before beginning implementation.)
