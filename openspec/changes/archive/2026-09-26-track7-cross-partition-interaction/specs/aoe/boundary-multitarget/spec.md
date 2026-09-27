# Specification: Boundary Multi-Target AoE Partitioning & Dispatch

## Capability: `aoe/boundary-multitarget`

### Context
Area-of-Effect (AoE) abilities (e.g. explosive spells, cleaves, auras) centered near or crossing a spatial partition boundary encompass entities managed by multiple different simulation nodes. The system must partition target entities by owner and dispatch batched hits in parallel.

---

### Scenario: Boundary AoE Target Partitioning
GIVEN an AoE radial explosion at $(95.0, 0.0, 0.0)$ with radius $20.0m$ centered on Node 1 (bounds $X \le 100.0$)
AND 3 local entities on Node 1 within the blast
AND 2 ghost entities belonging to Node 2 (bounds $X > 100.0$) within the blast
WHEN the spatial query gathers targets
THEN targets are partitioned into:
  - Local batch (Node 1): 3 entities
  - Remote batch (Node 2): 2 entities
AND zero duplicate entity entries exist in either batch.

### Scenario: Parallel Remote Batch Dispatch
GIVEN an AoE partitioning yielding a local batch and remote batches for Node 2 and Node 3
WHEN executing the ability
THEN local entities are damaged immediately on Node 1
AND a single `BatchRemoteAbilityRequest` is dispatched to Node 2 for its entities
AND a single `BatchRemoteAbilityRequest` is dispatched to Node 3 for its entities
AND Node 1 awaits completion of remote hits without blocking the simulation tick loop.

### Scenario: Partial Node Failure or Timeout During AoE
GIVEN an AoE dispatched across Node 1, Node 2, and Node 3
AND Node 3 becomes unresponsive or crashes
WHEN awaiting responses with a strict timeout (e.g. $100\text{ms}$)
THEN hits on Node 1 and Node 2 remain committed and successful
AND the timeout on Node 3 logs a boundary dispatch warning without crashing Node 1 or rolling back local hits.
