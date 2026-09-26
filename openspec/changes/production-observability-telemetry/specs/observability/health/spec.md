# Observability Health Probing Specification

## Purpose

Defines HTTP liveness and readiness probing endpoints across cluster components for container orchestration and automated failure recovery.

## ADDED Requirements

### Requirement: HTTP Liveness Probe
Each server binary SHALL expose an HTTP `GET /healthz` endpoint to verify process liveness.

#### Scenario: Orchestrator queries liveness probe
- **GIVEN** a running server process
- **WHEN** an HTTP `GET /healthz` request is received
- **THEN** the server SHALL respond with HTTP status `200 OK`
- **AND** the response body SHALL contain a status indicator confirming liveness.

### Requirement: HTTP Readiness Probe
Each server binary SHALL expose an HTTP `GET /ready` endpoint indicating whether the service is fully initialized and prepared to accept incoming client or cluster traffic.

#### Scenario: Service initial startup before readiness
- **GIVEN** a server process in the process of starting up (connecting to database or discovering Director)
- **WHEN** an HTTP `GET /ready` request is received prior to initialization completing
- **THEN** the server SHALL respond with HTTP status `503 Service Unavailable`.

#### Scenario: Service operational after readiness
- **GIVEN** a server process that has completed its startup sequence
- **WHEN** an HTTP `GET /ready` request is received
- **THEN** the server SHALL respond with HTTP status `200 OK`.
