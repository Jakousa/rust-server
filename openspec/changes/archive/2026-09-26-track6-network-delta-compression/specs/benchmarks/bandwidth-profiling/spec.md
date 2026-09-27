# Specification: Bandwidth Profiling & Verification

## Capability: `benchmarks/bandwidth-profiling`

### Context
The ultimate metric of success for Track 6 is a measurable, reproducible reduction in network egress bytes under realistic cluster load and high entity density.

---

### Scenario: Egress Bandwidth Benchmark Under 1,000 Bot Density
GIVEN a cluster simulation node running 20 Hz ticks with 1,000 active bots performing random walk and combat interactions
WHEN comparing uncompressed `EntityStatePacket` replication against bit-packed `EntityDeltaBatchPacket` replication
THEN total egress bytes per tick must decrease by at least 60% (targeting 70%–80%)
AND memory consumption of the baseline tracker must remain bounded with a hard capacity cap (max 60 ticks history per observer, $\le 300\text{ MB}$ under worst-case 3-second lag across all 1,000 bots, and $< 20\text{ MB}$ under typical healthy ACK latency).

### Scenario: Per-Entity Wire Size Reduction
GIVEN an entity with typical movement (position and yaw changed, health/flags constant)
WHEN serialized via `EntityDeltaBatchPacket`
THEN the per-entity wire footprint within the batch must be $\le 12$ bytes (compared to 68 bytes uncompressed, an $82\%$ reduction).

### Scenario: Zero Allocations in Replication Loop
GIVEN the simulation tick loop updating 200 observers with 50 entities each
WHEN generating delta batches
THEN scratch buffers and object pools are reused
AND no transient heap reallocations occur in the per-entity encoding loop.
