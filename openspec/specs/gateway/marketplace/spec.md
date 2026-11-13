# Asynchronous Global Marketplace Service Specification

## Purpose
Defines edge gateway interception and ACID storage settlement for asynchronous item listings, escrow management, buyout settlement, and listing cancellation/expiration refunds without burdening the 20 Hz simulation loop.

## Requirements

### Requirement: Marketplace Simulation Bypass
`mmo-gateway` SHALL intercept marketplace packets at the edge tier and execute them asynchronously against `MarketplaceService` and `mmo-storage` without forwarding them to simulation nodes.

#### Scenario: Edge interception of marketplace requests
- **GIVEN** an active client connection on `mmo-gateway`
- **WHEN** the client sends a `MarketListRequest`, `MarketBuyRequest`, `MarketCancelRequest`, or `MarketQueryRequest`
- **THEN** `mmo-gateway` SHALL intercept the packet at the edge gateway
- **AND** execute the request asynchronously against `MarketplaceService` and `mmo-storage`
- **AND** return a `MarketResponse` packet over the client's reliable stream sink
- **AND** completely bypass all `mmo-node` spatial simulation loops.

### Requirement: Safe Atomic Item Listing with Escrow
Listing an item on the marketplace SHALL verify ownership and atomically transfer the item into market escrow.

#### Scenario: Atomic escrow transfer on MarketListRequest
- **GIVEN** a seller attempting to list an item from `container_id`
- **WHEN** `MarketListRequest` is processed
- **THEN** the system SHALL verify container ownership and atomically transfer the item to the market escrow container
- **AND** persist a `market_listings` row with status `Active` and listing expiry timestamp
- **AND** prevent double-listing, double-spending, or modifying the item while listed.

### Requirement: Atomic Purchase Settlement
Purchasing an active marketplace listing SHALL settle payment and item delivery atomically under ACID row isolation.

#### Scenario: ACID buyout settlement and rollback on failure
- **GIVEN** an active listing with item `I` and buyout price `P`
- **WHEN** a buyer issues a `MarketBuyRequest` with sufficient currency/tokens in their payment container
- **THEN** the transaction SHALL execute under ACID row isolation:
  - Deduct price `P` from buyer's container and credit seller's container
  - Transfer item `I` from market escrow container to buyer's target container
  - Update listing status to `Sold`
- **AND** if any condition fails (insufficient funds, buyer container full, listing already sold or cancelled), the entire operation SHALL roll back with a descriptive error code in `MarketResponse`.

### Requirement: Listing Cancellation and Expiration Refunds
Cancelled or expired marketplace listings SHALL atomically return the escrowed item to the seller's container.

#### Scenario: Seller cancellation or TTL expiration refund
- **GIVEN** an active listing owned by seller `S`
- **WHEN** `MarketCancelRequest` is invoked by `S` (or when the listing expires past its TTL)
- **THEN** the listing status SHALL transition to `Cancelled` (or `Expired`)
- **AND** the escrowed item SHALL be returned atomically to the seller's container.
