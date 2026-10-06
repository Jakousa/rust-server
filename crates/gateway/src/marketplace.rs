//! Gateway marketplace service handling asynchronous player market actions.
//!
//! Handles listing creation, purchasing, cancellation, and queries directly at the
//! edge gateway layer, executing transactions against `mmo_storage::MarketplaceRepository`
//! and completely bypassing `mmo-node` spatial simulation loops.

use std::sync::Arc;

use mmo_schema::client::{
    MarketBuyRequestPacket, MarketCancelRequestPacket, MarketListRequestPacket, MarketListingWire,
    MarketQueryRequestPacket, MarketResponsePacket,
};
use mmo_storage::models::MarketListingRecord;
use mmo_storage::MarketplaceRepository;

/// Default marketplace escrow container GUID/ID when none is explicitly specified.
pub const DEFAULT_MARKET_ESCROW_CONTAINER_ID: u64 = 999_999;

/// Wire opcode indicator constants for market request types.
pub const REQUEST_TYPE_LIST: u8 = 1;
pub const REQUEST_TYPE_BUY: u8 = 2;
pub const REQUEST_TYPE_CANCEL: u8 = 3;
pub const REQUEST_TYPE_QUERY: u8 = 4;

/// Error code constants for market responses.
pub const MARKET_ERROR_SUCCESS: u32 = 0;
pub const MARKET_ERROR_GENERIC: u32 = 1;
pub const MARKET_ERROR_UNAUTHORIZED: u32 = 2;
pub const MARKET_ERROR_NOT_FOUND: u32 = 3;
pub const MARKET_ERROR_NOT_ACTIVE: u32 = 4;

/// Asynchronous marketplace service operating at the edge gateway.
pub struct GatewayMarketplaceService {
    repo: Arc<MarketplaceRepository>,
    escrow_container_id: u64,
}

impl GatewayMarketplaceService {
    /// Creates a new `GatewayMarketplaceService` backed by the shared storage repository.
    pub fn new(repo: Arc<MarketplaceRepository>) -> Self {
        Self {
            repo,
            escrow_container_id: DEFAULT_MARKET_ESCROW_CONTAINER_ID,
        }
    }

    /// Sets the designated market escrow container ID.
    pub fn with_escrow_container_id(mut self, escrow_container_id: u64) -> Self {
        self.escrow_container_id = escrow_container_id;
        self
    }

    /// Accesses the underlying marketplace repository.
    pub fn repo(&self) -> &Arc<MarketplaceRepository> {
        &self.repo
    }

    /// Accesses the configured escrow container ID.
    pub fn escrow_container_id(&self) -> u64 {
        self.escrow_container_id
    }

    /// Handles a market item listing request.
    pub async fn handle_list(
        &self,
        player_guid: u64,
        req: MarketListRequestPacket,
    ) -> MarketResponsePacket {
        match self
            .repo
            .create_listing(
                player_guid,
                req.src_container_id,
                req.src_slot,
                req.price,
                req.duration_secs,
                self.escrow_container_id,
            )
            .await
        {
            Ok(listing) => MarketResponsePacket {
                request_type: REQUEST_TYPE_LIST,
                success: true,
                error_code: MARKET_ERROR_SUCCESS,
                error_message: String::new(),
                listings: vec![listing_to_wire(&listing)],
            },
            Err(e) => {
                let error_code = match &e {
                    mmo_storage::db::StorageError::UnauthorizedMarketAction(_) => {
                        MARKET_ERROR_UNAUTHORIZED
                    }
                    _ => MARKET_ERROR_GENERIC,
                };
                MarketResponsePacket {
                    request_type: REQUEST_TYPE_LIST,
                    success: false,
                    error_code,
                    error_message: e.to_string(),
                    listings: Vec::new(),
                }
            }
        }
    }

