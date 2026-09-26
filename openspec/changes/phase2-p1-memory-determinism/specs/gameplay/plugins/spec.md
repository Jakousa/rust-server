# Delta Specification: gameplay/plugins

## ADDED Requirements

### Requirement: Event-Driven Despawn and Death Pruning
Gameplay plugins tracking entity references SHALL subscribe to entity despawn and death notifications to prune stale references reactively without executing full-world entity iterations every tick.

#### Scenario: Reactive cleanup on entity despawn
- **WHEN** an entity despawns or dies in the simulation
- **THEN** registered gameplay plugins (such as spawner and threat tracking) SHALL receive notification and remove stale GUID references from internal tracking structures without querying all entities in the entity store
