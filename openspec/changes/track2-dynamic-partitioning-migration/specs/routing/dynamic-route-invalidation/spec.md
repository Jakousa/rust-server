# Capability Specification: Dynamic Gateway Route Invalidation

## Domain: Ingress & Session Routing

### Feature: Live Session Route Updates

---

#### Scenario: Live Ingress Route Invalidation on Migration
- **GIVEN** an active client connection on the Edge Gateway mapped to session ID $S$ and player GUID $G$
- **WHEN** the entity migrates from Donor Node $D$ to Recipient Node $R$
- **THEN** the Recipient Node or Director sends an `UpdateRoute` gRPC request to the Edge Gateway specifying $(G, S, \text{node\_addr}_R)$
- **AND** the Edge Gateway updates its lock-free `SessionRouter` mapping $G \mapsto \text{node\_addr}_R$ and $S \mapsto \text{node\_addr}_R$
- **AND** subsequent client packets arriving on session $S$ are immediately dispatched to Node $R$ without packet drops or socket re-establishment.

#### Scenario: In-Flight Forwarding Grace Period
- **GIVEN** a client packet dispatched by the Edge Gateway to Donor Node $D$ immediately prior to route update receipt
- **WHEN** Donor Node $D$ receives the packet while the entity is in `Migrating` lease state
- **THEN** Donor Node $D$ either forwards the packet payload to Recipient Node $R$ via inter-node command forwarding or gracefully ignores non-critical movement inputs
- **AND** no client transport disconnection occurs.
