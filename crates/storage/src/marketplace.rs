//! Asynchronous Global Marketplace Service and Transactional Repository.
//!
//! Provides atomic, isolated listing (with escrow backing), purchasing, cancellation,
//! expiration reaping, and paginated queries completely outside spatial simulation loops.

use std::sync::Arc;
use std::time::Duration;

use sqlx::Row;

use crate::db::{
    current_time_ms, current_time_secs, find_first_free_slot, resolve_container_owner_guid,
    Database, DbBackend, StorageError,
};
pub use crate::models::{ListingStatus, MarketListingRecord};
use crate::with_dialect_tx;

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

struct ExpiredCandidate {
    listing_id: String,
    seller_container_id: i64,
    escrow_container_id: i64,
    escrow_slot: i32,
    item_instance_id: String,
}

/// Default `item_def_id` used for stackable currency items in marketplace settlements.
pub const DEFAULT_CURRENCY_ITEM_DEF_ID: u32 = 1;

/// Repository managing asynchronous marketplace transactions and listings.
#[derive(Clone)]
pub struct MarketplaceRepository {
    db: Database,
    currency_item_def_id: u32,
}

const MAX_LISTING_DURATION_SECS: u64 = 2_592_000; // 30 days

impl MarketplaceRepository {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            currency_item_def_id: DEFAULT_CURRENCY_ITEM_DEF_ID,
        }
    }

    /// Configures the `item_def_id` treated as currency during marketplace purchases.
    pub fn with_currency_item_def_id(mut self, currency_item_def_id: u32) -> Self {
        self.currency_item_def_id = currency_item_def_id;
        self
    }

    /// Returns the configured currency `item_def_id`.
    pub fn currency_item_def_id(&self) -> u32 {
        self.currency_item_def_id
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
        let duration_secs = duration_secs.min(MAX_LISTING_DURATION_SECS);
        let expires_at_epoch_ms =
            now_ms.saturating_add((duration_secs as i64).saturating_mul(1000));
        let listing_id = format!("list-{}-{}-{}", seller_guid, src_slot, now_ms);

        if src_container_id == escrow_container_id {
            return Err(StorageError::InvalidParameter(
                "Source and escrow containers must be distinct".to_string(),
            ));
        }

        let (first_cid, second_cid) = if src_container_id < escrow_container_id {
            (src_container_id, escrow_container_id)
        } else {
            (escrow_container_id, src_container_id)
        };

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Verify seller ownership of source container and lock containers in ascending order
            let c_lock_sql = format!(
                "SELECT container_id, owner_type, owner_id, capacity FROM containers WHERE container_id = ?{}",
                lock_suffix!()
            );
            let row_first = sqlx::query(&q!(&c_lock_sql))
                .bind(first_cid as i64)
                .fetch_optional(&mut *conn)
                .await?;
            let row_second = sqlx::query(&q!(&c_lock_sql))
                .bind(second_cid as i64)
                .fetch_optional(&mut *conn)
                .await?;

            let (c_src, c_esc) = if src_container_id < escrow_container_id {
                (row_first, row_second)
            } else {
                (row_second, row_first)
            };

            let c_src_row = c_src.ok_or(StorageError::ContainerNotFound(src_container_id))?;
            let owner_id: String = c_src_row.try_get("owner_id")?;
            if owner_id != seller_guid.to_string() {
                return Err(StorageError::UnauthorizedContainerAccess(src_container_id));
            }

            // 2. Verify escrow container exists and find free unreserved slot
            let esc_row = c_esc.ok_or(StorageError::ContainerNotFound(escrow_container_id))?;
            let esc_cap: i32 = esc_row.try_get("capacity")?;
            let esc_owner_type: String = esc_row.try_get("owner_type")?;
            let esc_owner_id: String = esc_row.try_get("owner_id")?;
            let esc_owner_guid = resolve_container_owner_guid(&esc_owner_type, &esc_owner_id);

            let occupied_rows = sqlx::query(&q!("SELECT slot FROM items WHERE container_id = ?"))
                .bind(escrow_container_id as i64)
                .fetch_all(&mut *conn)
                .await?;
            let mut occupied_slots: std::collections::HashSet<u32> = occupied_rows
                .into_iter()
                .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                .collect();

            let esc_reserved_rows = sqlx::query(&q!(
                "SELECT slot FROM container_reservations WHERE container_id = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(escrow_container_id as i64)
            .bind(now_ms)
            .fetch_all(&mut *conn)
            .await?;
            for r in esc_reserved_rows {
                if let Ok(slot) = r.try_get::<i32, _>("slot") {
                    occupied_slots.insert(slot as u32);
                }
            }

            let free_slot = find_first_free_slot(&occupied_slots, esc_cap as u32).ok_or(
                StorageError::ContainerFull(escrow_container_id, esc_cap as u32),
            )?;

            // 3. Verify item exists in seller container at slot and is not locked by 2PC reservations
            let item_sql = format!(
                "SELECT item_instance_id, item_def_id FROM items WHERE container_id = ? AND slot = ?{}",
                lock_suffix!()
            );
            let item_row = sqlx::query(&q!(&item_sql))
                .bind(src_container_id as i64)
                .bind(src_slot as i32)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| {
                    StorageError::ItemNotFound(format!("slot {src_slot}"), src_container_id)
                })?;

            let item_instance_id: String = item_row.try_get("item_instance_id")?;
            let item_def_id: i32 = item_row.try_get("item_def_id")?;

            let active_res = sqlx::query(&q!(
                "SELECT 1 FROM container_reservations WHERE ((container_id = ? AND slot = ?) OR item_instance_id = ?) AND expires_at_epoch_ms > ?"
            ))
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .bind(&item_instance_id)
            .bind(now_ms)
            .fetch_optional(&mut *conn)
            .await?;
            if active_res.is_some() {
                return Err(StorageError::SlotReserved(src_slot, src_container_id));
            }

            // 4. Move item to escrow container and sync character_guid
            let move_res = sqlx::query(&q!(
                "UPDATE items SET container_id = ?, slot = ?, character_guid = ?, updated_at = ? WHERE item_instance_id = ? AND container_id = ? AND slot = ?"
            ))
            .bind(escrow_container_id as i64)
            .bind(free_slot as i32)
            .bind(esc_owner_guid)
            .bind(now_secs)
            .bind(&item_instance_id)
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .execute(&mut *conn)
            .await?;

            if move_res.rows_affected() != 1 {
                return Err(StorageError::ItemNotFound(
                    item_instance_id,
                    src_container_id,
                ));
            }

            // 5. Insert listing row
            sqlx::query(&q!(r#"
                INSERT INTO market_listings (
                    listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                    price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                    expires_at_epoch_ms, updated_at_epoch_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'Active', ?, ?, ?)
                "#))
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
                listing_id: listing_id.clone(),
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
        })
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
        let currency_def_id = self.currency_item_def_id as i32;

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Fetch and lock listing first so lock acquisition order is always:
            //    market_listings -> containers (ascending container_id)
            let list_sql = format!(
                r#"
                SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                       price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                       expires_at_epoch_ms, updated_at_epoch_ms
                FROM market_listings WHERE listing_id = ?{}
                "#,
                lock_suffix!()
            );
            let list_row = sqlx::query(&q!(&list_sql))
                .bind(listing_id)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| StorageError::ListingNotFound(listing_id.to_string()))?;

            // 2. Verify listing is Active, unexpired, and price is valid
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
            if price < 0 {
                return Err(StorageError::TradeError(format!(
                    "Invalid listing price: {}",
                    price
                )));
            }
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

            // 3. Lock all involved containers in ascending container_id order via individual queries
            let mut cids = vec![
                buyer_container_id,
                payment_container_id,
                escrow_container_id as u64,
            ];
            if price > 0 {
                cids.push(seller_container_id as u64);
            }
            cids.sort_unstable();
            cids.dedup();

            let container_sql = format!(
                "SELECT container_id, owner_type, owner_id, capacity FROM containers WHERE container_id = ?{}",
                lock_suffix!()
            );
            let mut buyer_cap: i32 = 0;
            let mut buyer_owner_guid: i64 = 0;
            let mut seller_cap: i32 = 0;
            let mut seller_owner_guid: i64 = 0;
            let buyer_guid_str = buyer_guid.to_string();

            for cid in cids {
                let row = sqlx::query(&q!(&container_sql))
                    .bind(cid as i64)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or(StorageError::ContainerNotFound(cid))?;

                let owner_type: String = row.try_get("owner_type")?;
                let owner_id: String = row.try_get("owner_id")?;
                let capacity: i32 = row.try_get("capacity")?;

                if (cid == buyer_container_id || cid == payment_container_id)
                    && owner_id != buyer_guid_str
                {
                    return Err(StorageError::UnauthorizedContainerAccess(cid));
                }
                if cid == buyer_container_id {
                    buyer_cap = capacity;
                    buyer_owner_guid = resolve_container_owner_guid(&owner_type, &owner_id);
                }
                if cid == seller_container_id as u64 {
                    seller_cap = capacity;
                    seller_owner_guid = resolve_container_owner_guid(&owner_type, &owner_id);
                }
            }

            // 4. Reject if escrow item or slot has an active 2PC reservation
            let esc_res = sqlx::query(&q!(
                "SELECT 1 FROM container_reservations WHERE ((container_id = ? AND slot = ?) OR item_instance_id = ?) AND expires_at_epoch_ms > ?"
            ))
            .bind(escrow_container_id)
            .bind(escrow_slot)
            .bind(&item_instance_id)
            .bind(now_ms)
            .fetch_optional(&mut *conn)
            .await?;
            if esc_res.is_some() {
                return Err(StorageError::SlotReserved(
                    escrow_slot as u32,
                    escrow_container_id as u64,
                ));
            }

            // 5. Authoritatively verify & deduct currency from payment_container_id, then credit seller
            if price > 0 {
                let pay_res_rows = sqlx::query(&q!(
                    "SELECT slot FROM container_reservations WHERE container_id = ? AND expires_at_epoch_ms > ?"
                ))
                .bind(payment_container_id as i64)
                .bind(now_ms)
                .fetch_all(&mut *conn)
                .await?;
                let pay_reserved_slots: std::collections::HashSet<i32> = pay_res_rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<i32, _>("slot").ok())
                    .collect();

                let cur_sql = format!(
                    "SELECT item_instance_id, slot, quantity FROM items WHERE container_id = ? AND item_def_id = ? ORDER BY slot ASC{}",
                    lock_suffix!()
                );
                let cur_rows = sqlx::query(&q!(&cur_sql))
                    .bind(payment_container_id as i64)
                    .bind(currency_def_id)
                    .fetch_all(&mut *conn)
                    .await?;

                let mut spendable_stacks: Vec<(String, i32, i32)> = Vec::new();
                let mut available_unreserved: i128 = 0;
                let mut reserved_funds: i128 = 0;
                let mut first_reserved_slot: Option<i32> = None;

                for r in cur_rows {
                    let stack_id: String = r.try_get("item_instance_id")?;
                    let slot: i32 = r.try_get("slot")?;
                    let qty: i32 = r.try_get("quantity")?;
                    if qty <= 0 {
                        continue;
                    }

                    let item_reserved = if pay_reserved_slots.contains(&slot) {
                        true
                    } else {
                        sqlx::query(&q!(
                            "SELECT 1 FROM container_reservations WHERE item_instance_id = ? AND expires_at_epoch_ms > ?"
                        ))
                        .bind(&stack_id)
                        .bind(now_ms)
                        .fetch_optional(&mut *conn)
                        .await?
                        .is_some()
                    };

                    if item_reserved {
                        reserved_funds += qty as i128;
                        if first_reserved_slot.is_none() {
                            first_reserved_slot = Some(slot);
                        }
                    } else {
                        available_unreserved += qty as i128;
                        spendable_stacks.push((stack_id, slot, qty));
                    }
                }

                let required = price as i128;
                if available_unreserved < required {
                    if available_unreserved + reserved_funds >= required {
                        if let Some(r_slot) = first_reserved_slot {
                            return Err(StorageError::SlotReserved(
                                r_slot as u32,
                                payment_container_id,
                            ));
                        }
                    }
                    return Err(StorageError::TradeError(format!(
                        "Insufficient funds in payment container {}: available {}, required {}",
                        payment_container_id, available_unreserved, price
                    )));
                }

                let mut remaining = required;
                for (stack_id, _slot, qty) in spendable_stacks {
                    if remaining == 0 {
                        break;
                    }
                    let qty_128 = qty as i128;
                    if qty_128 <= remaining {
                        let del_res = sqlx::query(&q!(
                            "DELETE FROM items WHERE item_instance_id = ? AND container_id = ?"
                        ))
                        .bind(&stack_id)
                        .bind(payment_container_id as i64)
                        .execute(&mut *conn)
                        .await?;
                        if del_res.rows_affected() != 1 {
                            return Err(StorageError::ItemNotFound(stack_id, payment_container_id));
                        }
                        remaining -= qty_128;
                    } else {
                        let new_qty = (qty_128 - remaining) as i32;
                        let upd_res = sqlx::query(&q!(
                            "UPDATE items SET quantity = ?, updated_at = ? WHERE item_instance_id = ? AND container_id = ?"
                        ))
                        .bind(new_qty)
                        .bind(now_secs)
                        .bind(&stack_id)
                        .bind(payment_container_id as i64)
                        .execute(&mut *conn)
                        .await?;
                        if upd_res.rows_affected() != 1 {
                            return Err(StorageError::ItemNotFound(stack_id, payment_container_id));
                        }
                        remaining = 0;
                    }
                }
            }

            // 6. Find unreserved free slot in buyer's destination container
            let occupied_rows = sqlx::query(&q!("SELECT slot FROM items WHERE container_id = ?"))
                .bind(buyer_container_id as i64)
                .fetch_all(&mut *conn)
                .await?;
            let mut occupied_slots: std::collections::HashSet<u32> = occupied_rows
                .into_iter()
                .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                .collect();

            let buyer_reserved_rows = sqlx::query(&q!(
                "SELECT slot FROM container_reservations WHERE container_id = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(buyer_container_id as i64)
            .bind(now_ms)
            .fetch_all(&mut *conn)
            .await?;
            for r in buyer_reserved_rows {
                if let Ok(slot) = r.try_get::<i32, _>("slot") {
                    occupied_slots.insert(slot as u32);
                }
            }

            let free_slot = find_first_free_slot(&occupied_slots, buyer_cap as u32).ok_or(
                StorageError::ContainerFull(buyer_container_id, buyer_cap as u32),
            )?;

            // 7. Transfer item from escrow to buyer container and sync character_guid
            let move_res = sqlx::query(&q!(
                "UPDATE items SET container_id = ?, slot = ?, character_guid = ?, updated_at = ? WHERE item_instance_id = ? AND container_id = ? AND slot = ?"
            ))
            .bind(buyer_container_id as i64)
            .bind(free_slot as i32)
            .bind(buyer_owner_guid)
            .bind(now_secs)
            .bind(&item_instance_id)
            .bind(escrow_container_id)
            .bind(escrow_slot)
            .execute(&mut *conn)
            .await?;

            if move_res.rows_affected() != 1 {
                return Err(StorageError::ItemNotFound(
                    item_instance_id,
                    escrow_container_id as u64,
                ));
            }

            // 8. Credit seller currency into seller_container_id when price > 0
            if price > 0 {
                let seller_res_rows = sqlx::query(&q!(
                    "SELECT slot FROM container_reservations WHERE container_id = ? AND expires_at_epoch_ms > ?"
                ))
                .bind(seller_container_id)
                .bind(now_ms)
                .fetch_all(&mut *conn)
                .await?;
                let seller_reserved_slots: std::collections::HashSet<i32> = seller_res_rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<i32, _>("slot").ok())
                    .collect();

                let seller_cur_sql = format!(
                    "SELECT item_instance_id, slot, quantity FROM items WHERE container_id = ? AND item_def_id = ? ORDER BY slot ASC{}",
                    lock_suffix!()
                );
                let seller_cur_rows = sqlx::query(&q!(&seller_cur_sql))
                    .bind(seller_container_id)
                    .bind(currency_def_id)
                    .fetch_all(&mut *conn)
                    .await?;

                let mut credit_rem = price as i128;
                for r in seller_cur_rows {
                    if credit_rem == 0 {
                        break;
                    }
                    let stack_id: String = r.try_get("item_instance_id")?;
                    let slot: i32 = r.try_get("slot")?;
                    let qty: i32 = r.try_get("quantity")?;
                    if qty < 0 || seller_reserved_slots.contains(&slot) {
                        continue;
                    }
                    let item_reserved = sqlx::query(&q!(
                        "SELECT 1 FROM container_reservations WHERE item_instance_id = ? AND expires_at_epoch_ms > ?"
                    ))
                    .bind(&stack_id)
                    .bind(now_ms)
                    .fetch_optional(&mut *conn)
                    .await?
                    .is_some();
                    if item_reserved {
                        continue;
                    }

                    let headroom = (i32::MAX as i128) - (qty as i128);
                    if headroom > 0 {
                        let add = credit_rem.min(headroom);
                        let new_qty = (qty as i128 + add) as i32;
                        sqlx::query(&q!(
                            "UPDATE items SET quantity = ?, character_guid = ?, updated_at = ? WHERE item_instance_id = ? AND container_id = ?"
                        ))
                        .bind(new_qty)
                        .bind(seller_owner_guid)
                        .bind(now_secs)
                        .bind(&stack_id)
                        .bind(seller_container_id)
                        .execute(&mut *conn)
                        .await?;
                        credit_rem -= add;
                    }
                }

                if credit_rem > 0 {
                    let s_occ_rows =
                        sqlx::query(&q!("SELECT slot FROM items WHERE container_id = ?"))
                            .bind(seller_container_id)
                            .fetch_all(&mut *conn)
                            .await?;
                    let mut seller_occupied: std::collections::HashSet<u32> = s_occ_rows
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();
                    for rslot in &seller_reserved_slots {
                        if *rslot >= 0 {
                            seller_occupied.insert(*rslot as u32);
                        }
                    }

                    let mut stack_idx: u32 = 0;
                    while credit_rem > 0 {
                        let free_slot = find_first_free_slot(&seller_occupied, seller_cap as u32)
                            .ok_or(StorageError::ContainerFull(
                            seller_container_id as u64,
                            seller_cap as u32,
                        ))?;
                        seller_occupied.insert(free_slot);

                        let stack_qty = credit_rem.min(i32::MAX as i128) as i32;
                        let new_cur_id = format!("mkt_cur_{}_{}", listing_id, stack_idx);
                        stack_idx += 1;

                        sqlx::query(&q!(
                            "INSERT INTO items (item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 100, '{}', ?, ?)"
                        ))
                        .bind(&new_cur_id)
                        .bind(seller_owner_guid)
                        .bind(seller_container_id)
                        .bind(currency_def_id)
                        .bind(free_slot as i32)
                        .bind(stack_qty)
                        .bind(now_secs)
                        .bind(now_secs)
                        .execute(&mut *conn)
                        .await?;

                        credit_rem -= stack_qty as i128;
                    }
                }
            }

            // 9. Update listing status to 'Sold'
            sqlx::query(&q!(
                "UPDATE market_listings SET status = 'Sold', updated_at_epoch_ms = ? WHERE listing_id = ?"
            ))
            .bind(now_ms)
            .bind(listing_id)
            .execute(&mut *conn)
            .await?;

            // 10. Record audit logging into container_transactions
            let item_tx_id = format!("mkt_item_{}", listing_id);
            sqlx::query(&q!(
                "INSERT INTO container_transactions (tx_id, src_container_id, dst_container_id, item_instance_id, status, timestamp_epoch_ms) VALUES (?, ?, ?, ?, 'committed', ?)"
            ))
            .bind(&item_tx_id)
            .bind(escrow_container_id)
            .bind(buyer_container_id as i64)
            .bind(&item_instance_id)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;

            let pay_tx_id = format!("mkt_pay_{}", listing_id);
            let pay_item_desc = format!("price:{}", price);
            sqlx::query(&q!(
                "INSERT INTO container_transactions (tx_id, src_container_id, dst_container_id, item_instance_id, status, timestamp_epoch_ms) VALUES (?, ?, ?, ?, 'committed', ?)"
            ))
            .bind(&pay_tx_id)
            .bind(payment_container_id as i64)
            .bind(seller_container_id)
            .bind(&pay_item_desc)
            .bind(now_ms)
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
        })
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

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Fetch and lock listing first (consistent lock order: market_listings -> containers)
            let list_sql = format!(
                r#"
                SELECT listing_id, seller_guid, seller_container_id, item_instance_id, item_def_id,
                       price, escrow_container_id, escrow_slot, status, created_at_epoch_ms,
                       expires_at_epoch_ms, updated_at_epoch_ms
                FROM market_listings WHERE listing_id = ?{}
                "#,
                lock_suffix!()
            );
            let list_row = sqlx::query(&q!(&list_sql))
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
            if status_str != "Active" && status_str != "ExpiredPendingClaim" {
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

            // 2. Lock escrow & destination containers in ascending order and verify seller ownership
            let mut cids = vec![escrow_container_id as u64, dst_container_id];
            cids.sort_unstable();
            cids.dedup();

            let container_sql = format!(
                "SELECT container_id, owner_type, owner_id, capacity FROM containers WHERE container_id = ?{}",
                lock_suffix!()
            );
            let mut dst_cap: i32 = 0;
            let mut dst_owner_guid: i64 = 0;
            let seller_guid_str = seller_guid.to_string();

            for cid in cids {
                let row = sqlx::query(&q!(&container_sql))
                    .bind(cid as i64)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or(StorageError::ContainerNotFound(cid))?;
                if cid == dst_container_id {
                    let owner_type: String = row.try_get("owner_type")?;
                    let owner_id: String = row.try_get("owner_id")?;
                    if owner_id != seller_guid_str {
                        return Err(StorageError::UnauthorizedContainerAccess(dst_container_id));
                    }
                    dst_cap = row.try_get("capacity")?;
                    dst_owner_guid = resolve_container_owner_guid(&owner_type, &owner_id);
                }
            }

            // 3. Reject if escrow item or slot has an active 2PC reservation
            let esc_res = sqlx::query(&q!(
                "SELECT 1 FROM container_reservations WHERE ((container_id = ? AND slot = ?) OR item_instance_id = ?) AND expires_at_epoch_ms > ?"
            ))
            .bind(escrow_container_id)
            .bind(escrow_slot)
            .bind(&item_instance_id)
            .bind(now_ms)
            .fetch_optional(&mut *conn)
            .await?;
            if esc_res.is_some() {
                return Err(StorageError::SlotReserved(
                    escrow_slot as u32,
                    escrow_container_id as u64,
                ));
            }

            // 4. Find free slot in destination container (excluding items + active reservations)
            let occupied_rows = sqlx::query(&q!("SELECT slot FROM items WHERE container_id = ?"))
                .bind(dst_container_id as i64)
                .fetch_all(&mut *conn)
                .await?;
            let mut occupied_slots: std::collections::HashSet<u32> = occupied_rows
                .into_iter()
                .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                .collect();

            let dst_reserved_rows = sqlx::query(&q!(
                "SELECT slot FROM container_reservations WHERE container_id = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(dst_container_id as i64)
            .bind(now_ms)
            .fetch_all(&mut *conn)
            .await?;
            for r in dst_reserved_rows {
                if let Ok(slot) = r.try_get::<i32, _>("slot") {
                    occupied_slots.insert(slot as u32);
                }
            }

            let free_slot = find_first_free_slot(&occupied_slots, dst_cap as u32).ok_or(
                StorageError::ContainerFull(dst_container_id, dst_cap as u32),
            )?;

            // 5. Move item back to seller container and sync character_guid
            let move_res = sqlx::query(&q!(
                "UPDATE items SET container_id = ?, slot = ?, character_guid = ?, updated_at = ? WHERE item_instance_id = ? AND container_id = ? AND slot = ?"
            ))
            .bind(dst_container_id as i64)
            .bind(free_slot as i32)
            .bind(dst_owner_guid)
            .bind(now_secs)
            .bind(&item_instance_id)
            .bind(escrow_container_id)
            .bind(escrow_slot)
            .execute(&mut *conn)
            .await?;

            if move_res.rows_affected() != 1 {
                return Err(StorageError::ItemNotFound(
                    item_instance_id,
                    escrow_container_id as u64,
                ));
            }

            // 6. Mark listing as Cancelled
            sqlx::query(&q!(
                "UPDATE market_listings SET status = 'Cancelled', updated_at_epoch_ms = ? WHERE listing_id = ?"
            ))
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
        })
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
        let query_str = self.db.sql(query);

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
                let row = sqlx::query(&query_str)
                    .bind(listing_id)
                    .fetch_optional(pool)
                    .await?;
                match row {
                    Some(r) => Ok(Some(parse_market_listing!(r))),
                    None => Ok(None),
                }
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
        let query_str = self.db.sql(&query);

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
        }
    }

    /// Finds expired active listings, marks them 'Expired', and atomically refunds items to sellers.
    pub async fn reap_expired_listings(&self, max_batch: u32) -> Result<u32, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();

        let query = r#"
            SELECT listing_id, seller_guid, seller_container_id, item_instance_id, escrow_container_id, escrow_slot
            FROM market_listings
            WHERE status = 'Active' AND expires_at_epoch_ms <= ?
            ORDER BY expires_at_epoch_ms ASC
            LIMIT ?
        "#;
        let query_str = self.db.sql(query);

        let candidates = match self.db.backend() {
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
                        escrow_container_id: r.try_get("escrow_container_id")?,
                        escrow_slot: r.try_get("escrow_slot")?,
                        item_instance_id: r.try_get("item_instance_id")?,
                    });
                }
                list
            }
            DbBackend::Postgres(pool) => {
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
                        escrow_container_id: r.try_get("escrow_container_id")?,
                        escrow_slot: r.try_get("escrow_slot")?,
                        item_instance_id: r.try_get("item_instance_id")?,
                    });
                }
                list
            }
        };

        let mut reaped = 0;
        for r in candidates {
            let res: Result<bool, StorageError> = with_dialect_tx!(
                &self.db,
                |conn, is_pg| async move {
                    // 1. Lock listing row first and verify it is still Active
                    let list_sql = format!(
                        "SELECT status FROM market_listings WHERE listing_id = ?{}",
                        lock_suffix!()
                    );
                    let Some(list_row) = sqlx::query(&q!(&list_sql))
                        .bind(&r.listing_id)
                        .fetch_optional(&mut *conn)
                        .await?
                    else {
                        return Ok(false);
                    };
                    let status_str: String = list_row.try_get("status")?;
                    if status_str != "Active" {
                        return Ok(false);
                    }

                    // 2. Lock escrow and seller containers in ascending order
                    let mut cids = vec![r.escrow_container_id, r.seller_container_id];
                    cids.sort_unstable();
                    cids.dedup();

                    let c_sql = format!(
                        "SELECT container_id, owner_type, owner_id, capacity FROM containers WHERE container_id = ?{}",
                        lock_suffix!()
                    );
                    let mut seller_cap: Option<i32> = None;
                    let mut seller_owner_guid: i64 = 0;
                    for cid in cids {
                        let c_opt = sqlx::query(&q!(&c_sql))
                            .bind(cid)
                            .fetch_optional(&mut *conn)
                            .await?;
                        if cid == r.seller_container_id {
                            if let Some(c_row) = c_opt {
                                seller_cap = Some(c_row.try_get("capacity")?);
                                let owner_type: String = c_row.try_get("owner_type")?;
                                let owner_id: String = c_row.try_get("owner_id")?;
                                seller_owner_guid =
                                    resolve_container_owner_guid(&owner_type, &owner_id);
                            }
                        }
                    }

                    // 3. Do not touch items locked by an active 2PC reservation
                    let esc_res = sqlx::query(&q!(
                        "SELECT 1 FROM container_reservations WHERE ((container_id = ? AND slot = ?) OR item_instance_id = ?) AND expires_at_epoch_ms > ?"
                    ))
                    .bind(r.escrow_container_id)
                    .bind(r.escrow_slot)
                    .bind(&r.item_instance_id)
                    .bind(now_ms)
                    .fetch_optional(&mut *conn)
                    .await?;
                    if esc_res.is_some() {
                        return Ok(false);
                    }

                    let Some(cap) = seller_cap else {
                        sqlx::query(&q!(
                            "UPDATE market_listings SET status = 'ExpiredPendingClaim', updated_at_epoch_ms = ? WHERE listing_id = ? AND status = 'Active'"
                        ))
                        .bind(now_ms)
                        .bind(&r.listing_id)
                        .execute(&mut *conn)
                        .await?;
                        return Ok(false);
                    };

                    let occupied_rows =
                        sqlx::query(&q!("SELECT slot FROM items WHERE container_id = ?"))
                            .bind(r.seller_container_id)
                            .fetch_all(&mut *conn)
                            .await?;
                    let mut occupied_slots: std::collections::HashSet<u32> = occupied_rows
                        .into_iter()
                        .filter_map(|row| row.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let seller_reserved_rows = sqlx::query(&q!(
                        "SELECT slot FROM container_reservations WHERE container_id = ? AND expires_at_epoch_ms > ?"
                    ))
                    .bind(r.seller_container_id)
                    .bind(now_ms)
                    .fetch_all(&mut *conn)
                    .await?;
                    for row in seller_reserved_rows {
                        if let Ok(slot) = row.try_get::<i32, _>("slot") {
                            occupied_slots.insert(slot as u32);
                        }
                    }

                    let Some(free_slot) = find_first_free_slot(&occupied_slots, cap as u32) else {
                        sqlx::query(&q!(
                            "UPDATE market_listings SET status = 'ExpiredPendingClaim', updated_at_epoch_ms = ? WHERE listing_id = ? AND status = 'Active'"
                        ))
                        .bind(now_ms)
                        .bind(&r.listing_id)
                        .execute(&mut *conn)
                        .await?;
                        return Ok(false);
                    };

                    let move_res = sqlx::query(&q!(
                        "UPDATE items SET container_id = ?, slot = ?, character_guid = ?, updated_at = ? WHERE item_instance_id = ? AND container_id = ? AND slot = ?"
                    ))
                    .bind(r.seller_container_id)
                    .bind(free_slot as i32)
                    .bind(seller_owner_guid)
                    .bind(now_secs)
                    .bind(&r.item_instance_id)
                    .bind(r.escrow_container_id)
                    .bind(r.escrow_slot)
                    .execute(&mut *conn)
                    .await?;

                    if move_res.rows_affected() != 1 {
                        return Err(StorageError::ItemNotFound(
                            r.item_instance_id.clone(),
                            r.escrow_container_id as u64,
                        ));
                    }

                    let res = sqlx::query(&q!(
                        "UPDATE market_listings SET status = 'Expired', updated_at_epoch_ms = ? WHERE listing_id = ? AND status = 'Active'"
                    ))
                    .bind(now_ms)
                    .bind(&r.listing_id)
                    .execute(&mut *conn)
                    .await?;

                    if res.rows_affected() == 1 {
                        Ok(true)
                    } else {
                        Err(StorageError::ListingNotActive(
                            r.listing_id.clone(),
                            "Already processed".to_string(),
                        ))
                    }
                }
            );

            if let Ok(true) = res {
                reaped += 1;
            }
        }

        Ok(reaped)
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

    async fn create_currency(
        db: &Database,
        item_id: &str,
        character_guid: u64,
        container_id: u64,
        slot: u32,
        quantity: i32,
    ) {
        let item = crate::models::ItemRecordBuilder::new(item_id)
            .character_guid(character_guid)
            .container_id(container_id)
            .item_def_id(DEFAULT_CURRENCY_ITEM_DEF_ID)
            .slot(slot)
            .quantity(quantity as u32)
            .build();
        crate::repository::CharacterRepository::new(db.clone())
            .add_item(&item)
            .await
            .unwrap();
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
        create_currency(&db, "buyer-gold-1", buyer_guid, payment_container, 0, 2000).await;

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

        // Verify item is now in buyer container with synced character_guid
        let buyer_items = crepo.get_container_items(buyer_container).await.unwrap();
        assert_eq!(buyer_items.len(), 1);
        assert_eq!(buyer_items[0].item_instance_id, "item-sword-1");
        assert_eq!(buyer_items[0].character_guid, buyer_guid);

        // Verify currency deducted from buyer payment container and credited to seller
        let pay_items = crepo.get_container_items(payment_container).await.unwrap();
        assert_eq!(pay_items.len(), 1);
        assert_eq!(pay_items[0].quantity, 500);
        let seller_items = crepo.get_container_items(seller_container).await.unwrap();
        assert_eq!(seller_items.len(), 1);
        assert_eq!(seller_items[0].item_def_id, DEFAULT_CURRENCY_ITEM_DEF_ID);
        assert_eq!(seller_items[0].quantity, 1500);
        assert_eq!(seller_items[0].character_guid, seller_guid);

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
    async fn test_marketplace_purchase_listing_container_validations() {
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
        create_currency(&db, "buyer-gold-self", buyer_guid, buyer_container, 0, 100).await;

        let listing = repo
            .create_listing(
                seller_guid,
                seller_container,
                0,
                100,
                3600,
                escrow_container,
            )
            .await
            .unwrap();

        // 1. Missing buyer container
        let err_missing_buyer = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                9991,
                payment_container,
                100,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err_missing_buyer,
            StorageError::ContainerNotFound(9991)
        ));

        // 2. Missing payment container
        let err_missing_payment = repo
            .purchase_listing(buyer_guid, &listing.listing_id, buyer_container, 9992, 100)
            .await
            .unwrap_err();
        assert!(matches!(
            err_missing_payment,
            StorageError::ContainerNotFound(9992)
        ));

        // 3. Unauthorized container access (buyer 3003 tries to use 2002's container)
        let imposter_guid = 3003;
        let err_unauth = repo
            .purchase_listing(
                imposter_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                100,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err_unauth,
            StorageError::UnauthorizedContainerAccess(_)
        ));

        // 4. Successful purchase with identical buyer and payment container (deduplication)
        let purchased = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                buyer_container,
                100,
            )
            .await
            .unwrap();
        assert_eq!(purchased.status, ListingStatus::Sold);
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

    #[tokio::test]
    async fn test_marketplace_reap_expired_listings_seller_inventory_full_safety() {
        let (db, repo) = setup_test_db().await;

        let seller_guid = 1001;
        let seller_container = 10;
        let escrow_container = 99;

        create_container(&db, seller_container, "1001", 10).await;
        create_container(&db, escrow_container, "system", 50).await;
        create_item(&db, "item-exp-full", seller_container, 0, 700).await;

        let listing = repo
            .create_listing(seller_guid, seller_container, 0, 100, 0, escrow_container)
            .await
            .unwrap();

        // Fill seller container completely (slots 0..10)
        for s in 0..10 {
            create_item(&db, &format!("seller-fill-{}", s), seller_container, s, 100).await;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let reaped = repo.reap_expired_listings(10).await.unwrap();
        assert_eq!(
            reaped, 0,
            "Must not reap when seller container has no free slot"
        );

        // Listing status must transition to ExpiredPendingClaim (preventing reaper starvation)
        let fetched = repo
            .get_listing(&listing.listing_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.status, ListingStatus::ExpiredPendingClaim);

        // Item must remain safely in escrow
        let crepo = crate::container_repo::ContainerRepository::new(db.clone());
        let escrow_items = crepo.get_container_items(escrow_container).await.unwrap();
        assert_eq!(escrow_items.len(), 1);
        assert_eq!(escrow_items[0].item_instance_id, "item-exp-full");

        // Verify reaper is NOT starved for subsequent expired listings:
        // Create second seller with space in container
        let seller2_guid = 1002;
        let seller2_container = 11;
        create_container(&db, seller2_container, "1002", 10).await;
        create_item(&db, "item-exp-avail", seller2_container, 0, 701).await;
        let listing2 = repo
            .create_listing(seller2_guid, seller2_container, 0, 100, 0, escrow_container)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Second reap sweep should successfully process listing2 without being blocked by listing 1
        let reaped2 = repo.reap_expired_listings(10).await.unwrap();
        assert_eq!(reaped2, 1, "Subsequent unblocked listing must be reaped");
        let fetched2 = repo
            .get_listing(&listing2.listing_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched2.status, ListingStatus::Expired);

        // Verify seller 1 can reclaim the ExpiredPendingClaim listing once bag space is cleared
        sqlx::query("DELETE FROM items WHERE item_instance_id = 'seller-fill-0'")
            .execute(db.pool())
            .await
            .unwrap();

        let reclaimed = repo
            .cancel_listing(seller_guid, &listing.listing_id, seller_container)
            .await
            .unwrap();
        assert_eq!(reclaimed.status, ListingStatus::Cancelled);

        let s1_items = crepo.get_container_items(seller_container).await.unwrap();
        assert!(s1_items
            .iter()
            .any(|i| i.item_instance_id == "item-exp-full"));
    }

    #[tokio::test]
    async fn test_marketplace_purchase_records_container_transactions_audit() {
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

        create_item(&db, "item-audit-sword", seller_container, 0, 888).await;
        create_currency(
            &db,
            "buyer-gold-audit",
            buyer_guid,
            payment_container,
            0,
            500,
        )
        .await;

        let listing = repo
            .create_listing(
                seller_guid,
                seller_container,
                0,
                500,
                3600,
                escrow_container,
            )
            .await
            .unwrap();

        let purchased = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                500,
            )
            .await
            .unwrap();
        assert_eq!(purchased.status, ListingStatus::Sold);

        // Verify container_transactions has item transfer and payment records
        let item_tx_id = format!("mkt_item_{}", listing.listing_id);
        let pay_tx_id = format!("mkt_pay_{}", listing.listing_id);

        let item_tx_row = sqlx::query("SELECT src_container_id, dst_container_id, item_instance_id, status FROM container_transactions WHERE tx_id = ?")
            .bind(&item_tx_id)
            .fetch_optional(db.sqlite_pool().unwrap())
            .await
            .unwrap()
            .expect("item audit transaction row exists");

        assert_eq!(
            item_tx_row.try_get::<i64, _>("src_container_id").unwrap(),
            escrow_container as i64
        );
        assert_eq!(
            item_tx_row.try_get::<i64, _>("dst_container_id").unwrap(),
            buyer_container as i64
        );
        assert_eq!(
            item_tx_row
                .try_get::<String, _>("item_instance_id")
                .unwrap(),
            "item-audit-sword"
        );
        assert_eq!(
            item_tx_row.try_get::<String, _>("status").unwrap(),
            "committed"
        );

        let pay_tx_row = sqlx::query("SELECT src_container_id, dst_container_id, item_instance_id, status FROM container_transactions WHERE tx_id = ?")
            .bind(&pay_tx_id)
            .fetch_optional(db.sqlite_pool().unwrap())
            .await
            .unwrap()
            .expect("payment audit transaction row exists");

        assert_eq!(
            pay_tx_row.try_get::<i64, _>("src_container_id").unwrap(),
            payment_container as i64
        );
        assert_eq!(
            pay_tx_row.try_get::<i64, _>("dst_container_id").unwrap(),
            seller_container as i64
        );
        assert_eq!(
            pay_tx_row.try_get::<String, _>("item_instance_id").unwrap(),
            "price:500"
        );
        assert_eq!(
            pay_tx_row.try_get::<String, _>("status").unwrap(),
            "committed"
        );
    }

    #[tokio::test]
    async fn test_marketplace_insufficient_currency_and_multistack_settlement() {
        let (db, repo) = setup_test_db().await;
        let crepo = crate::container_repo::ContainerRepository::new(db.clone());

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

        create_item(&db, "item-epic-axe", seller_container, 0, 777).await;
        create_currency(&db, "gold-stack-1", buyer_guid, payment_container, 0, 300).await;

        let listing = repo
            .create_listing(
                seller_guid,
                seller_container,
                0,
                500,
                3600,
                escrow_container,
            )
            .await
            .unwrap();

        // Attempting purchase with client price=500 when container only has 300 must fail
        let err = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                500,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::TradeError(_)));

        // Listing remains Active and currency untouched
        let active = repo
            .get_listing(&listing.listing_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.status, ListingStatus::Active);
        assert_eq!(
            crepo.get_container_items(payment_container).await.unwrap()[0].quantity,
            300
        );

        // Add second stack (350) -> total 650 across 2 stacks; spend 500 -> first deleted, second left with 150
        create_currency(&db, "gold-stack-2", buyer_guid, payment_container, 1, 350).await;
        // Seller already has 100 gold in slot 2 -> should be topped up to 600
        create_currency(
            &db,
            "seller-gold-existing",
            seller_guid,
            seller_container,
            2,
            100,
        )
        .await;

        let sold = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                999, // Over-offered client price: must only deduct listing.price = 500
            )
            .await
            .unwrap();
        assert_eq!(sold.status, ListingStatus::Sold);

        let pay_items = crepo.get_container_items(payment_container).await.unwrap();
        assert_eq!(pay_items.len(), 1);
        assert_eq!(pay_items[0].item_instance_id, "gold-stack-2");
        assert_eq!(pay_items[0].quantity, 150);

        let seller_items = crepo.get_container_items(seller_container).await.unwrap();
        assert_eq!(seller_items.len(), 1);
        assert_eq!(seller_items[0].quantity, 600);
        assert_eq!(seller_items[0].character_guid, seller_guid);

        let buyer_items = crepo.get_container_items(buyer_container).await.unwrap();
        assert_eq!(buyer_items.len(), 1);
        assert_eq!(buyer_items[0].item_instance_id, "item-epic-axe");
        assert_eq!(buyer_items[0].character_guid, buyer_guid);
    }

    #[tokio::test]
    async fn test_marketplace_rejects_reservations_and_syncs_owner_guid() {
        let (db, repo) = setup_test_db().await;
        let crepo = crate::container_repo::ContainerRepository::new(db.clone());
        let tx_coord = crate::transaction_coord::TransactionCoordinator::new(db.clone());

        let seller_guid = 1001;
        let buyer_guid = 2002;
        let seller_container = 10;
        let escrow_container = 99;
        let buyer_container = 20;
        let payment_container = 21;
        let dummy_container = 22;

        create_container(&db, seller_container, "1001", 10).await;
        create_container(&db, escrow_container, "system", 50).await;
        create_container(&db, buyer_container, "2002", 10).await;
        create_container(&db, payment_container, "2002", 10).await;
        create_container(&db, dummy_container, "2002", 10).await;

        create_item(&db, "item-res-lock", seller_container, 0, 555).await;

        // 1. Reserve seller slot 0 via 2PC -> create_listing must reject with SlotReserved
        tx_coord
            .prepare_transfer(
                "tx_mkt_lock_1",
                seller_container,
                0,
                dummy_container,
                0,
                5000,
            )
            .await
            .unwrap();
        let list_err = repo
            .create_listing(
                seller_guid,
                seller_container,
                0,
                100,
                3600,
                escrow_container,
            )
            .await
            .unwrap_err();
        assert!(matches!(list_err, StorageError::SlotReserved(0, 10)));
        tx_coord.abort_transfer("tx_mkt_lock_1").await.unwrap();

        // Now listing succeeds, and item in system escrow has character_guid == 0
        let listing = repo
            .create_listing(
                seller_guid,
                seller_container,
                0,
                100,
                3600,
                escrow_container,
            )
            .await
            .unwrap();
        let esc_items = crepo.get_container_items(escrow_container).await.unwrap();
        assert_eq!(esc_items[0].character_guid, 0);

        // 2. Lock buyer currency in payment_container via 2PC -> purchase_listing rejects
        create_currency(&db, "buyer-gold-res", buyer_guid, payment_container, 0, 100).await;
        tx_coord
            .prepare_transfer(
                "tx_pay_lock",
                payment_container,
                0,
                dummy_container,
                1,
                5000,
            )
            .await
            .unwrap();
        let buy_res_pay_err = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                100,
            )
            .await
            .unwrap_err();
        assert!(matches!(buy_res_pay_err, StorageError::SlotReserved(0, 21)));
        tx_coord.abort_transfer("tx_pay_lock").await.unwrap();

        // 3. Reserve escrow slot via 2PC -> purchase_listing & cancel_listing reject
        tx_coord
            .prepare_transfer(
                "tx_esc_lock",
                escrow_container,
                listing.escrow_slot,
                dummy_container,
                2,
                5000,
            )
            .await
            .unwrap();
        let buy_res_esc_err = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                100,
            )
            .await
            .unwrap_err();
        assert!(matches!(buy_res_esc_err, StorageError::SlotReserved(_, 99)));
        let cancel_res_esc_err = repo
            .cancel_listing(seller_guid, &listing.listing_id, seller_container)
            .await
            .unwrap_err();
        assert!(matches!(
            cancel_res_esc_err,
            StorageError::SlotReserved(_, 99)
        ));
        tx_coord.abort_transfer("tx_esc_lock").await.unwrap();

        // 4. Reserve buyer_container slot 0 -> purchase_listing must place item in slot 1!
        create_item(&db, "tmp-item", dummy_container, 0, 111).await;
        tx_coord
            .prepare_transfer("tx_dst_slot0", dummy_container, 0, buyer_container, 0, 5000)
            .await
            .unwrap();
        let purchased = repo
            .purchase_listing(
                buyer_guid,
                &listing.listing_id,
                buyer_container,
                payment_container,
                100,
            )
            .await
            .unwrap();
        assert_eq!(purchased.status, ListingStatus::Sold);
        tx_coord.abort_transfer("tx_dst_slot0").await.unwrap();

        let buyer_items = crepo.get_container_items(buyer_container).await.unwrap();
        assert_eq!(buyer_items.len(), 1);
        assert_eq!(buyer_items[0].slot, 1);
        assert_eq!(buyer_items[0].character_guid, buyer_guid);
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
