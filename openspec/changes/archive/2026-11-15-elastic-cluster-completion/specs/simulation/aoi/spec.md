## ADDED Requirements

### Requirement: Cross-Partition Boundary Ghost AoI Visibility
Simulation nodes SHALL replicate near-boundary entities to adjacent peer partitions and include nearby `GhostStore` records in observer AoI delta broadcasts so entities remain visible across partition seams without popping out before ownership handoff.

#### Scenario: Observer near partition boundary receives ghost entity updates
- **GIVEN** an active observer on `Node 1` near a partition seam within `effective_radius` of an entity owned by `Node 2` replicated into `Node 1`'s `GhostStore`
- **WHEN** `AoiManager::update_observers_with_ghosts` runs during a simulation tick
- **THEN** the ghost entity SHALL be included in the observer's visible set and encoded into `EntityDeltaBatch` / `EntityState` packets without duplicating local entities.

### Requirement: Client Cross-Node Tick Continuity on Partition Handover
The client interpolation buffer and WebTransport bridge SHALL cleanly rebase entity interpolation history when a player migrates to a peer node whose authoritative server tick resets or jumps backward relative to the donor node.

#### Scenario: Client transitions to peer node with lower server_tick
- **GIVEN** a client has buffered snapshots for an entity from `Node 1` at `server_tick = 5000`
- **WHEN** the player migrates to `Node 2` and receives `EntityDeltaBatch` updates starting at `server_tick = 12`
- **THEN** `InterpolationBuffer::push_snapshot` and `ClientWorldBridge._pushInterpSnapshot` SHALL reset the stale tick baseline (`back.tick - tick > 4`) and enqueue the new snapshot smoothly without dropping updates or freezing entity rendering.
