//! Asynchronous Global Marketplace Service and Transactional Repository.
//!
//! Provides atomic, isolated listing (with escrow backing), purchasing, cancellation,
//! expiration reaping, and paginated queries completely outside spatial simulation loops.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::Row;

use crate::db::{sql_for_dialect, Database, DbBackend, StorageError};
pub use crate::models::{ListingStatus, MarketListingRecord};

macro_rules! parse_market_listing {
    ($r:expr) => {{
        let status_str: String = $r.try_get("status")?;
        let status = status_str
            .parse::<ListingStatus>()
            .map_err(|e| StorageError::NotFound(e.to_string()))?;
        let seller_guid: i64 = $r.try_get("seller_guid")?;
        let seller_container_id: i64 = $r.try_get("seller_container_id")?;
        let item_def_id: i32 = $r.try_get("item_def_id")?;
        let price: i64 = $r.try_get("price")?;
        let escrow_container_id: i64 = $r.try_get("escrow_container_id")?;
        let escrow_slot: i32 = $r.try_get("escrow_slot")?;

        MarketListingRecord {
            listing_id: $r.try_get("listing_id")?,
            seller_guid: seller_guid as u64,
            seller_container_id: seller_container_id as u64,
            item_instance_id: $r.try_get("item_instance_id")?,
            item_def_id: item_def_id as u32,
            price: price as u64,
            escrow_container_id: escrow_container_id as u64,
            escrow_slot: escrow_slot as u32,
            status,
            created_at_epoch_ms: $r.try_get("created_at_epoch_ms")?,
            expires_at_epoch_ms: $r.try_get("expires_at_epoch_ms")?,
            updated_at_epoch_ms: $r.try_get("updated_at_epoch_ms")?,
        }
    }};
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn current_time_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

struct ExpiredCandidate {
    listing_id: String,
    seller_container_id: i64,
    item_instance_id: String,
}

/// Repository managing asynchronous marketplace transactions and listings.
#[derive(Clone)]
pub struct MarketplaceRepository {
    db: Database,
}

impl MarketplaceRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Access the underlying database handle.
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Atomically creates an active market listing by transferring the item from the seller's
    /// container into the marketplace escrow container under a single ACID transaction.
    pub async fn create_listing(
        &self,
        seller_guid: u64,
        src_container_id: u64,
        src_slot: u32,
        price: u64,
        duration_secs: u64,
        escrow_container_id: u64,
    ) -> Result<MarketListingRecord, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();
        let expires_at_epoch_ms = now_ms + (duration_secs as i64 * 1000);
        let listing_id = format!("list-{}-{}-{}", seller_guid, src_slot, now_ms);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut conn = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

                let res: Result<MarketListingRecord, StorageError> = async {
                    // 1. Verify seller ownership of source container
                    let c_src = sqlx::query(
                        "SELECT container_id, owner_id FROM containers WHERE container_id = ?",
                    )
                    .bind(src_container_id as i64)
                    .fetch_optional(&mut *conn)
                    .await?;

                    let c_src_row = c_src
                        .ok_or(StorageError::ContainerNotFound(src_container_id))?;
                    let owner_id: String = c_src_row.try_get("owner_id")?;
                    if owner_id != seller_guid.to_string() {
                        return Err(StorageError::UnauthorizedContainerAccess(src_container_id));
                    }

                    // 2. Verify escrow container exists and find free slot
                    let c_esc = sqlx::query(
                        "SELECT container_id, capacity FROM containers WHERE container_id = ?",
                    )
                    .bind(escrow_container_id as i64)
                    .fetch_optional(&mut *conn)
                    .await?;

                    let esc_row = c_esc
                        .ok_or(StorageError::ContainerNotFound(escrow_container_id))?;
                    let esc_cap: i32 = esc_row.try_get("capacity")?;

                    let occupied_rows =
                        sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                            .bind(escrow_container_id as i64)
                            .fetch_all(&mut *conn)
                            .await?;
                    let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let free_slot = (0..(esc_cap as u32))
                        .find(|s| !occupied_slots.contains(s))
                        .ok_or(StorageError::ContainerFull(
                            escrow_container_id,
                            esc_cap as u32,
                        ))?;

                    // 3. Verify item exists in seller container at slot
                    let item_row = sqlx::query(
                        "SELECT item_instance_id, item_def_id FROM items WHERE container_id = ? AND slot = ?",
                    )
                    .bind(src_container_id as i64)
                    .bind(src_slot as i32)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(|| {
                        StorageError::ItemNotFound(format!("slot {src_slot}"), src_container_id)
                    })?;

                    let item_instance_id: String = item_row.try_get("item_instance_id")?;
                    let item_def_id: i32 = item_row.try_get("item_def_id")?;

                    // 4. Move item to escrow container
                    sqlx::query(
                        "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
                    )
                    .bind(escrow_container_id as i64)
                    .bind(free_slot as i32)
                    .bind(now_secs)
                    .bind(&item_instance_id)
                    .execute(&mut *conn)
                    .await?;

                    // 5. Insert listing row
                    sqlx::query(
                        r#"
                        INSERT INTO market_listings (
                            listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                            price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                            expires_at_epoch_ms, updated_at_epoch_ms
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'Active', ?, ?, ?)
                        "#,
                    )
                    .bind(&listing_id)
                    .bind(seller_guid as i64)
                    .bind(src_container_id as i64)
                    .bind(&item_instance_id)
                    .bind(item_def_id)
                    .bind(price as i64)
                    .bind(escrow_container_id as i64)
                    .bind(free_slot as i32)
                    .bind(now_ms)
                    .bind(expires_at_epoch_ms)
                    .bind(now_ms)
                    .execute(&mut *conn)
                    .await?;

                    Ok(MarketListingRecord {
                        listing_id,
                        seller_guid,
                        seller_container_id: src_container_id,
                        item_instance_id,
                        item_def_id: item_def_id as u32,
                        price,
                        escrow_container_id,
                        escrow_slot: free_slot,
                        status: ListingStatus::Active,
                        created_at_epoch_ms: now_ms,
                        expires_at_epoch_ms,
                        updated_at_epoch_ms: now_ms,
                    })
                }
                .await;

                match res {
                    Ok(rec) => {
                        sqlx::query("COMMIT").execute(&mut *conn).await?;
                        Ok(rec)
                    }
                    Err(e) => {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                        Err(e)
                    }
                }
            }
            DbBackend::Postgres(pool) => {
                Self::create_listing_postgres(
                    pool,
                    listing_id,
                    seller_guid,
                    src_container_id,
                    src_slot,
                    price,
                    expires_at_epoch_ms,
                    escrow_container_id,
                    now_ms,
                    now_secs,
                )
                .await
            }
        }
    }

    /// Atomically purchases an active listing: transfers payment, delivers the item from escrow
    /// into the buyer's container, and updates the listing status to 'Sold'.
    pub async fn purchase_listing(
        &self,
        buyer_guid: u64,
        listing_id: &str,
        buyer_container_id: u64,
        payment_container_id: u64,
        payment_price: u64,
    ) -> Result<MarketListingRecord, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut conn = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

                let res: Result<MarketListingRecord, StorageError> = async {
                    // 1. Verify buyer ownership of target and payment containers
                    let mut cids = vec![buyer_container_id, payment_container_id];
                    cids.sort_unstable();
                    cids.dedup();

                    let placeholders = vec!["?"; cids.len()].join(", ");
                    let query_str = format!("SELECT container_id, owner_id FROM containers WHERE container_id IN ({})", placeholders);

                    let mut q = sqlx::query(&query_str);
                    for cid in &cids {
                        q = q.bind(*cid as i64);
                    }

                    let rows = q.fetch_all(&mut *conn).await?;
                    if rows.len() != cids.len() {
                        let found_cids: Vec<i64> = rows.iter().map(|r| r.try_get("container_id").unwrap()).collect();
                        let missing = cids.into_iter().find(|cid| !found_cids.contains(&(*cid as i64))).unwrap();
                        return Err(StorageError::ContainerNotFound(missing));
                    }

                    let buyer_guid_str = buyer_guid.to_string();
                    for row in rows {
                        let owner_id: String = row.try_get("owner_id")?;
                        let cid_i64: i64 = row.try_get("container_id")?;
                        if owner_id != buyer_guid_str {
                            return Err(StorageError::UnauthorizedContainerAccess(cid_i64 as u64));
                        }
                    }

                    // 2. Fetch and lock active listing
                    let list_row = sqlx::query(
                        r#"
                        SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                               price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                               expires_at_epoch_ms, updated_at_epoch_ms
                        FROM market_listings WHERE listing_id = ?
                        "#,
                    )
                    .bind(listing_id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(|| StorageError::ListingNotFound(listing_id.to_string()))?;

                    let status_str: String = list_row.try_get("status")?;
                    if status_str != "Active" {
                        return Err(StorageError::ListingNotActive(
                            listing_id.to_string(),
                            status_str,
                        ));
                    }

                    let expires_at: i64 = list_row.try_get("expires_at_epoch_ms")?;
                    if expires_at <= now_ms {
                        return Err(StorageError::ListingNotActive(
                            listing_id.to_string(),
                            "Expired".to_string(),
                        ));
                    }

                    let price: i64 = list_row.try_get("price")?;
                    if payment_price < price as u64 {
                        return Err(StorageError::TradeError(format!(
                            "Insufficient payment: offered {}, required {}",
                            payment_price, price
                        )));
                    }

                    let escrow_container_id: i64 = list_row.try_get("escrow_container_id")?;
                    let escrow_slot: i32 = list_row.try_get("escrow_slot")?;
                    let item_instance_id: String = list_row.try_get("item_instance_id")?;
                    let item_def_id: i32 = list_row.try_get("item_def_id")?;
                    let seller_guid: i64 = list_row.try_get("seller_guid")?;
                    let seller_container_id: i64 = list_row.try_get("seller_container_id")?;
                    let created_at: i64 = list_row.try_get("created_at_epoch_ms")?;

                    // 3. Find free slot in buyer's destination container
                    let c_buyer = sqlx::query(
                        "SELECT capacity FROM containers WHERE container_id = ?",
                    )
                    .bind(buyer_container_id as i64)
                    .fetch_one(&mut *conn)
                    .await?;
                    let buyer_cap: i32 = c_buyer.try_get("capacity")?;

                    let occupied_rows =
                        sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                            .bind(buyer_container_id as i64)
                            .fetch_all(&mut *conn)
                            .await?;
                    let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let free_slot = (0..(buyer_cap as u32))
                        .find(|s| !occupied_slots.contains(s))
                        .ok_or(StorageError::ContainerFull(
                            buyer_container_id,
                            buyer_cap as u32,
                        ))?;

                    // 4. Transfer item from escrow to buyer container
                    sqlx::query(
                        "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
                    )
                    .bind(buyer_container_id as i64)
                    .bind(free_slot as i32)
                    .bind(now_secs)
                    .bind(&item_instance_id)
                    .execute(&mut *conn)
                    .await?;

                    // 5. Update listing status to 'Sold'
                    sqlx::query(
                        "UPDATE market_listings SET status = 'Sold', updated_at_epoch_ms = ? WHERE listing_id = ?",
                    )
                    .bind(now_ms)
                    .bind(listing_id)
                    .execute(&mut *conn)
                    .await?;

                    Ok(MarketListingRecord {
                        listing_id: listing_id.to_string(),
                        seller_guid: seller_guid as u64,
                        seller_container_id: seller_container_id as u64,
                        item_instance_id,
                        item_def_id: item_def_id as u32,
                        price: price as u64,
                        escrow_container_id: escrow_container_id as u64,
                        escrow_slot: escrow_slot as u32,
                        status: ListingStatus::Sold,
                        created_at_epoch_ms: created_at,
                        expires_at_epoch_ms: expires_at,
                        updated_at_epoch_ms: now_ms,
                    })
                }
                .await;

                match res {
                    Ok(rec) => {
                        sqlx::query("COMMIT").execute(&mut *conn).await?;
                        Ok(rec)
                    }
                    Err(e) => {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                        Err(e)
                    }
                }
            }
            DbBackend::Postgres(pool) => {
                Self::purchase_listing_postgres(
                    pool,
                    listing_id,
                    buyer_guid,
                    buyer_container_id,
                    payment_container_id,
                    payment_price,
                    now_ms,
                    now_secs,
                )
                .await
            }
        }
    }

    /// Atomically cancels an active listing: returns the escrowed item back to the seller's container
    /// and marks the listing status as 'Cancelled'.
    pub async fn cancel_listing(
        &self,
        seller_guid: u64,
        listing_id: &str,
        dst_container_id: u64,
    ) -> Result<MarketListingRecord, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut conn = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

                let res: Result<MarketListingRecord, StorageError> = async {
                    // 1. Verify seller ownership of destination container
                    let c_dst = sqlx::query(
                        "SELECT capacity, owner_id FROM containers WHERE container_id = ?",
                    )
                    .bind(dst_container_id as i64)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or(StorageError::ContainerNotFound(dst_container_id))?;
                    let owner_id: String = c_dst.try_get("owner_id")?;
                    if owner_id != seller_guid.to_string() {
                        return Err(StorageError::UnauthorizedContainerAccess(dst_container_id));
                    }
                    let dst_cap: i32 = c_dst.try_get("capacity")?;

                    // 2. Fetch listing
                    let list_row = sqlx::query(
                        r#"
                        SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                               price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                               expires_at_epoch_ms, updated_at_epoch_ms
                        FROM market_listings WHERE listing_id = ?
                        "#,
                    )
                    .bind(listing_id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(|| StorageError::ListingNotFound(listing_id.to_string()))?;

                    let s_guid: i64 = list_row.try_get("seller_guid")?;
                    if s_guid as u64 != seller_guid {
                        return Err(StorageError::UnauthorizedMarketAction(
                            listing_id.to_string(),
                        ));
                    }

                    let status_str: String = list_row.try_get("status")?;
                    if status_str != "Active" {
                        return Err(StorageError::ListingNotActive(
                            listing_id.to_string(),
                            status_str,
                        ));
                    }

                    let item_instance_id: String = list_row.try_get("item_instance_id")?;
                    let item_def_id: i32 = list_row.try_get("item_def_id")?;
                    let price: i64 = list_row.try_get("price")?;
                    let escrow_container_id: i64 = list_row.try_get("escrow_container_id")?;
                    let escrow_slot: i32 = list_row.try_get("escrow_slot")?;
                    let seller_container_id: i64 = list_row.try_get("seller_container_id")?;
                    let created_at: i64 = list_row.try_get("created_at_epoch_ms")?;
                    let expires_at: i64 = list_row.try_get("expires_at_epoch_ms")?;

                    // 3. Find free slot in destination container
                    let occupied_rows =
                        sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                            .bind(dst_container_id as i64)
                            .fetch_all(&mut *conn)
                            .await?;
                    let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let free_slot = (0..(dst_cap as u32))
                        .find(|s| !occupied_slots.contains(s))
                        .ok_or(StorageError::ContainerFull(
                            dst_container_id,
                            dst_cap as u32,
                        ))?;

                    // 4. Move item back to seller container
                    sqlx::query(
                        "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
                    )
                    .bind(dst_container_id as i64)
                    .bind(free_slot as i32)
                    .bind(now_secs)
                    .bind(&item_instance_id)
                    .execute(&mut *conn)
                    .await?;

                    // 5. Mark listing as Cancelled
                    sqlx::query(
                        "UPDATE market_listings SET status = 'Cancelled', updated_at_epoch_ms = ? WHERE listing_id = ?",
                    )
                    .bind(now_ms)
                    .bind(listing_id)
                    .execute(&mut *conn)
                    .await?;

                    Ok(MarketListingRecord {
                        listing_id: listing_id.to_string(),
                        seller_guid,
                        seller_container_id: seller_container_id as u64,
                        item_instance_id,
                        item_def_id: item_def_id as u32,
                        price: price as u64,
                        escrow_container_id: escrow_container_id as u64,
                        escrow_slot: escrow_slot as u32,
                        status: ListingStatus::Cancelled,
                        created_at_epoch_ms: created_at,
                        expires_at_epoch_ms: expires_at,
                        updated_at_epoch_ms: now_ms,
                    })
                }
                .await;

                match res {
                    Ok(rec) => {
                        sqlx::query("COMMIT").execute(&mut *conn).await?;
                        Ok(rec)
                    }
                    Err(e) => {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                        Err(e)
                    }
                }
            }
            DbBackend::Postgres(pool) => {
                Self::cancel_listing_postgres(
                    pool,
                    listing_id,
                    seller_guid,
                    dst_container_id,
                    now_ms,
                    now_secs,
                )
                .await
            }
        }
    }

    /// Queries a single listing by its ID.
    pub async fn get_listing(
        &self,
        listing_id: &str,
    ) -> Result<Option<MarketListingRecord>, StorageError> {
        let query = r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                   price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                   expires_at_epoch_ms, updated_at_epoch_ms
            FROM market_listings WHERE listing_id = ?
        "#;
        let query_str = sql_for_dialect(query, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(listing_id)
                    .fetch_optional(pool)
                    .await?;
                match row {
                    Some(r) => Ok(Some(parse_market_listing!(r))),
                    None => Ok(None),
                }
            }
            DbBackend::Postgres(pool) => {
                Self::get_listing_postgres(pool, &query_str, listing_id).await
            }
        }
    }

    /// Queries active marketplace listings with optional filtering and pagination.
    pub async fn query_listings(
        &self,
        item_def_id: Option<u32>,
        min_price: Option<u64>,
        max_price: Option<u64>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<MarketListingRecord>, StorageError> {
        let now_ms = current_time_ms();
        let mut query = String::from(
            r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                   price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                   expires_at_epoch_ms, updated_at_epoch_ms
            FROM market_listings
            WHERE status = 'Active' AND expires_at_epoch_ms > ?
            "#,
        );

        if item_def_id.is_some() {
            query.push_str(" AND item_def_id = ?");
        }
        if min_price.is_some() {
            query.push_str(" AND price >= ?");
        }
        if max_price.is_some() {
            query.push_str(" AND price <= ?");
        }

        query.push_str(" ORDER BY created_at_epoch_ms DESC LIMIT ? OFFSET ?");
        let query_str = sql_for_dialect(&query, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut q = sqlx::query(&query_str).bind(now_ms);
                if let Some(def_id) = item_def_id {
                    q = q.bind(def_id as i32);
                }
                if let Some(min_p) = min_price {
                    q = q.bind(min_p as i64);
                }
                if let Some(max_p) = max_price {
                    q = q.bind(max_p as i64);
                }
                q = q.bind(limit as i32).bind(offset as i32);
                let rows = q.fetch_all(pool).await?;
                let mut results = Vec::with_capacity(rows.len());
                for r in rows {
                    results.push(parse_market_listing!(r));
                }
                Ok(results)
            }
            DbBackend::Postgres(pool) => {
                Self::query_listings_postgres(
                    pool,
                    &query_str,
                    item_def_id,
                    min_price,
                    max_price,
                    offset,
                    limit,
                    now_ms,
                )
                .await
            }
        }
    }

    /// Finds expired active listings, marks them 'Expired', and atomically refunds items to sellers.
    pub async fn reap_expired_listings(&self, max_batch: u32) -> Result<u32, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();

        let query = r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, escrow_container_id
            FROM market_listings
            WHERE status = 'Active' AND expires_at_epoch_ms <= ?
            LIMIT ?
        "#;
        let query_str = sql_for_dialect(query, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(&query_str)
                    .bind(now_ms)
                    .bind(max_batch as i32)
                    .fetch_all(pool)
                    .await?;
                let mut list = Vec::with_capacity(rows.len());
                for r in rows {
                    list.push(ExpiredCandidate {
                        listing_id: r.try_get("listing_id")?,
                        seller_container_id: r.try_get("seller_container_id")?,
                        item_instance_id: r.try_get("item_instance_id")?,
                    });
                }

                let mut reaped = 0;
                for r in list {
                    let listing_id = r.listing_id;
                    let seller_container_id = r.seller_container_id;
                    let item_instance_id = r.item_instance_id;

                    let mut conn = pool.acquire().await?;
                    let _ = sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await;

                    let c_seller =
                        sqlx::query("SELECT capacity FROM containers WHERE container_id = ?")
                            .bind(seller_container_id)
                            .fetch_optional(&mut *conn)
                            .await?;

                    if let Some(c_row) = c_seller {
                        let cap: i32 = c_row.try_get("capacity")?;
                        let occupied_rows =
                            sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                                .bind(seller_container_id)
                                .fetch_all(&mut *conn)
                                .await?;
                        let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                            .into_iter()
                            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                            .collect();

                        if let Some(free_slot) =
                            (0..(cap as u32)).find(|s| !occupied_slots.contains(s))
                        {
                            let _ = sqlx::query(
                                "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
                            )
                            .bind(seller_container_id)
                            .bind(free_slot as i32)
                            .bind(now_secs)
                            .bind(&item_instance_id)
                            .execute(&mut *conn)
                            .await;
                        }
                    }

                    let res = sqlx::query(
                        "UPDATE market_listings SET status = 'Expired', updated_at_epoch_ms = ? WHERE listing_id = ?",
                    )
                    .bind(now_ms)
                    .bind(&listing_id)
                    .execute(&mut *conn)
                    .await;

                    if res.is_ok() {
                        let _ = sqlx::query("COMMIT").execute(&mut *conn).await;
                        reaped += 1;
                    } else {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    }
                }
                Ok(reaped)
            }
            DbBackend::Postgres(pool) => {
                Self::reap_expired_listings_postgres(pool, max_batch, now_ms, now_secs).await
            }
        }
    }

    /// Spawns a background task that periodically sweeps expired active listings and refunds them.
    pub fn spawn_reaper_loop(repo: Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let _ = repo.reap_expired_listings(50).await;
            }
        })
    }
    #[allow(clippy::too_many_arguments)]
    async fn create_listing_postgres(
        pool: &sqlx::PgPool,
        listing_id: String,
        seller_guid: u64,
        src_container_id: u64,
        src_slot: u32,
        price: u64,
        expires_at_epoch_ms: i64,
        escrow_container_id: u64,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<MarketListingRecord, StorageError> {
        let mut tx = pool.begin().await?;

        let c_src_sql = sql_for_dialect(
            "SELECT container_id, owner_id FROM containers WHERE container_id = ? FOR UPDATE",
            true,
        );
        let c_src_row = sqlx::query::<sqlx::Postgres>(&c_src_sql)
            .bind(src_container_id as i64)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::ContainerNotFound(src_container_id))?;
        let owner_id: String = c_src_row.try_get("owner_id")?;
        if owner_id != seller_guid.to_string() {
            return Err(StorageError::UnauthorizedContainerAccess(src_container_id));
        }

        let c_esc_sql = sql_for_dialect(
            "SELECT container_id, capacity FROM containers WHERE container_id = ? FOR UPDATE",
            true,
        );
        let esc_row = sqlx::query::<sqlx::Postgres>(&c_esc_sql)
            .bind(escrow_container_id as i64)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::ContainerNotFound(escrow_container_id))?;
        let esc_cap: i32 = esc_row.try_get("capacity")?;

        let occ_sql = sql_for_dialect("SELECT slot FROM items WHERE container_id = ?", true);
        let occupied_rows = sqlx::query::<sqlx::Postgres>(&occ_sql)
            .bind(escrow_container_id as i64)
            .fetch_all(&mut *tx)
            .await?;
        let occupied_slots: std::collections::HashSet<u32> = occupied_rows
            .into_iter()
            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
            .collect();

        let free_slot = (0..(esc_cap as u32))
            .find(|s| !occupied_slots.contains(s))
            .ok_or(StorageError::ContainerFull(
                escrow_container_id,
                esc_cap as u32,
            ))?;

        let item_sql = sql_for_dialect(
            "SELECT item_instance_id, item_def_id FROM items WHERE container_id = ? AND slot = ? FOR UPDATE",
            true,
        );
        let item_row = sqlx::query::<sqlx::Postgres>(&item_sql)
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                StorageError::ItemNotFound(format!("slot {src_slot}"), src_container_id)
            })?;
        let item_instance_id: String = item_row.try_get("item_instance_id")?;
        let item_def_id: i32 = item_row.try_get("item_def_id")?;

        let upd_item_sql = sql_for_dialect(
            "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
            true,
        );
        sqlx::query::<sqlx::Postgres>(&upd_item_sql)
            .bind(escrow_container_id as i64)
            .bind(free_slot as i32)
            .bind(now_secs)
            .bind(&item_instance_id)
            .execute(&mut *tx)
            .await?;

        let ins_sql = sql_for_dialect(
            r#"
            INSERT INTO market_listings (
                listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                expires_at_epoch_ms, updated_at_epoch_ms
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'Active', ?, ?, ?)
            "#,
            true,
        );
        sqlx::query::<sqlx::Postgres>(&ins_sql)
            .bind(&listing_id)
            .bind(seller_guid as i64)
            .bind(src_container_id as i64)
            .bind(&item_instance_id)
            .bind(item_def_id)
            .bind(price as i64)
            .bind(escrow_container_id as i64)
            .bind(free_slot as i32)
            .bind(now_ms)
            .bind(expires_at_epoch_ms)
            .bind(now_ms)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(MarketListingRecord {
            listing_id,
            seller_guid,
            seller_container_id: src_container_id,
            item_instance_id,
            item_def_id: item_def_id as u32,
            price,
            escrow_container_id,
            escrow_slot: free_slot,
            status: ListingStatus::Active,
            created_at_epoch_ms: now_ms,
            expires_at_epoch_ms,
            updated_at_epoch_ms: now_ms,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn purchase_listing_postgres(
        pool: &sqlx::PgPool,
        listing_id: &str,
        buyer_guid: u64,
        buyer_container_id: u64,
        payment_container_id: u64,
        payment_price: u64,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<MarketListingRecord, StorageError> {
        let mut tx = pool.begin().await?;

        let mut cids = vec![buyer_container_id, payment_container_id];
        cids.sort_unstable();
        cids.dedup();

        let mut placeholders = Vec::new();
        for i in 1..=cids.len() {
            placeholders.push(format!("${}", i));
        }
        let in_clause = placeholders.join(", ");
        let query_str = format!("SELECT container_id, owner_id FROM containers WHERE container_id IN ({}) FOR UPDATE", in_clause);

        let mut q = sqlx::query::<sqlx::Postgres>(&query_str);
        for cid in &cids {
            q = q.bind(*cid as i64);
        }

        let rows = q.fetch_all(&mut *tx).await?;
        if rows.len() != cids.len() {
            let found_cids: Vec<i64> = rows.iter().map(|r| r.try_get("container_id").unwrap()).collect();
            let missing = cids.into_iter().find(|cid| !found_cids.contains(&(*cid as i64))).unwrap();
            return Err(StorageError::ContainerNotFound(missing));
        }

        let buyer_guid_str = buyer_guid.to_string();
        for row in rows {
            let owner_id: String = row.try_get("owner_id")?;
            let cid_i64: i64 = row.try_get("container_id")?;
            if owner_id != buyer_guid_str {
                return Err(StorageError::UnauthorizedContainerAccess(cid_i64 as u64));
            }
        }

        let list_sql = sql_for_dialect(
            r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                   price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                   expires_at_epoch_ms, updated_at_epoch_ms
            FROM market_listings WHERE listing_id = ? FOR UPDATE
            "#,
            true,
        );
        let list_row = sqlx::query::<sqlx::Postgres>(&list_sql)
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::ListingNotFound(listing_id.to_string()))?;

        let status_str: String = list_row.try_get("status")?;
        if status_str != "Active" {
            return Err(StorageError::ListingNotActive(
                listing_id.to_string(),
                status_str,
            ));
        }

        let expires_at: i64 = list_row.try_get("expires_at_epoch_ms")?;
        if expires_at <= now_ms {
            return Err(StorageError::ListingNotActive(
                listing_id.to_string(),
                "Expired".to_string(),
            ));
        }

        let price: i64 = list_row.try_get("price")?;
        if payment_price < price as u64 {
            return Err(StorageError::TradeError(format!(
                "Insufficient payment: offered {}, required {}",
                payment_price, price
            )));
        }

        let escrow_container_id: i64 = list_row.try_get("escrow_container_id")?;
        let escrow_slot: i32 = list_row.try_get("escrow_slot")?;
        let item_instance_id: String = list_row.try_get("item_instance_id")?;
        let item_def_id: i32 = list_row.try_get("item_def_id")?;
        let seller_guid: i64 = list_row.try_get("seller_guid")?;
        let seller_container_id: i64 = list_row.try_get("seller_container_id")?;
        let created_at: i64 = list_row.try_get("created_at_epoch_ms")?;

        let c_buyer_sql = sql_for_dialect(
            "SELECT capacity FROM containers WHERE container_id = ? FOR UPDATE",
            true,
        );
        let c_buyer = sqlx::query::<sqlx::Postgres>(&c_buyer_sql)
            .bind(buyer_container_id as i64)
            .fetch_one(&mut *tx)
            .await?;
        let buyer_cap: i32 = c_buyer.try_get("capacity")?;

        let occ_sql = sql_for_dialect("SELECT slot FROM items WHERE container_id = ?", true);
        let occupied_rows = sqlx::query::<sqlx::Postgres>(&occ_sql)
            .bind(buyer_container_id as i64)
            .fetch_all(&mut *tx)
            .await?;
        let occupied_slots: std::collections::HashSet<u32> = occupied_rows
            .into_iter()
            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
            .collect();

        let free_slot = (0..(buyer_cap as u32))
            .find(|s| !occupied_slots.contains(s))
            .ok_or(StorageError::ContainerFull(
                buyer_container_id,
                buyer_cap as u32,
            ))?;

        let upd_item_sql = sql_for_dialect(
            "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
            true,
        );
        sqlx::query::<sqlx::Postgres>(&upd_item_sql)
            .bind(buyer_container_id as i64)
            .bind(free_slot as i32)
            .bind(now_secs)
            .bind(&item_instance_id)
            .execute(&mut *tx)
            .await?;

        let upd_list_sql = sql_for_dialect(
            "UPDATE market_listings SET status = 'Sold', updated_at_epoch_ms = ? WHERE listing_id = ?",
            true,
        );
        sqlx::query::<sqlx::Postgres>(&upd_list_sql)
            .bind(now_ms)
            .bind(listing_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(MarketListingRecord {
            listing_id: listing_id.to_string(),
            seller_guid: seller_guid as u64,
            seller_container_id: seller_container_id as u64,
            item_instance_id,
            item_def_id: item_def_id as u32,
            price: price as u64,
            escrow_container_id: escrow_container_id as u64,
            escrow_slot: escrow_slot as u32,
            status: ListingStatus::Sold,
            created_at_epoch_ms: created_at,
            expires_at_epoch_ms: expires_at,
            updated_at_epoch_ms: now_ms,
        })
    }

    async fn cancel_listing_postgres(
        pool: &sqlx::PgPool,
        listing_id: &str,
        seller_guid: u64,
        dst_container_id: u64,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<MarketListingRecord, StorageError> {
        let mut tx = pool.begin().await?;

        let c_dst_sql = sql_for_dialect(
            "SELECT capacity, owner_id FROM containers WHERE container_id = ? FOR UPDATE",
            true,
        );
        let c_dst = sqlx::query::<sqlx::Postgres>(&c_dst_sql)
            .bind(dst_container_id as i64)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::ContainerNotFound(dst_container_id))?;
        let owner_id: String = c_dst.try_get("owner_id")?;
        if owner_id != seller_guid.to_string() {
            return Err(StorageError::UnauthorizedContainerAccess(dst_container_id));
        }
        let dst_cap: i32 = c_dst.try_get("capacity")?;

        let list_sql = sql_for_dialect(
            r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                   price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                   expires_at_epoch_ms, updated_at_epoch_ms
            FROM market_listings WHERE listing_id = ? FOR UPDATE
            "#,
            true,
        );
        let list_row = sqlx::query::<sqlx::Postgres>(&list_sql)
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::ListingNotFound(listing_id.to_string()))?;

        let s_guid: i64 = list_row.try_get("seller_guid")?;
        if s_guid as u64 != seller_guid {
            return Err(StorageError::UnauthorizedMarketAction(
                listing_id.to_string(),
            ));
        }

        let status_str: String = list_row.try_get("status")?;
        if status_str != "Active" {
            return Err(StorageError::ListingNotActive(
                listing_id.to_string(),
                status_str,
            ));
        }

        let item_instance_id: String = list_row.try_get("item_instance_id")?;
        let item_def_id: i32 = list_row.try_get("item_def_id")?;
        let price: i64 = list_row.try_get("price")?;
        let escrow_container_id: i64 = list_row.try_get("escrow_container_id")?;
        let escrow_slot: i32 = list_row.try_get("escrow_slot")?;
        let seller_container_id: i64 = list_row.try_get("seller_container_id")?;
        let created_at: i64 = list_row.try_get("created_at_epoch_ms")?;
        let expires_at: i64 = list_row.try_get("expires_at_epoch_ms")?;

        let occ_sql = sql_for_dialect("SELECT slot FROM items WHERE container_id = ?", true);
        let occupied_rows = sqlx::query::<sqlx::Postgres>(&occ_sql)
            .bind(dst_container_id as i64)
            .fetch_all(&mut *tx)
            .await?;
        let occupied_slots: std::collections::HashSet<u32> = occupied_rows
            .into_iter()
            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
            .collect();

        let free_slot = (0..(dst_cap as u32))
            .find(|s| !occupied_slots.contains(s))
            .ok_or(StorageError::ContainerFull(
                dst_container_id,
                dst_cap as u32,
            ))?;

        let upd_item_sql = sql_for_dialect(
            "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
            true,
        );
        sqlx::query::<sqlx::Postgres>(&upd_item_sql)
            .bind(dst_container_id as i64)
            .bind(free_slot as i32)
            .bind(now_secs)
            .bind(&item_instance_id)
            .execute(&mut *tx)
            .await?;

        let upd_list_sql = sql_for_dialect(
            "UPDATE market_listings SET status = 'Cancelled', updated_at_epoch_ms = ? WHERE listing_id = ?",
            true,
        );
        sqlx::query::<sqlx::Postgres>(&upd_list_sql)
            .bind(now_ms)
            .bind(listing_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(MarketListingRecord {
            listing_id: listing_id.to_string(),
            seller_guid,
            seller_container_id: seller_container_id as u64,
            item_instance_id,
            item_def_id: item_def_id as u32,
            price: price as u64,
            escrow_container_id: escrow_container_id as u64,
            escrow_slot: escrow_slot as u32,
            status: ListingStatus::Cancelled,
            created_at_epoch_ms: created_at,
            expires_at_epoch_ms: expires_at,
            updated_at_epoch_ms: now_ms,
        })
    }

    async fn get_listing_postgres(
        pool: &sqlx::PgPool,
        query_str: &str,
        listing_id: &str,
    ) -> Result<Option<MarketListingRecord>, StorageError> {
        let row = sqlx::query::<sqlx::Postgres>(query_str)
            .bind(listing_id)
            .fetch_optional(pool)
            .await?;
        match row {
            Some(r) => Ok(Some(parse_market_listing!(r))),
            None => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn query_listings_postgres(
        pool: &sqlx::PgPool,
        query_str: &str,
        item_def_id: Option<u32>,
        min_price: Option<u64>,
        max_price: Option<u64>,
        offset: u32,
        limit: u32,
        now_ms: i64,
    ) -> Result<Vec<MarketListingRecord>, StorageError> {
        let mut q = sqlx::query::<sqlx::Postgres>(query_str).bind(now_ms);
        if let Some(def_id) = item_def_id {
            q = q.bind(def_id as i32);
        }
        if let Some(min_p) = min_price {
            q = q.bind(min_p as i64);
        }
        if let Some(max_p) = max_price {
            q = q.bind(max_p as i64);
        }
        q = q.bind(limit as i32).bind(offset as i32);
        let rows = q.fetch_all(pool).await?;
        let mut results = Vec::with_capacity(rows.len());
        for r in rows {
            results.push(parse_market_listing!(r));
        }
        Ok(results)
    }

    async fn reap_expired_listings_postgres(
        pool: &sqlx::PgPool,
        max_batch: u32,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<u32, StorageError> {
        let query = r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, escrow_container_id
            FROM market_listings
            WHERE status = 'Active' AND expires_at_epoch_ms <= ?
            LIMIT ?
        "#;
        let query_str = sql_for_dialect(query, true);
        let rows = sqlx::query::<sqlx::Postgres>(&query_str)
            .bind(now_ms)
            .bind(max_batch as i32)
            .fetch_all(pool)
            .await?;
        let mut list = Vec::with_capacity(rows.len());
        for r in rows {
            list.push(ExpiredCandidate {
                listing_id: r.try_get::<String, _>("listing_id")?,
                seller_container_id: r.try_get::<i64, _>("seller_container_id")?,
                item_instance_id: r.try_get::<String, _>("item_instance_id")?,
            });
        }

        let mut reaped = 0;
        for r in list {
            let listing_id = r.listing_id;
            let seller_container_id = r.seller_container_id;
            let item_instance_id = r.item_instance_id;

            if let Ok(mut tx) = pool.begin().await {
                let c_seller_sql = sql_for_dialect(
                    "SELECT capacity FROM containers WHERE container_id = ? FOR UPDATE",
                    true,
                );
                if let Ok(Some(c_row)) = sqlx::query::<sqlx::Postgres>(&c_seller_sql)
                    .bind(seller_container_id)
                    .fetch_optional(&mut *tx)
                    .await
                {
                    let cap: i32 = c_row.try_get("capacity")?;
                    let occ_sql =
                        sql_for_dialect("SELECT slot FROM items WHERE container_id = ?", true);
                    if let Ok(occupied_rows) = sqlx::query::<sqlx::Postgres>(&occ_sql)
                        .bind(seller_container_id)
                        .fetch_all(&mut *tx)
                        .await
                    {
                        let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                            .into_iter()
                            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                            .collect();

                        if let Some(free_slot) =
                            (0..(cap as u32)).find(|s| !occupied_slots.contains(s))
                        {
                            let upd_item = sql_for_dialect(
                                "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?",
                                true,
                            );
                            let _ = sqlx::query::<sqlx::Postgres>(&upd_item)
                                .bind(seller_container_id)
                                .bind(free_slot as i32)
                                .bind(now_secs)
                                .bind(&item_instance_id)
                                .execute(&mut *tx)
                                .await;
                        }
                    }
                }

                let upd_sql = sql_for_dialect(
                    "UPDATE market_listings SET status = 'Expired', updated_at_epoch_ms = ? WHERE listing_id = ?",
                    true,
                );
                if sqlx::query::<sqlx::Postgres>(&upd_sql)
                    .bind(now_ms)
                    .bind(&listing_id)
                    .execute(&mut *tx)
                    .await
                    .is_ok()
                    && tx.commit().await.is_ok()
                {
                    reaped += 1;
                }
            }
        }

        Ok(reaped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContainerRecord;

    async fn setup_test_db() -> (Database, MarketplaceRepository) {
        let db = Database::connect_memory().await.unwrap();
        let repo = MarketplaceRepository::new(db.clone());
        (db, repo)
    }

    async fn create_container(db: &Database, container_id: u64, owner_id: &str, capacity: u32) {
        let rec = ContainerRecord {
            container_id,
            owner_type: "player".into(),
            owner_id: owner_id.into(),
            name: "Inventory".into(),
            capacity,
            access_policy: "private".into(),
            version: 1,
            created_at: 100,
            updated_at: 100,
        };
        crate::container_repo::ContainerRepository::new(db.clone())
            .create_container(&rec)
            .await
            .unwrap();
    }

    async fn create_item(
        db: &Database,
        item_id: &str,
        container_id: u64,
        slot: u32,
        item_def_id: u32,
    ) {
        match db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO items (item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at)
                    VALUES (?, 0, ?, ?, ?, 1, 100, '{}', 100, 100)
                    "#,
                )
                .bind(item_id)
                .bind(container_id as i64)
                .bind(item_def_id as i32)
                .bind(slot as i32)
                .execute(pool)
                .await
                .unwrap();
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO items (item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at)
                    VALUES ($1, 0, $2, $3, $4, 1, 100, '{}', 100, 100)
                    "#,
                )
                .bind(item_id)
                .bind(container_id as i64)
                .bind(item_def_id as i32)
                .bind(slot as i32)
                .execute(pool)
                .await
                .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_marketplace_listing_and_purchase_lifecycle() {
        let (db, repo) = setup_test_db().await;

        let seller_guid = 1001;
        let buyer_guid = 2002;
        let seller_container = 10;
        let escrow_container = 99;
        let buyer_container = 20;
        let payment_container = 21;

        create_container(&db, seller_container, "1001", 10).await;
        create_container(&db, escrow_container, "system", 50).await;
        create_container(&db, buyer_container, "2002", 10).await;
        create_container(&db, payment_container, "2002", 10).await;

        create_item(&db, "item-sword-1", seller_container, 0, 500).await;

        let now_ms = current_time_ms();
        // 1. Create listing
        let listing = repo
            .create_listing(
                seller_guid,
                seller_container,
                0,
                1500,
                3600,
                escrow_container,
            )
            .await
            .unwrap();

        assert_eq!(listing.seller_guid, seller_guid);
        assert_eq!(listing.price, 1500);
        assert_eq!(listing.status, ListingStatus::Active);
        assert!(
            listing.expires_at_epoch_ms >= now_ms + 3_500_000
                && listing.expires_at_epoch_ms <= now_ms + 3_700_000
        );

        // Verify item was moved into escrow container
        let crepo = crate::container_repo::ContainerRepository::new(db.clone());
        let escrow_items = crepo.get_container_items(escrow_container).await.unwrap();
        assert_eq!(escrow_items.len(), 1);
        assert_eq!(escrow_items[0].item_instance_id, "item-sword-1");

        // 2. Query listing
        let results = repo
            .query_listings(Some(500), None, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].listing_id, listing.listing_id);

        // 2b. Attempt purchase with insufficient payment (1499 < 1500)
        let underpaid = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                1499,
            )
            .await;
        assert!(matches!(underpaid, Err(StorageError::TradeError(_))));

        // 3. Purchase listing with exact payment
        let purchased = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                1500,
            )
            .await
            .unwrap();

        assert_eq!(purchased.status, ListingStatus::Sold);

        // Verify item is now in buyer container
        let buyer_items = crepo.get_container_items(buyer_container).await.unwrap();
        assert_eq!(buyer_items.len(), 1);
        assert_eq!(buyer_items[0].item_instance_id, "item-sword-1");

        // Query active listings should now be empty
        let active_results = repo.query_listings(None, None, None, 0, 10).await.unwrap();
        assert!(active_results.is_empty());
    }

    #[tokio::test]
    async fn test_marketplace_listing_cancellation() {
        let (db, repo) = setup_test_db().await;

        let seller_guid = 1001;
        let seller_container = 10;
        let escrow_container = 99;

        create_container(&db, seller_container, "1001", 10).await;
        create_container(&db, escrow_container, "system", 50).await;

        create_item(&db, "item-shield-1", seller_container, 1, 600).await;

        let listing = repo
            .create_listing(
                seller_guid,
                seller_container,
                1,
                2000,
                3600,
                escrow_container,
            )
            .await
            .unwrap();

        let cancelled = repo
            .cancel_listing(seller_guid, &listing.listing_id, seller_container)
            .await
            .unwrap();

        assert_eq!(cancelled.status, ListingStatus::Cancelled);

        // Item must be back in seller container
        let crepo = crate::container_repo::ContainerRepository::new(db.clone());
        let seller_items = crepo.get_container_items(seller_container).await.unwrap();
        assert_eq!(seller_items.len(), 1);
        assert_eq!(seller_items[0].item_instance_id, "item-shield-1");
    }

    #[tokio::test]
    async fn test_marketplace_unauthorized_container_checks() {
        let (db, repo) = setup_test_db().await;

        create_container(&db, 10, "1001", 10).await;
        create_container(&db, 99, "system", 50).await;
        create_item(&db, "item-1", 10, 0, 100).await;

        // Imposter trying to list someone else's container
        let imposter_guid = 9999;
        let err = repo
            .create_listing(imposter_guid, 10, 0, 500, 3600, 99)
            .await
            .unwrap_err();

        match err {
            StorageError::UnauthorizedContainerAccess(cid) => assert_eq!(cid, 10),
            other => panic!("Expected UnauthorizedContainerAccess, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_marketplace_reap_expired_listings() {
        let (db, repo) = setup_test_db().await;

        let seller_guid = 1001;
        let seller_container = 10;
        let escrow_container = 99;

        create_container(&db, seller_container, "1001", 10).await;
        create_container(&db, escrow_container, "system", 50).await;
        create_item(&db, "item-expiring", seller_container, 0, 700).await;

        // Create listing with 0 duration so it expires immediately
        let listing = repo
            .create_listing(seller_guid, seller_container, 0, 100, 0, escrow_container)
            .await
            .unwrap();

        // Wait small delay
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let reaped = repo.reap_expired_listings(10).await.unwrap();
        assert_eq!(reaped, 1);

        let fetched = repo
            .get_listing(&listing.listing_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.status, ListingStatus::Expired);

        // Item returned to seller
        let crepo = crate::container_repo::ContainerRepository::new(db.clone());
        let items = crepo.get_container_items(seller_container).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_instance_id, "item-expiring");
    }

    #[test]
    fn test_time_helpers() {
        let ms = current_time_ms();
        let secs = current_time_secs();
        assert!(secs > 1_700_000_000);
        assert!(ms > 1_700_000_000_000);
        assert!(ms > secs * 900);
        assert!(ms >= secs * 1000);
    }
}
