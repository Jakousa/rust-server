# Specification: Cross-Partition Ability Dispatch Protocol

## Capability: `interaction/cross-node-dispatch`

### Context
When an ability targets an entity on a remote node, execution must be delegated authoritatively to the target's owning simulation node via gRPC, ensuring atomic resource deduction and proactive cooldown protection on the caster, coupled with authoritative state and range re-validation on the recipient node.

---

### Scenario: Single-Target Remote Ability Execution with Proactive Cooldown
GIVEN Caster $A$ on Node 1 initiating an ability with cost 20 Mana and damage 50 targeting $B$ on Node 2
WHEN Node 1 completes initial cast validation
THEN 20 Mana is deducted from $A$'s `AttributeSet` on Node 1
AND the ability cooldown and GCD are proactively triggered on $A$ immediately
AND Node 1 dispatches a `DispatchAbility` gRPC request to Node 2
AND Node 2 authoritatively validates:
  - Target $B$ exists and is alive (`HealthComponent.is_dead == false`)
  - Target $B$ is not under an active migration lease
  - Actual distance between `caster_position` and $B$'s authoritative position $\le \text{max\_range} \times 1.2$ (latency tolerance)
AND Node 2 applies 50 damage to $B$ and records combat events
AND Node 2 responds with `RemoteAbilityResponse { success: true, applied_amount: 50.0, target_died: false }`
AND Node 1 receives confirmation without requiring additional cooldown adjustments.

### Scenario: Anti-Cheat Authoritative Range Re-Validation Failure
GIVEN Caster $A$ on Node 1 dispatching an ability targeting $B$ based on a stale ghost record
AND $B$ on Node 2 has moved out of range ($> \text{max\_range} \times 1.2$)
WHEN Node 2 receives `DispatchAbility`
THEN Node 2 rejects the request with `error_code = OutOfRange`
AND Node 1 enqueues a compensation event draining at tick start
AND Node 1 refunds $A$'s 20 Mana and resets the ability cooldown.

### Scenario: Remote Target Dead or Missing (Veto & Compensation)
GIVEN Caster $A$ initiating an ability against $B$ on Node 2
AND target $B$ died or despawned on Node 2 prior to the dispatch arriving
WHEN Node 2 receives `DispatchAbility`
THEN Node 2 rejects the request with `error_code = TargetDead`
AND Node 1 enqueues a compensation event that refunds $A$'s 20 Mana and clears the cooldown.

### Scenario: Target Under Active Migration Lease (Strict Freeze)
GIVEN Target $B$ currently undergoing two-phase migration with an active state freeze lease
WHEN Node 2 receives `DispatchAbility` for $B$
THEN Node 2 rejects the request with `error_code = TargetMigrating`
AND Node 1 refunds the caster and logs the migration conflict cleanly.

### Scenario: Caster Migration Mid-Flight ("Boundary Dance" Prevention)
GIVEN Caster $A$ dispatching a cross-node ability against $B$
AND Caster $A$ crosses the spatial boundary and migrates to Node 2 while the gRPC call is in flight
WHEN migration occurs
THEN $A$'s active cooldowns in `CooldownTracker` are serialized and carried over to Node 2
AND $A$ cannot bypass cooldowns or double-cast high-cooldown abilities across partition boundaries.

### Scenario: Non-Blocking Compensation Queueing
GIVEN incoming gRPC responses from peer simulation nodes
WHEN responses arrive on the Tokio asynchronous runtime
THEN compensation results are pushed into an unbounded MPSC queue on `TickSimulation`
AND the simulation tick loop drains and applies refunds at the start of `step()` without acquiring write locks during network callbacks.
