# Specification: Asynchronous Global Marketplace Service

## Capability: `gateway/marketplace` & `storage/marketplace`

### Feature: Marketplace Simulation Bypass
- **GIVEN** an active client connection on `mmo-gateway`
- **WHEN** the client sends a `MarketListRequest`, `MarketBuyRequest`, `MarketCancelRequest`, or `MarketQueryRequest`
- **THEN** `mmo-gateway` SHALL intercept the packet at the edge gateway
- **AND** execute the request asynchronously against `MarketplaceService` and `mmo-storage`
- **AND** return a `MarketResponse` packet over the client's reliable stream sink
- **AND** completely bypass all `mmo-node` spatial simulation loops.

### Feature: Safe Atomic Item Listing with Escrow
- **GIVEN** a seller attempting to list an item from `container_id`
- **WHEN** `MarketListRequest` is processed
- **THEN** the system SHALL verify container ownership and atomically transfer the item to the market escrow container
- **AND** persist a `market_listings` row with status `Active` and listing expiry timestamp
- **AND** prevent double-listing, double-spending, or modifying the item while listed.

### Feature: Atomic Purchase Settlement
- **GIVEN** an active listing with item `I` and buyout price `P`
- **WHEN** a buyer issues a `MarketBuyRequest` with sufficient currency/tokens in their payment container
- **THEN** the transaction SHALL execute under ACID row isolation:
  - Deduct price `P` from buyer's container and credit seller's container
  - Transfer item `I` from market escrow container to buyer's target container
  - Update listing status to `Sold`
- **AND** if any condition fails (insufficient funds, buyer container full, listing already sold or cancelled), the entire operation SHALL roll back with a descriptive error code in `MarketResponse`.

### Feature: Listing Cancellation and Expiration Refunds
- **GIVEN** an active listing owned by seller `S`
- **WHEN** `MarketCancelRequest` is invoked by `S` (or when the listing expires past its TTL)
- **THEN** the listing status SHALL transition to `Cancelled` (or `Expired`)
- **AND** the escrowed item SHALL be returned atomically to the seller's container.
