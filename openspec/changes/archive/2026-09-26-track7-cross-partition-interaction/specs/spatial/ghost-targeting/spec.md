# Specification: Ghost Target Resolution & Unified Spatial Queries

## Capability: `spatial/ghost-targeting`

### Context
In a spatial partitioned cluster, entities near a node's border are replicated as read-only proxies ([`GhostRecord`](file:///home/japek/Github/rust-server/crates/node/src/network.rs#L57)) in [`GhostStore`](file:///home/japek/Github/rust-server/crates/node/src/network.rs#L69). Players must be able to target and gather these proxies in 3D spatial queries without knowing which cluster node simulates them.

---

### Scenario: Point-to-Point Range Validation Against a Ghost Target
GIVEN an authoritative entity $A$ at position $(10.0, 0.0, 0.0)$ on Node 1
AND a ghost entity $B$ at position $(15.0, 0.0, 0.0)$ replicated from Node 2 with `max_range = 10.0m`
WHEN validating range using unified `validate_point_range`
THEN the validation succeeds with distance $= 5.0m$
AND the system identifies $B$ as a remote candidate owned by Node 2.

### Scenario: Cone / Cleave Arc Gathering Across Partition Border
GIVEN entity $A$ facing along the $+X$ axis ($(1.0, 0.0, 0.0)$)
AND local entities $L_1, L_2$ within the frontal $90^\circ$ cone
AND ghost entities $G_1$ (within the cone) and $G_2$ (behind $A$, outside the cone)
WHEN executing `query_cone_targets` against the unified candidate set
THEN $L_1, L_2,$ and $G_1$ are returned in the target list
AND $G_2$ is filtered out
AND the result preserves sorted distance from $A$.

### Scenario: Raycast Line-of-Sight Across Boundary
GIVEN a caster entity $A$ at $(0.0, 1.0, 0.0)$ targeting ghost entity $B$ at $(20.0, 1.0, 0.0)$
AND an authoritative obstacle AABB positioned at $(10.0, 1.0, 0.0)$ intersecting the ray between $A$ and $B$
WHEN validating Line-of-Sight with `validate_line_of_sight`
THEN the validation returns `Err(SpatialQueryError::LineOfSightBlocked)`
AND prevents ability activation through solid terrain.

### Scenario: Stale Ghost Exclusion
GIVEN a ghost entity record in `GhostStore` that has not received a sync update for $> 30$ ticks ($1.5\text{s}$)
WHEN evaluating spatial queries
THEN the stale ghost is treated as out-of-range or despawned
AND excluded from target candidate lists to avoid phantom ability hits.
