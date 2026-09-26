## MODIFIED Requirements

### Requirement: Client Kinematic Sanity Validation
The system SHALL validate client-reported movement coordinates against maximum speed caps, acceleration bounds, and world limits, calculating elapsed delta time strictly from server-side monotonic clock timestamps and tracking movement packet sequence monotonicity while completely ignoring any client-provided delta time.

#### Scenario: Valid movement acceptance
- **WHEN** a client submits a movement packet where distance divided by server-measured elapsed time since the entity's last movement is within speed limits and sequence order is valid
- **THEN** the system SHALL update the authoritative entity coordinates, advance the monotonic movement timestamp and sequence tracker, and broadcast the movement

#### Scenario: Speed hack rejection
- **WHEN** a client submits a movement packet exceeding allowable speed limits under server-measured delta time
- **THEN** the system SHALL reject the coordinates and send a snap-back correction packet to the client

#### Scenario: Client spoofed delta time rejection
- **WHEN** a client submits an oversized displacement alongside an artificially inflated client delta time
- **THEN** the server SHALL ignore the client's delta time, calculate speed using server monotonic elapsed time, detect the speed hack, and issue a snap-back correction packet

## ADDED Requirements

### Requirement: Remote Command Initiator Verification
The simulation node SHALL strictly verify and require a valid, non-zero initiator GUID in incoming remote entity commands, rejecting commands with malformed or undersized payloads without synthesizing synthetic fallback player identities.

#### Scenario: Undersized remote command rejection
- **WHEN** an incoming remote entity command contains an undersized or invalid initiator payload
- **THEN** the node SHALL reject the command with an error and refuse to execute or dispatch the command
