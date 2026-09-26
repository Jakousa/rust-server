# Delta Specification: simulation/tick

## MODIFIED Requirements

### Requirement: Fixed 20 Hz Simulation Loop
The simulation node SHALL execute game world updates on a fixed 20 Hz (50 millisecond) tick interval using a clamped tick accumulator that prevents catch-up starvation under transient load spikes.

#### Scenario: Tick stepping
- **WHEN** 50 milliseconds elapse in wall-clock time
- **THEN** the simulation node SHALL execute one simulation step across all locally authoritative entities

#### Scenario: Clamped catch-up on tick overrun
- **WHEN** execution time or scheduling delays cause accumulated lag exceeding multiple tick intervals
- **THEN** the simulation loop SHALL execute a clamped maximum of catch-up ticks (at most 3 ticks per cycle) and yield control back to the async runtime, resetting residual lag to prevent starvation of network I/O
