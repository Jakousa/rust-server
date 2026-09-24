# Architectural and Technical Review: Client AoI Broadcasting

After a thorough review of the proposed OpenSpec change and the existing codebase, here are my detailed findings:

## 1. Architectural Soundness
**Status: Robust**
The separation of concerns between `mmo_node` handling the authoritative Area-of-Interest (AoI) simulation and `mmo_gateway` handling the downstream socket multiplexing is excellent. Tracking the AoI purely on the server node prevents client-side manipulation (e.g., wallhacks) since clients are only ever sent what they are explicitly permitted to see.

## 2. Algorithmic Efficiency
**Status: Needs Improvement (CRITICAL)**
The `design.md` explicitly calls for "Minimal allocation per tick by reusing observer sets and spatial query buffers." However, cross-referencing this with the current `Octree` implementation in `crates/core/src/octree.rs` reveals a performance bottleneck:
- Methods like `Octree::query_radius_with_pos` instantiate and return a **new** `Vec` for every call (`let mut results = Vec::new();`).
- At 20 Hz, with 1,000 players, this will result in 20,000 `Vec` allocations per second per simulation node.
- **Suggestion:** Before implementing `AoiManager`, add an `Octree::query_radius_into` method that accepts a mutable reference to an existing buffer (`&mut Vec<(Guid, (f32, f32, f32))>` or similar). This will allow `AoiManager` to reuse a single pre-allocated buffer across ticks for spatial queries.
- **Density Throttling Edge Case:** If the returned entity count exceeds `max_entities`, pruning the "outer" entities requires sorting them by distance. Since `query_radius_with_pos` does not return the squared distance to the center, `AoiManager` will have to compute this redundantly. Consider either returning the computed distance from the Octree query or explicitly computing it during the sorting phase.

## 3. Wire Protocol Alignment
**Status: Validated**
The protocol alignment is correct. `EntityStatePacket` and `EntityDespawnPacket` exist in `crates/schema/src/client.rs` and properly encapsulate position, velocity, yaw, pitch, health, and max health, matching the `design.md` requirements. Re-using these binary codecs will easily slot into the new broadcasting pipeline.

## 4. Gateway Async Design
**Status: Robust**
The decision to utilize `TcpStream::into_split()` alongside a per-session `mpsc::channel` in `GatewayServer` is perfectly suited for Tokio's async model. By separating the read and write halves into distinct tasks, you entirely avoid blocking the incoming packet reader loop when the server needs to broadcast large AoI updates, mitigating potential latency jitter and async deadlocks.

## 5. Task Breakdown
**Status: Needs Minor Adjustments**
The sequence in `tasks.md` is logical and verifiable, but misses the necessary prerequisite work on the Octree.
- **Suggestion:** Add a new prerequisite task `1.0`: "Refactor `Octree` spatial queries in `crates/core/src/octree.rs` to support zero-allocation buffer reuse (`query_radius_into`) to support AoiManager's performance goals."

## Verdict
**REVISIONS_RECOMMENDED**

Please address the algorithmic efficiency constraint regarding `Octree` buffer allocations. Once the design and tasks reflect this required optimization, the implementation will be rock solid.
