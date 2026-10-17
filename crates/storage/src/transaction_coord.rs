//! Distributed Two-Phase Commit (2PC / Sagas) Transaction Coordinator for containers.

use crate::db::{current_time_ms, current_time_secs, Database, DbBackend, StorageError};
use crate::models::{ContainerTransactionRecord, ReservationRecord};
use crate::with_dialect_tx;
use sqlx::Row;

/// Coordinates distributed, atomic two-phase transfers between arbitrary containers.
#[derive(Clone)]
pub struct TransactionCoordinator {
    db: Database,
}

impl TransactionCoordinator {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Phase 1: Prepares a transfer by reserving the source item and target slot under strict row locks.
    pub async fn prepare_transfer(
        &self,
        tx_id: &str,
        src_container_id: u64,
        src_slot: u32,
        dst_container_id: u64,
        dst_slot: u32,
        ttl_ms: u64,
    ) -> Result<ReservationRecord, StorageError> {
        let now_ms = current_time_ms();
        let expires_at_ms = now_ms + ttl_ms as i64;

        let (first_id, second_id) = (
            src_container_id.min(dst_container_id),
            src_container_id.max(dst_container_id),
        );

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Verify both containers exist
            let q_c = format!(
                "SELECT container_id, capacity FROM containers WHERE container_id = ?{}",
                lock_suffix!()
            );
            let c1 = sqlx::query(&q!(&q_c))
                .bind(first_id as i64)
                .fetch_optional(&mut *conn)
                .await?;
            let c2 = sqlx::query(&q!(&q_c))
                .bind(second_id as i64)
                .fetch_optional(&mut *conn)
                .await?;

            if c1.is_none() {
                return Err(StorageError::ContainerNotFound(first_id));
            }
            if c2.is_none() {
                return Err(StorageError::ContainerNotFound(second_id));
            }

            let dst_capacity: i32 = if dst_container_id == first_id {
                c1.unwrap().try_get("capacity")?
            } else {
                c2.unwrap().try_get("capacity")?
            };

            if dst_slot >= dst_capacity as u32 {
                return Err(StorageError::SlotOccupied(dst_slot, dst_container_id));
            }

            // 2. Fetch source item
            let src_item = sqlx::query(&q!(
                "SELECT item_instance_id FROM items WHERE container_id = ? AND slot = ?"
            ))
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .fetch_optional(&mut *conn)
            .await?;

            let item_instance_id: String = match src_item {
                Some(row) => row.try_get("item_instance_id")?,
                None => {
                    return Err(StorageError::ItemNotFound(
                        format!("slot:{}", src_slot),
                        src_container_id,
                    ))
                }
            };

            // 3. Check destination slot is not occupied
            let dst_occupied = sqlx::query(&q!(
                "SELECT 1 FROM items WHERE container_id = ? AND slot = ?"
            ))
            .bind(dst_container_id as i64)
            .bind(dst_slot as i32)
            .fetch_optional(&mut *conn)
            .await?;

            if dst_occupied.is_some() {
                return Err(StorageError::SlotOccupied(dst_slot, dst_container_id));
            }

            // 4. Check for active non-expired reservations on source or destination slot
            let active_src_res = sqlx::query(&q!(
                "SELECT 1 FROM container_reservations WHERE container_id = ? AND slot = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .bind(now_ms)
            .fetch_optional(&mut *conn)
            .await?;

            if active_src_res.is_some() {
                return Err(StorageError::SlotReserved(src_slot, src_container_id));
            }

            let active_dst_res = sqlx::query(&q!(
                "SELECT 1 FROM container_reservations WHERE container_id = ? AND slot = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(dst_container_id as i64)
            .bind(dst_slot as i32)
            .bind(now_ms)
            .fetch_optional(&mut *conn)
            .await?;

            if active_dst_res.is_some() {
                return Err(StorageError::SlotReserved(dst_slot, dst_container_id));
            }

            // Clean up any stale expired reservations for these slots
            sqlx::query(&q!(
                "DELETE FROM container_reservations WHERE ((container_id = ? AND slot = ?) OR (container_id = ? AND slot = ?)) AND expires_at_epoch_ms <= ?"
            ))
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .bind(dst_container_id as i64)
            .bind(dst_slot as i32)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;

            // 5. Insert reservations
            let src_res_id = format!("{}_src", tx_id);
            let dst_res_id = format!("{}_dst", tx_id);

            sqlx::query(&q!(
                r#"
                INSERT INTO container_reservations (reservation_id, tx_id, container_id, slot, item_instance_id, reservation_type, expires_at_epoch_ms, created_at_epoch_ms)
                VALUES (?, ?, ?, ?, ?, 'transfer_out', ?, ?)
                "#
            ))
            .bind(&src_res_id)
            .bind(tx_id)
            .bind(src_container_id as i64)
            .bind(src_slot as i32)
            .bind(&item_instance_id)
            .bind(expires_at_ms)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;

            sqlx::query(&q!(
                r#"
                INSERT INTO container_reservations (reservation_id, tx_id, container_id, slot, item_instance_id, reservation_type, expires_at_epoch_ms, created_at_epoch_ms)
                VALUES (?, ?, ?, ?, NULL, 'transfer_in', ?, ?)
                "#
            ))
            .bind(&dst_res_id)
            .bind(tx_id)
            .bind(dst_container_id as i64)
            .bind(dst_slot as i32)
            .bind(expires_at_ms)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;

            Ok(ReservationRecord {
                reservation_id: src_res_id,
                tx_id: tx_id.to_string(),
                container_id: src_container_id,
                slot: src_slot,
                item_instance_id: Some(item_instance_id),
                reservation_type: "transfer_out".to_string(),
                expires_at_epoch_ms: expires_at_ms,
                created_at_epoch_ms: now_ms,
            })
        })
    }

    /// Reserves a single slot on a container (used for distributed 2PC or isolated reservations).
    pub async fn reserve_slot(
        &self,
        tx_id: &str,
        container_id: u64,
        slot: u32,
        item_instance_id: Option<&str>,
        reservation_type: &str,
        ttl_ms: u64,
    ) -> Result<ReservationRecord, StorageError> {
        let now_ms = current_time_ms();
        let expires_at_ms = now_ms + ttl_ms as i64;
        let reservation_id = format!("res_{}_{}_{}", tx_id, container_id, slot);

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Verify container exists and capacity
            let q_c = format!(
                "SELECT container_id, capacity FROM containers WHERE container_id = ?{}",
                lock_suffix!()
            );
            let c = sqlx::query(&q!(&q_c))
                .bind(container_id as i64)
                .fetch_optional(&mut *conn)
                .await?;

            let cap: i32 = match c {
                Some(row) => row.try_get("capacity")?,
                None => return Err(StorageError::ContainerNotFound(container_id)),
            };

            if slot >= cap as u32 {
                return Err(StorageError::SlotOccupied(slot, container_id));
            }

            // 2. Validate items based on reservation_type
            if reservation_type == "transfer_out" {
                let src_item = sqlx::query(&q!(
                    "SELECT item_instance_id FROM items WHERE container_id = ? AND slot = ?"
                ))
                .bind(container_id as i64)
                .bind(slot as i32)
                .fetch_optional(&mut *conn)
                .await?;

                let found_id: String = match src_item {
                    Some(row) => row.try_get("item_instance_id")?,
                    None => {
                        return Err(StorageError::ItemNotFound(
                            format!("slot:{}", slot),
                            container_id,
                        ))
                    }
                };

                if let Some(expected_id) = item_instance_id {
                    if found_id != expected_id {
                        return Err(StorageError::ItemNotFound(
                            expected_id.to_string(),
                            container_id,
                        ));
                    }
                }
            } else if reservation_type == "transfer_in" {
                let occupied = sqlx::query(&q!(
                    "SELECT 1 FROM items WHERE container_id = ? AND slot = ?"
                ))
                .bind(container_id as i64)
                .bind(slot as i32)
                .fetch_optional(&mut *conn)
                .await?;

                if occupied.is_some() {
                    return Err(StorageError::SlotOccupied(slot, container_id));
                }
            }

            // 3. Check for active reservation
            let active_res = sqlx::query(&q!(
                "SELECT 1 FROM container_reservations WHERE container_id = ? AND slot = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(container_id as i64)
            .bind(slot as i32)
            .bind(now_ms)
            .fetch_optional(&mut *conn)
            .await?;

            if active_res.is_some() {
                return Err(StorageError::SlotReserved(slot, container_id));
            }

            // Cleanup any expired reservation on this slot
            sqlx::query(&q!(
                "DELETE FROM container_reservations WHERE container_id = ? AND slot = ?"
            ))
            .bind(container_id as i64)
            .bind(slot as i32)
            .execute(&mut *conn)
            .await?;

            // 4. Insert reservation
            sqlx::query(&q!(
                r#"
                INSERT INTO container_reservations (reservation_id, tx_id, container_id, slot, item_instance_id, reservation_type, expires_at_epoch_ms, created_at_epoch_ms)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#
            ))
            .bind(&reservation_id)
            .bind(tx_id)
            .bind(container_id as i64)
            .bind(slot as i32)
            .bind(item_instance_id)
            .bind(reservation_type)
            .bind(expires_at_ms)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;

            Ok(ReservationRecord {
                reservation_id: reservation_id.clone(),
                tx_id: tx_id.to_string(),
                container_id,
                slot,
                item_instance_id: item_instance_id.map(|s| s.to_string()),
                reservation_type: reservation_type.to_string(),
                expires_at_epoch_ms: expires_at_ms,
                created_at_epoch_ms: now_ms,
            })
        })
    }

    /// Phase 2: Commits a prepared transfer atomically, asserting that the reservation is active and unexpired.
    pub async fn commit_transfer(
        &self,
        tx_id: &str,
    ) -> Result<ContainerTransactionRecord, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Fetch reservation details
            let rows = sqlx::query(&q!(
                "SELECT reservation_id, container_id, slot, item_instance_id, reservation_type, expires_at_epoch_ms FROM container_reservations WHERE tx_id = ?"
            ))
            .bind(tx_id)
            .fetch_all(&mut *conn)
            .await?;

            if rows.len() != 2 {
                return Err(StorageError::ReservationExpiredOrAborted(tx_id.to_string()));
            }

            let mut src_info = None;
            let mut dst_info = None;

            for r in rows {
                let res_type: String = r.try_get("reservation_type")?;
                let cid: i64 = r.try_get("container_id")?;
                let slot: i32 = r.try_get("slot")?;
                let item_id: Option<String> = r.try_get("item_instance_id")?;

                if res_type == "transfer_out" {
                    src_info = Some((cid as u64, slot as u32, item_id.unwrap_or_default()));
                } else if res_type == "transfer_in" {
                    dst_info = Some((cid as u64, slot as u32));
                }
            }

            let (src_cid, _src_slot, item_id) = src_info
                .ok_or_else(|| StorageError::ReservationExpiredOrAborted(tx_id.to_string()))?;
            let (dst_cid, dst_slot) = dst_info
                .ok_or_else(|| StorageError::ReservationExpiredOrAborted(tx_id.to_string()))?;

            // 2. Atomic check-and-delete verifying lease is still valid
            let deleted = sqlx::query(&q!(
                "DELETE FROM container_reservations WHERE tx_id = ? AND expires_at_epoch_ms > ?"
            ))
            .bind(tx_id)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?
            .rows_affected();

            if deleted < 2 {
                return Err(StorageError::ReservationExpiredOrAborted(tx_id.to_string()));
            }

            // 3. Move the item
            sqlx::query(&q!(
                "UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?"
            ))
            .bind(dst_cid as i64)
            .bind(dst_slot as i32)
            .bind(now_secs)
            .bind(&item_id)
            .execute(&mut *conn)
            .await?;

            // 4. Increment container versions
            sqlx::query(&q!(
                "UPDATE containers SET version = version + 1, updated_at = ? WHERE container_id IN (?, ?)"
            ))
            .bind(now_secs)
            .bind(src_cid as i64)
            .bind(dst_cid as i64)
            .execute(&mut *conn)
            .await?;

            // 5. Insert transaction audit log
            sqlx::query(&q!(
                "INSERT INTO container_transactions (tx_id, src_container_id, dst_container_id, item_instance_id, status, timestamp_epoch_ms) VALUES (?, ?, ?, ?, 'committed', ?)"
            ))
            .bind(tx_id)
            .bind(src_cid as i64)
            .bind(dst_cid as i64)
            .bind(&item_id)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;

            Ok(ContainerTransactionRecord {
                tx_id: tx_id.to_string(),
                src_container_id: src_cid,
                dst_container_id: dst_cid,
                item_instance_id: item_id,
                status: "committed".to_string(),
                timestamp_epoch_ms: now_ms,
            })
        })
    }

    /// Aborts an active transfer reservation and cleans up locks.
    pub async fn abort_transfer(&self, tx_id: &str) -> Result<bool, StorageError> {
        let now_ms = current_time_ms();
        let query_del_str = self
            .db
            .sql("DELETE FROM container_reservations WHERE tx_id = ?");
        let query_ins = self.db.sql(
            "INSERT INTO container_transactions (tx_id, src_container_id, dst_container_id, item_instance_id, status, timestamp_epoch_ms) VALUES (?, 0, 0, '', 'aborted', ?)",
        );

        let rows_affected = match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let affected = sqlx::query(&query_del_str)
                    .bind(tx_id)
                    .execute(pool)
                    .await?
                    .rows_affected();

                if affected > 0 {
                    let _ = sqlx::query(&query_ins)
                        .bind(tx_id)
                        .bind(now_ms)
                        .execute(pool)
                        .await;
                }
                affected
            }
            DbBackend::Postgres(pool) => {
                let affected = sqlx::query(&query_del_str)
                    .bind(tx_id)
                    .execute(pool)
                    .await?
                    .rows_affected();

                if affected > 0 {
                    let _ = sqlx::query(&query_ins)
                        .bind(tx_id)
                        .bind(now_ms)
                        .execute(pool)
                        .await;
                }
                affected
            }
        };

        Ok(rows_affected > 0)
    }

    /// Background lease reaper: aborts all expired reservations.
    pub async fn reap_expired_reservations(&self) -> Result<usize, StorageError> {
        let now_ms = current_time_ms();
        let query_str = self.db.sql(
            "SELECT DISTINCT tx_id FROM container_reservations WHERE expires_at_epoch_ms <= ?",
        );

        let tx_ids: Vec<String> = match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(&query_str).bind(now_ms).fetch_all(pool).await?;
                rows.into_iter()
                    .filter_map(|r| r.try_get("tx_id").ok())
                    .collect()
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(&query_str).bind(now_ms).fetch_all(pool).await?;
                rows.into_iter()
                    .filter_map(|r| r.try_get::<String, _>("tx_id").ok())
                    .collect()
            }
        };

        let mut reaped = 0;
        for tx_id in &tx_ids {
            if self.abort_transfer(tx_id).await? {
                reaped += 1;
            }
        }

        Ok(reaped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container_repo::ContainerRepository;
    use crate::models::{ContainerRecord, ItemRecord};
    use crate::repository::CharacterRepository;

    async fn setup_test_containers(db: &Database) -> (u64, u64) {
        let container_repo = ContainerRepository::new(db.clone());
        let c1 = ContainerRecord {
            container_id: 6001,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            name: "Bags".to_string(),
            capacity: 10,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        let c2 = ContainerRecord {
            container_id: 6002,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            name: "Bank".to_string(),
            capacity: 10,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        container_repo.create_container(&c1).await.unwrap();
        container_repo.create_container(&c2).await.unwrap();
        (6001, 6002)
    }

    #[tokio::test]
    async fn test_2pc_transfer_prepare_and_commit() {
        let db = Database::connect_memory().await.unwrap();
        let (c1, c2) = setup_test_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        // Add item to C1 slot 0
        let item = ItemRecord {
            item_instance_id: "gem_01".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 50,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        // 1. Prepare transfer (C1 slot 0 -> C2 slot 5)
        let now_ms = current_time_ms();
        let res = coord
            .prepare_transfer("tx_success_1", c1, 0, c2, 5, 5000)
            .await
            .unwrap();
        assert_eq!(res.slot, 0);
        assert!(
            res.expires_at_epoch_ms >= now_ms + 4000 && res.expires_at_epoch_ms <= now_ms + 6000
        );

        // 2. Reject concurrent duplicate reservation on same slot
        let dup = coord.prepare_transfer("tx_dup_2", c1, 0, c2, 6, 5000).await;
        assert!(matches!(dup, Err(StorageError::SlotReserved(0, 6001))));

        // 3. Commit transfer
        let tx_rec = coord.commit_transfer("tx_success_1").await.unwrap();
        assert_eq!(tx_rec.status, "committed");

        // 4. Verify item moved
        let item_after = char_repo.get_item("gem_01").await.unwrap().unwrap();
        assert_eq!(item_after.container_id, c2);
        assert_eq!(item_after.slot, 5);

        // 5. Subsequent commit attempt fails
        let second_commit = coord.commit_transfer("tx_success_1").await;
        assert!(second_commit.is_err());
    }

    #[tokio::test]
    async fn test_2pc_transfer_abort_and_rollback() {
        let db = Database::connect_memory().await.unwrap();
        let (c1, c2) = setup_test_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        let item = ItemRecord {
            item_instance_id: "ring_01".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 51,
            slot: 2,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        // Prepare
        coord
            .prepare_transfer("tx_abort_1", c1, 2, c2, 3, 5000)
            .await
            .unwrap();

        // Abort
        let aborted = coord.abort_transfer("tx_abort_1").await.unwrap();
        assert!(aborted);

        // Verify status 'aborted' was recorded in container_transactions
        let tx_row = sqlx::query("SELECT status FROM container_transactions WHERE tx_id = ?")
            .bind("tx_abort_1")
            .fetch_optional(db.sqlite_pool().unwrap())
            .await
            .unwrap();
        assert!(tx_row.is_some());
        assert_eq!(
            tx_row.unwrap().try_get::<String, _>("status").unwrap(),
            "aborted"
        );

        // Verify non-existent abort does not insert into container_transactions
        let non_aborted = coord.abort_transfer("tx_non_existent").await.unwrap();
        assert!(!non_aborted);
        let non_row = sqlx::query("SELECT 1 FROM container_transactions WHERE tx_id = ?")
            .bind("tx_non_existent")
            .fetch_optional(db.sqlite_pool().unwrap())
            .await
            .unwrap();
        assert!(non_row.is_none());

        // Item still in C1 slot 2
        let item_after = char_repo.get_item("ring_01").await.unwrap().unwrap();
        assert_eq!(item_after.container_id, c1);
        assert_eq!(item_after.slot, 2);

        // Commit fails
        let commit_res = coord.commit_transfer("tx_abort_1").await;
        assert!(matches!(
            commit_res,
            Err(StorageError::ReservationExpiredOrAborted(_))
        ));
    }

    #[tokio::test]
    async fn test_2pc_lease_expiry_and_reaper() {
        let db = Database::connect_memory().await.unwrap();
        let (c1, c2) = setup_test_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        let item = ItemRecord {
            item_instance_id: "helm_01".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 52,
            slot: 3,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        // 1. Prepare with 1ms TTL
        coord
            .prepare_transfer("tx_expire_1", c1, 3, c2, 4, 1)
            .await
            .unwrap();

        let item2 = ItemRecord {
            item_instance_id: "helm_02".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 53,
            slot: 4,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item2).await.unwrap();

        coord
            .prepare_transfer("tx_expire_2", c1, 4, c2, 6, 1)
            .await
            .unwrap();

        // Sleep to ensure lease expiration
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;

        // 2. Run reaper (reaps 2 transactions)
        let reaped = coord.reap_expired_reservations().await.unwrap();
        assert_eq!(reaped, 2);

        // Commit fails because reservation was reaped
        let commit_res = coord.commit_transfer("tx_expire_1").await;
        assert!(matches!(
            commit_res,
            Err(StorageError::ReservationExpiredOrAborted(_))
        ));
    }

    #[tokio::test]
    async fn test_2pc_error_branches_and_reserve_slot() {
        let db = Database::connect_memory().await.unwrap();
        let (c1, c2) = setup_test_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        // Exact slot capacity boundary (c2 capacity is 10, slot 10 is out of bounds)
        let err_cap_exact = coord
            .prepare_transfer("tx_cap_exact", c1, 0, c2, 10, 5000)
            .await;
        assert!(matches!(
            err_cap_exact,
            Err(StorageError::SlotOccupied(10, _))
        ));

        let err_res_cap = coord
            .reserve_slot("tx_res_cap", c2, 10, None, "transfer", 5000)
            .await;
        assert!(matches!(
            err_res_cap,
            Err(StorageError::SlotOccupied(10, _))
        ));

        // 1. ContainerNotFound
        let err_c = coord
            .prepare_transfer("tx_err_1", 99999, 0, c2, 1, 5000)
            .await;
        assert!(matches!(err_c, Err(StorageError::ContainerNotFound(99999))));

        // 2. Capacity exceeded / out of bounds
        let err_cap = coord
            .prepare_transfer("tx_err_2", c1, 0, c2, 100, 5000)
            .await;
        assert!(matches!(err_cap, Err(StorageError::SlotOccupied(100, _))));

        // 3. Source item not found
        let err_item = coord.prepare_transfer("tx_err_3", c1, 0, c2, 1, 5000).await;
        assert!(matches!(err_item, Err(StorageError::ItemNotFound(..))));

        // Add items to c1 slot 0 and c2 slot 1
        let item1 = ItemRecord {
            item_instance_id: "edge_item_1".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 10,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        let item2 = ItemRecord {
            item_instance_id: "edge_item_2".to_string(),
            character_guid: 1001,
            container_id: c2,
            item_def_id: 11,
            slot: 1,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item1).await.unwrap();
        char_repo.add_item(&item2).await.unwrap();

        // Test reverse order container IDs: src_container_id > dst_container_id
        let res_rev = coord
            .prepare_transfer("tx_rev_order", c2, 1, c1, 5, 5000)
            .await
            .unwrap();
        assert_eq!(res_rev.container_id, c2);
        coord.abort_transfer("tx_rev_order").await.unwrap();

        // 4. Destination slot occupied
        let err_occ = coord.prepare_transfer("tx_err_4", c1, 0, c2, 1, 5000).await;
        assert!(matches!(err_occ, Err(StorageError::SlotOccupied(1, _))));

        // 5. Test reserve_slot directly with transfer_out and TTL assertion
        let now_ms = current_time_ms();
        let res1 = coord
            .reserve_slot("tx_res_1", c1, 0, Some("edge_item_1"), "transfer_out", 5000)
            .await
            .unwrap();
        assert_eq!(res1.slot, 0);
        assert!(
            res1.expires_at_epoch_ms >= now_ms + 4000 && res1.expires_at_epoch_ms <= now_ms + 6000
        );

        // Mismatched item_instance_id rejected
        let res_mismatch = coord
            .reserve_slot("tx_res_mis", c1, 0, Some("wrong_id"), "transfer_out", 5000)
            .await;
        assert!(matches!(res_mismatch, Err(StorageError::ItemNotFound(..))));

        // reserve_slot transfer_in on empty slot 3 succeeds
        let res_in = coord
            .reserve_slot("tx_res_in", c1, 3, None, "transfer_in", 5000)
            .await
            .unwrap();
        assert_eq!(res_in.slot, 3);

        // reserve_slot transfer_in on occupied slot 0 rejected
        let res_in_occ = coord
            .reserve_slot("tx_res_in_occ", c1, 0, None, "transfer_in", 5000)
            .await;
        assert!(matches!(res_in_occ, Err(StorageError::SlotOccupied(0, _))));

        // Duplicate reservation rejected
        let res_dup = coord
            .reserve_slot("tx_res_2", c1, 0, Some("edge_item_1"), "transfer_out", 5000)
            .await;
        assert!(matches!(res_dup, Err(StorageError::SlotReserved(0, _))));

        // Non-existent abort returns false
        let aborted = coord.abort_transfer("non_existent_tx").await.unwrap();
        assert!(!aborted);
    }

    #[tokio::test]
    async fn test_2pc_commit_partial_reservation_expiry() {
        let db = Database::connect_memory().await.unwrap();
        let (c1, c2) = setup_test_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        let item = ItemRecord {
            item_instance_id: "partial_item".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 100,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        coord
            .prepare_transfer("tx_partial", c1, 0, c2, 5, 5000)
            .await
            .unwrap();

        // Expire only one of the two reservations (the transfer_in one)
        sqlx::query("UPDATE container_reservations SET expires_at_epoch_ms = 0 WHERE tx_id = ? AND reservation_type = 'transfer_in'")
            .bind("tx_partial")
            .execute(db.sqlite_pool().unwrap())
            .await
            .unwrap();

        // commit_transfer must fail because deleted < 2 (only 1 row was unexpired)
        let res = coord.commit_transfer("tx_partial").await;
        assert!(matches!(
            res,
            Err(StorageError::ReservationExpiredOrAborted(_))
        ));
    }

    #[tokio::test]
    async fn test_2pc_asymmetric_container_capacities() {
        let db = Database::connect_memory().await.unwrap();
        let container_repo = ContainerRepository::new(db.clone());
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        let c_small = ContainerRecord {
            container_id: 7001,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            name: "Small".to_string(),
            capacity: 5,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        let c_large = ContainerRecord {
            container_id: 7002,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            name: "Large".to_string(),
            capacity: 20,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        container_repo.create_container(&c_small).await.unwrap();
        container_repo.create_container(&c_large).await.unwrap();

        let item = ItemRecord {
            item_instance_id: "asym_item_1".to_string(),
            character_guid: 1001,
            container_id: 7001,
            item_def_id: 1,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        let item2 = ItemRecord {
            item_instance_id: "asym_item_2".to_string(),
            character_guid: 1001,
            container_id: 7002,
            item_def_id: 2,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item2).await.unwrap();

        // 7001 (cap 5) -> 7002 (cap 20): dst is 7002, slot 10 is VALID (< 20)
        let res1 = coord
            .prepare_transfer("tx_asym_1", 7001, 0, 7002, 10, 5000)
            .await;
        assert!(res1.is_ok());

        // 7002 (cap 20) -> 7001 (cap 5): dst is 7001, slot 10 is INVALID (>= 5)
        let res2 = coord
            .prepare_transfer("tx_asym_2", 7002, 0, 7001, 10, 5000)
            .await;
        assert!(matches!(res2, Err(StorageError::SlotOccupied(10, 7001))));

        // 7002 (cap 20) -> 7001 (cap 5): dst is 7001, slot 3 is VALID (< 5)
        let res3 = coord
            .prepare_transfer("tx_asym_3", 7002, 0, 7001, 3, 5000)
            .await;
        assert!(res3.is_ok());
    }

    #[tokio::test]
    async fn test_2pc_pairwise_reservation_cleanup() {
        let db = Database::connect_memory().await.unwrap();
        let (c1, c2) = setup_test_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        // Place item in C1 slot 2
        let item = ItemRecord {
            item_instance_id: "pair_item".to_string(),
            character_guid: 1001,
            container_id: c1,
            item_def_id: 10,
            slot: 2,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        // Insert expired reservations on orthogonal slots: (C1, slot 5) and (C2, slot 2)
        let now_ms = current_time_ms();
        sqlx::query(
            r#"
            INSERT INTO container_reservations (reservation_id, tx_id, container_id, slot, item_instance_id, reservation_type, expires_at_epoch_ms, created_at_epoch_ms)
            VALUES (?, 'tx_other_1', ?, 5, NULL, 'incoming', ?, ?)
            "#,
        )
        .bind("res_orthogonal_c1_5")
        .bind(c1 as i64)
        .bind(now_ms - 10_000)
        .bind(now_ms - 20_000)
        .execute(db.sqlite_pool().unwrap())
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO container_reservations (reservation_id, tx_id, container_id, slot, item_instance_id, reservation_type, expires_at_epoch_ms, created_at_epoch_ms)
            VALUES (?, 'tx_other_2', ?, 2, 'dummy_item', 'outgoing', ?, ?)
            "#,
        )
        .bind("res_orthogonal_c2_2")
        .bind(c2 as i64)
        .bind(now_ms - 10_000)
        .bind(now_ms - 20_000)
        .execute(db.sqlite_pool().unwrap())
        .await
        .unwrap();

        // Prepare transfer for (C1, slot 2) -> (C2, slot 5)
        let res = coord
            .prepare_transfer("tx_pair_test", c1, 2, c2, 5, 5000)
            .await;
        assert!(
            res.is_ok(),
            "prepare_transfer should succeed, got {:?}",
            res
        );

        // Verify that orthogonal reservations (C1, slot 5) and (C2, slot 2) were NOT deleted
        let res_c1_5 = sqlx::query("SELECT 1 FROM container_reservations WHERE reservation_id = ?")
            .bind("res_orthogonal_c1_5")
            .fetch_optional(db.sqlite_pool().unwrap())
            .await
            .unwrap();
        assert!(
            res_c1_5.is_some(),
            "Orthogonal reservation on (C1, 5) must not be deleted"
        );

        let res_c2_2 = sqlx::query("SELECT 1 FROM container_reservations WHERE reservation_id = ?")
            .bind("res_orthogonal_c2_2")
            .fetch_optional(db.sqlite_pool().unwrap())
            .await
            .unwrap();
        assert!(
            res_c2_2.is_some(),
            "Orthogonal reservation on (C2, 2) must not be deleted"
        );
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