    /// Handles a market item buy request.
    pub async fn handle_buy(
        &self,
        buyer_guid: u64,
        req: MarketBuyRequestPacket,
    ) -> MarketResponsePacket {
        match self
            .repo
            .purchase_listing(
                buyer_guid,
                &req.listing_id,
                req.buyer_container_id,
                req.payment_container_id,
                req.price,
            )
            .await
        {
            Ok(listing) => MarketResponsePacket {
                request_type: REQUEST_TYPE_BUY,
                success: true,
                error_code: MARKET_ERROR_SUCCESS,
                error_message: String::new(),
                listings: vec![listing_to_wire(&listing)],
            },
            Err(e) => {
                let error_code = match &e {
                    mmo_storage::db::StorageError::ListingNotFound(_) => MARKET_ERROR_NOT_FOUND,
                    mmo_storage::db::StorageError::ListingNotActive(..) => MARKET_ERROR_NOT_ACTIVE,
                    mmo_storage::db::StorageError::UnauthorizedMarketAction(_) => {
                        MARKET_ERROR_UNAUTHORIZED
                    }
                    _ => MARKET_ERROR_GENERIC,
                };
                MarketResponsePacket {
                    request_type: REQUEST_TYPE_BUY,
                    success: false,
                    error_code,
                    error_message: e.to_string(),
                    listings: Vec::new(),
                }
            }
        }
    }

    /// Handles a market listing cancellation request.
    pub async fn handle_cancel(
        &self,
        seller_guid: u64,
        req: MarketCancelRequestPacket,
    ) -> MarketResponsePacket {
        match self
            .repo
            .cancel_listing(seller_guid, &req.listing_id, req.dst_container_id)
            .await
        {
            Ok(listing) => MarketResponsePacket {
                request_type: REQUEST_TYPE_CANCEL,
                success: true,
                error_code: MARKET_ERROR_SUCCESS,
                error_message: String::new(),
                listings: vec![listing_to_wire(&listing)],
            },
            Err(e) => {
                let error_code = match &e {
                    mmo_storage::db::StorageError::ListingNotFound(_) => MARKET_ERROR_NOT_FOUND,
                    mmo_storage::db::StorageError::ListingNotActive(..) => MARKET_ERROR_NOT_ACTIVE,
                    mmo_storage::db::StorageError::UnauthorizedMarketAction(_) => {
                        MARKET_ERROR_UNAUTHORIZED
                    }
                    _ => MARKET_ERROR_GENERIC,
                };
                MarketResponsePacket {
                    request_type: REQUEST_TYPE_CANCEL,
                    success: false,
                    error_code,
                    error_message: e.to_string(),
                    listings: Vec::new(),
                }
            }
        }
    }

    /// Handles a market listings query request.
    pub async fn handle_query(&self, req: MarketQueryRequestPacket) -> MarketResponsePacket {
        let item_def_id = if req.item_def_id > 0 {
            Some(req.item_def_id)
        } else {
            None
        };
        let min_price = if req.min_price > 0 {
            Some(req.min_price)
        } else {
            None
        };
        let max_price = if req.max_price > 0 {
            Some(req.max_price)
        } else {
            None
        };

        match self
            .repo
            .query_listings(item_def_id, min_price, max_price, req.offset, req.limit)
            .await
        {
            Ok(listings) => MarketResponsePacket {
                request_type: REQUEST_TYPE_QUERY,
                success: true,
                error_code: MARKET_ERROR_SUCCESS,
                error_message: String::new(),
                listings: listings.iter().map(listing_to_wire).collect(),
            },
            Err(e) => MarketResponsePacket {
                request_type: REQUEST_TYPE_QUERY,
                success: false,
                error_code: MARKET_ERROR_GENERIC,
                error_message: e.to_string(),
                listings: Vec::new(),
            },
        }
    }
}

/// Converts a `MarketListingRecord` storage record into wire `MarketListingWire` format.
fn listing_to_wire(record: &MarketListingRecord) -> MarketListingWire {
    MarketListingWire {
        listing_id: record.listing_id.clone(),
        seller_guid: record.seller_guid,
        item_instance_id: record.item_instance_id.clone(),
        item_def_id: record.item_def_id,
        price: record.price,
        created_at: record.created_at_epoch_ms as u64,
        expires_at: record.expires_at_epoch_ms as u64,
    }
}
