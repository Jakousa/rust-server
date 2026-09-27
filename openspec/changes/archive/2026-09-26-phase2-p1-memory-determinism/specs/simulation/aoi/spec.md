# Delta Specification: simulation/aoi

## MODIFIED Requirements

### Requirement: Dynamic Visibility Throttling
The system SHALL dynamically clamp the observer's visibility radius when the number of visible entities exceeds the configured per-observer density budget, using an $O(N)$ partial selection algorithm.

#### Scenario: High entity density throttling
- **WHEN** the count of entities within the player's default visibility radius exceeds the configured maximum entity threshold
- **THEN** the system SHALL select the nearest entities up to the density budget using $O(N)$ partitioning without performing a full $O(N \log N)$ sort across the entire candidate set, and reduce the effective visibility radius accordingly

## ADDED Requirements

### Requirement: Zero-Allocation Scratch Buffering
The simulation AoI manager SHALL maintain reusable internal scratch buffers for entity candidate collection and visibility delta computations to avoid allocating heap vectors per observer on every tick.

#### Scenario: Reusing scratch buffers across observers
- **WHEN** the AoI manager evaluates visibility for multiple active observers during an authoritative simulation tick
- **THEN** it SHALL clear and reuse pre-allocated scratch buffers rather than allocating new vectors per observer query
