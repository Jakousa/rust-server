## Purpose

Executes the fixed-frequency simulation game loop, client movement validation, and kinematic anti-cheat checks.

## ADDED Requirements

### Requirement: Fixed 20 Hz Simulation Loop
The simulation node SHALL execute game world updates on a fixed 20 Hz (50 millisecond) tick interval.

#### Scenario: Tick stepping
- **WHEN** 50 milliseconds elapse in wall-clock time
- **THEN** the simulation node SHALL execute one simulation step across all locally authoritative entities

### Requirement: Client Kinematic Sanity Validation
The system SHALL validate client-reported movement coordinates against maximum speed caps, acceleration bounds, and world limits.

#### Scenario: Valid movement acceptance
- **WHEN** a client submits a movement packet where distance divided by elapsed time is within speed limits
- **THEN** the system SHALL update the authoritative entity coordinates and broadcast the movement

#### Scenario: Speed hack rejection
- **WHEN** a client submits a movement packet exceeding allowable speed limits
- **THEN** the system SHALL reject the coordinates and send a snap-back correction packet to the client
