# Spec: Cancellable Timed Actions Primitives

## Capability: `primitives/timed-actions`

Provides an authoritative, tick-evaluated state machine for entity tasks that require elapsed time to complete and can be conditionally aborted.

---

### Requirement: Action Lifecycle & Progress

#### Scenario: Normal action completion
- **GIVEN** an entity with an active `ActionQueue` component
- **WHEN** an action with duration $D$ seconds is queued at simulation tick $T_0$
- **THEN** an `ActionStartedEvent` is emitted with the action ID and duration
- **AND** at each tick $T$, elapsed time advances by $\Delta t$
- **AND** when elapsed time $\ge D$, the action completes, emitting `ActionCompletedEvent`, and is removed from the queue.

---

### Requirement: Conditional Interruption

#### Scenario: Movement interruption
- **GIVEN** an active timed action configured with `CancelOnMovement = true`
- **WHEN** the entity executes a validated movement step with non-zero displacement
- **THEN** the action is immediately aborted
- **AND** an `ActionInterruptedEvent` is dispatched with reason `Movement`
- **AND** no `ActionCompletedEvent` is fired.

#### Scenario: Damage interruption
- **GIVEN** an active timed action configured with `CancelOnDamage = true`
- **WHEN** the entity suffers non-zero damage before completion
- **THEN** the action is immediately aborted with reason `Damage`.

#### Scenario: Stun / Crowd Control interruption
- **GIVEN** an active timed action
- **WHEN** the entity receives a crowd-control event or state flag (`Stunned`, `Incapacitated`)
- **THEN** the action is immediately aborted with reason `CrowdControl`.

---

### Requirement: Queueing & Overwrite Policy

#### Scenario: Queue replacement
- **GIVEN** an entity already performing an active timed action
- **WHEN** a new action is requested with `QueuePolicy::Replace`
- **THEN** the current action is canceled with reason `Replaced`
- **AND** the new action starts immediately.

#### Scenario: Queue rejection
- **GIVEN** an entity already performing an active timed action
- **WHEN** a new action is requested with `QueuePolicy::RejectIfBusy`
- **THEN** the request is rejected and the current action continues unaffected.
