//! Generic Asynchronous Escrow State Machine for conditional container exchanges.

use crate::db::{sql_for_dialect, Database, DbBackend, StorageError};
use crate::models::{EscrowRecord, EscrowStatus};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

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

/// Computes hex-encoded SHA-256 hash of a secret string.
pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generic Escrow coordinator managing conditional holding, settlement, and expiration refunds.
#[derive(Clone)]
pub struct EscrowCoordinator {
    db: Database,
}

impl EscrowCoordinator {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Deposits items from the depositor container into a dedicated escrow container.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_escrow(
        &self,
        escrow_id: &str,
        depositor_container_id: u64,
        beneficiary_container_id: u64,
        escrow_container_id: u64,
        item_instance_ids: &[String],
        secret: &str,
        ttl_seconds: u64,
    ) -> Result<EscrowRecord, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();
        let expires_at_ms = now_ms + (ttl_seconds as i64 * 1000);
        let secret_hash = hash_secret(secret);

        if item_instance_ids.is_empty() {
            return Err(StorageError::TradeError(
                "No items provided for escrow".to_string(),
            ));
        }

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut conn = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

                let res: Result<EscrowRecord, StorageError> = async {
                    // 1. Verify depositor and escrow containers exist
                    let c_dep = sqlx::query("SELECT container_id FROM containers WHERE container_id = ?")
                        .bind(depositor_container_id as i64)
                        .fetch_optional(&mut *conn)
                        .await?;
                    let c_esc = sqlx::query("SELECT container_id, capacity FROM containers WHERE container_id = ?")
                        .bind(escrow_container_id as i64)
                        .fetch_optional(&mut *conn)
                        .await?;

                    if c_dep.is_none() {
                        return Err(StorageError::ContainerNotFound(depositor_container_id));
                    }
                    let esc_row = c_esc.ok_or(StorageError::ContainerNotFound(escrow_container_id))?;
                    let esc_cap: i32 = esc_row.try_get("capacity")?;

                    // 2. Fetch occupied slots in escrow container
                    let occupied_rows = sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                        .bind(escrow_container_id as i64)
                        .fetch_all(&mut *conn)
                        .await?;
                    let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let mut free_slots = Vec::new();
                    for s in 0..(esc_cap as u32) {
                        if !occupied_slots.contains(&s) {
                            free_slots.push(s);
                        }
                    }

                    if free_slots.len() < item_instance_ids.len() {
                        return Err(StorageError::ContainerFull(escrow_container_id, esc_cap as u32));
                    }

                    // 3. Move items into escrow container
                    for (item_id, target_slot) in item_instance_ids.iter().zip(free_slots.iter()) {
                        let item_row = sqlx::query("SELECT container_id FROM items WHERE item_instance_id = ?")
                            .bind(item_id)
                            .fetch_optional(&mut *conn)
                            .await?;

                        match item_row {
                            Some(r) => {
                                let cid: i64 = r.try_get("container_id")?;
                                if cid as u64 != depositor_container_id {
                                    return Err(StorageError::ItemNotFound(item_id.clone(), depositor_container_id));
                                }
                            }
                            None => return Err(StorageError::ItemNotFound(item_id.clone(), depositor_container_id)),
                        }

                        sqlx::query("UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?")
                            .bind(escrow_container_id as i64)
                            .bind(*target_slot as i32)
                            .bind(now_secs)
                            .bind(item_id)
                            .execute(&mut *conn)
                            .await?;
                    }

                    // 4. Insert escrow record
                    sqlx::query(
                        r#"
                        INSERT INTO escrows (escrow_id, depositor_container_id, beneficiary_container_id, escrow_container_id, status, secret_hash, expires_at_epoch_ms, created_at_epoch_ms, updated_at_epoch_ms)
                        VALUES (?, ?, ?, ?, 'held', ?, ?, ?, ?)
                        "#
                    )
                    .bind(escrow_id)
                    .bind(depositor_container_id as i64)
                    .bind(beneficiary_container_id as i64)
                    .bind(escrow_container_id as i64)
                    .bind(&secret_hash)
                    .bind(expires_at_ms)
                    .bind(now_ms)
                    .bind(now_ms)
                    .execute(&mut *conn)
                    .await?;

                    Ok(EscrowRecord {
                        escrow_id: escrow_id.to_string(),
                        depositor_container_id,
                        beneficiary_container_id,
                        escrow_container_id,
                        status: EscrowStatus::Held,
                        secret_hash,
                        expires_at_epoch_ms: expires_at_ms,
                        created_at_epoch_ms: now_ms,
                        updated_at_epoch_ms: now_ms,
                    })
                }.await;

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
                Self::create_escrow_postgres(
                    pool,
                    escrow_id,
                    depositor_container_id,
                    beneficiary_container_id,
                    escrow_container_id,
                    item_instance_ids,
                    secret_hash,
                    expires_at_ms,
                    now_ms,
                    now_secs,
                )
                .await
            }
        }
    }

    /// Releases escrowed items to the beneficiary after verifying the authorization secret in constant time.
    pub async fn release_escrow(
        &self,
        escrow_id: &str,
        secret: &str,
    ) -> Result<bool, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();
        let candidate_hash = hash_secret(secret);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut conn = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

                let res: Result<bool, StorageError> = async {
                    let row = sqlx::query(
                        "SELECT beneficiary_container_id, escrow_container_id, status, secret_hash, expires_at_epoch_ms FROM escrows WHERE escrow_id = ?"
                    )
                    .bind(escrow_id)
                    .fetch_optional(&mut *conn)
                    .await?;

                    let r = row.ok_or_else(|| StorageError::EscrowNotFound(escrow_id.to_string()))?;
                    let status: String = r.try_get("status")?;
                    let stored_hash: String = r.try_get("secret_hash")?;
                    let expires_at: i64 = r.try_get("expires_at_epoch_ms")?;
                    let ben_cid: i64 = r.try_get("beneficiary_container_id")?;
                    let esc_cid: i64 = r.try_get("escrow_container_id")?;

                    if status != "held" {
                        return Err(StorageError::EscrowAlreadySettled(escrow_id.to_string()));
                    }
                    if now_ms >= expires_at {
                        return Err(StorageError::EscrowExpired(escrow_id.to_string()));
                    }

                    // Constant-time comparison preventing timing attacks
                    if candidate_hash.as_bytes().ct_eq(stored_hash.as_bytes()).unwrap_u8() != 1 {
                        return Err(StorageError::EscrowSecretMismatch(escrow_id.to_string()));
                    }

                    // Fetch beneficiary capacity
                    let ben_row = sqlx::query("SELECT capacity FROM containers WHERE container_id = ?")
                        .bind(ben_cid)
                        .fetch_optional(&mut *conn)
                        .await?;
                    let ben_cap: i32 = ben_row
                        .ok_or(StorageError::ContainerNotFound(ben_cid as u64))?
                        .try_get("capacity")?;

                    let ben_occupied = sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                        .bind(ben_cid)
                        .fetch_all(&mut *conn)
                        .await?;
                    let occupied_slots: std::collections::HashSet<u32> = ben_occupied
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let mut free_slots = Vec::new();
                    for s in 0..(ben_cap as u32) {
                        if !occupied_slots.contains(&s) {
                            free_slots.push(s);
                        }
                    }

                    let escrow_items = sqlx::query("SELECT item_instance_id FROM items WHERE container_id = ?")
                        .bind(esc_cid)
                        .fetch_all(&mut *conn)
                        .await?;

                    if free_slots.len() < escrow_items.len() {
                        return Err(StorageError::ContainerFull(ben_cid as u64, ben_cap as u32));
                    }

                    for (item_row, target_slot) in escrow_items.iter().zip(free_slots.iter()) {
                        let item_id: String = item_row.try_get("item_instance_id")?;
                        sqlx::query("UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?")
                            .bind(ben_cid)
                            .bind(*target_slot as i32)
                            .bind(now_secs)
                            .bind(&item_id)
                            .execute(&mut *conn)
                            .await?;
                    }

                    sqlx::query("UPDATE escrows SET status = 'settled', updated_at_epoch_ms = ? WHERE escrow_id = ?")
                        .bind(now_ms)
                        .bind(escrow_id)
                        .execute(&mut *conn)
                        .await?;

                    Ok(true)
                }.await;

                match res {
                    Ok(b) => {
                        sqlx::query("COMMIT").execute(&mut *conn).await?;
                        Ok(b)
                    }
                    Err(e) => {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                        Err(e)
                    }
                }
            }
            DbBackend::Postgres(pool) => {
                Self::release_escrow_postgres(pool, escrow_id, &candidate_hash, now_ms, now_secs)
                    .await
            }
        }
    }

    /// Refunds all escrowed items back to the depositor upon cancellation or timeout.
    pub async fn refund_escrow(&self, escrow_id: &str) -> Result<bool, StorageError> {
        let now_ms = current_time_ms();
        let now_secs = current_time_secs();

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut conn = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

                let res: Result<bool, StorageError> = async {
                    let row = sqlx::query(
                        "SELECT depositor_container_id, escrow_container_id, status FROM escrows WHERE escrow_id = ?"
                    )
                    .bind(escrow_id)
                    .fetch_optional(&mut *conn)
                    .await?;

                    let r = row.ok_or_else(|| StorageError::EscrowNotFound(escrow_id.to_string()))?;
                    let status: String = r.try_get("status")?;
                    let dep_cid: i64 = r.try_get("depositor_container_id")?;
                    let esc_cid: i64 = r.try_get("escrow_container_id")?;

                    if status != "held" {
                        return Ok(false);
                    }

                    // Move items back to depositor
                    let dep_row = sqlx::query("SELECT capacity FROM containers WHERE container_id = ?")
                        .bind(dep_cid)
                        .fetch_optional(&mut *conn)
                        .await?;
                    let dep_cap: i32 = dep_row
                        .ok_or(StorageError::ContainerNotFound(dep_cid as u64))?
                        .try_get("capacity")?;

                    let dep_occupied = sqlx::query("SELECT slot FROM items WHERE container_id = ?")
                        .bind(dep_cid)
                        .fetch_all(&mut *conn)
                        .await?;
                    let occupied_slots: std::collections::HashSet<u32> = dep_occupied
                        .into_iter()
                        .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                        .collect();

                    let mut free_slots = Vec::new();
                    for s in 0..(dep_cap as u32) {
                        if !occupied_slots.contains(&s) {
                            free_slots.push(s);
                        }
                    }

                    let escrow_items = sqlx::query("SELECT item_instance_id FROM items WHERE container_id = ?")
                        .bind(esc_cid)
                        .fetch_all(&mut *conn)
                        .await?;

                    for (item_row, target_slot) in escrow_items.iter().zip(free_slots.iter()) {
                        let item_id: String = item_row.try_get("item_instance_id")?;
                        sqlx::query("UPDATE items SET container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?")
                            .bind(dep_cid)
                            .bind(*target_slot as i32)
                            .bind(now_secs)
                            .bind(&item_id)
                            .execute(&mut *conn)
                            .await?;
                    }

                    sqlx::query("UPDATE escrows SET status = 'refunded', updated_at_epoch_ms = ? WHERE escrow_id = ?")
                        .bind(now_ms)
                        .bind(escrow_id)
                        .execute(&mut *conn)
                        .await?;

                    Ok(true)
                }.await;

                match res {
                    Ok(b) => {
                        sqlx::query("COMMIT").execute(&mut *conn).await?;
                        Ok(b)
                    }
                    Err(e) => {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                        Err(e)
                    }
                }
            }
            DbBackend::Postgres(pool) => {
                Self::refund_escrow_postgres(pool, escrow_id, now_ms, now_secs).await
            }
        }
    }

    /// Background reaper: finds expired escrows in 'held' status and refunds them.
    pub async fn reap_expired_escrows(&self) -> Result<usize, StorageError> {
        let now_ms = current_time_ms();
        let query =
            "SELECT escrow_id FROM escrows WHERE status = 'held' AND expires_at_epoch_ms <= ?";
        let query_str = sql_for_dialect(query, self.db.is_postgres());

        let escrow_ids: Vec<String> = match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(&query_str).bind(now_ms).fetch_all(pool).await?;
                rows.into_iter()
                    .filter_map(|r| r.try_get("escrow_id").ok())
                    .collect()
            }
            DbBackend::Postgres(pool) => {
                Self::reap_expired_escrows_postgres(pool, &query_str, now_ms).await?
            }
        };

        use futures_util::StreamExt;

        let mut reaped = 0;
        let mut stream = futures_util::stream::iter(escrow_ids.iter())
            .map(|id| self.refund_escrow(id))
            .buffer_unordered(10);

        while let Some(res) = stream.next().await {
            if res? {
                reaped += 1;
            }
        }

        Ok(reaped)
    }

    /// Retrieves an escrow record by ID.
    pub async fn get_escrow(&self, escrow_id: &str) -> Result<Option<EscrowRecord>, StorageError> {
        let query = "SELECT escrow_id, depositor_container_id, beneficiary_container_id, escrow_container_id, status, secret_hash, expires_at_epoch_ms, created_at_epoch_ms, updated_at_epoch_ms FROM escrows WHERE escrow_id = ?";
        let query_sql = sql_for_dialect(query, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&query_sql)
                    .bind(escrow_id)
                    .fetch_optional(pool)
                    .await?;
                match row {
                    Some(r) => {
                        let status_str: String = r.try_get("status")?;
                        let status = match status_str.as_str() {
                            "held" => EscrowStatus::Held,
                            "settled" => EscrowStatus::Settled,
                            "refunded" => EscrowStatus::Refunded,
                            _ => {
                                return Err(StorageError::TradeError(format!(
                                    "Unknown escrow status: {}",
                                    status_str
                                )))
                            }
                        };
                        let dep_id: i64 = r.try_get("depositor_container_id")?;
                        let ben_id: i64 = r.try_get("beneficiary_container_id")?;
                        let esc_id: i64 = r.try_get("escrow_container_id")?;
                        let expires: i64 = r.try_get("expires_at_epoch_ms")?;
                        let created: i64 = r.try_get("created_at_epoch_ms")?;
                        let updated: i64 = r.try_get("updated_at_epoch_ms")?;

                        Ok(Some(EscrowRecord {
                            escrow_id: r.try_get("escrow_id")?,
                            depositor_container_id: dep_id as u64,
                            beneficiary_container_id: ben_id as u64,
                            escrow_container_id: esc_id as u64,
                            status,
                            secret_hash: r.try_get("secret_hash")?,
                            expires_at_epoch_ms: expires,
                            created_at_epoch_ms: created,
                            updated_at_epoch_ms: updated,
                        }))
                    }
                    None => Ok(None),
                }
            }
            DbBackend::Postgres(pool) => {
                Self::get_escrow_postgres(pool, &query_sql, escrow_id).await
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn create_escrow_postgres(
        pool: &sqlx::PgPool,
        escrow_id: &str,
        depositor_container_id: u64,
        beneficiary_container_id: u64,
        escrow_container_id: u64,
        item_instance_ids: &[String],
        secret_hash: String,
        expires_at_ms: i64,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<EscrowRecord, StorageError> {
        let mut tx = pool.begin().await?;

        let c_dep = sqlx::query::<sqlx::Postgres>(
            "SELECT container_id FROM containers WHERE container_id = $1 FOR UPDATE",
        )
        .bind(depositor_container_id as i64)
        .fetch_optional(&mut *tx)
        .await?;
        let c_esc = sqlx::query::<sqlx::Postgres>(
            "SELECT container_id, capacity FROM containers WHERE container_id = $1 FOR UPDATE",
        )
        .bind(escrow_container_id as i64)
        .fetch_optional(&mut *tx)
        .await?;

        if c_dep.is_none() {
            return Err(StorageError::ContainerNotFound(depositor_container_id));
        }
        let esc_row = c_esc.ok_or(StorageError::ContainerNotFound(escrow_container_id))?;
        let esc_cap: i32 = esc_row.try_get("capacity")?;

        let occupied_rows =
            sqlx::query::<sqlx::Postgres>("SELECT slot FROM items WHERE container_id = $1")
                .bind(escrow_container_id as i64)
                .fetch_all(&mut *tx)
                .await?;
        let occupied_slots: std::collections::HashSet<u32> = occupied_rows
            .into_iter()
            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
            .collect();

        let mut free_slots = Vec::new();
        for s in 0..(esc_cap as u32) {
            if !occupied_slots.contains(&s) {
                free_slots.push(s);
            }
        }

        if free_slots.len() < item_instance_ids.len() {
            return Err(StorageError::ContainerFull(
                escrow_container_id,
                esc_cap as u32,
            ));
        }

        for (item_id, target_slot) in item_instance_ids.iter().zip(free_slots.iter()) {
            let item_row = sqlx::query::<sqlx::Postgres>(
                "SELECT container_id FROM items WHERE item_instance_id = $1",
            )
            .bind(item_id)
            .fetch_optional(&mut *tx)
            .await?;

            match item_row {
                Some(r) => {
                    let cid: i64 = r.try_get("container_id")?;
                    if cid as u64 != depositor_container_id {
                        return Err(StorageError::ItemNotFound(
                            item_id.clone(),
                            depositor_container_id,
                        ));
                    }
                }
                None => {
                    return Err(StorageError::ItemNotFound(
                        item_id.clone(),
                        depositor_container_id,
                    ))
                }
            }

            sqlx::query::<sqlx::Postgres>(
                "UPDATE items SET container_id = $1, slot = $2, updated_at = $3 WHERE item_instance_id = $4",
            )
            .bind(escrow_container_id as i64)
            .bind(*target_slot as i32)
            .bind(now_secs)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query::<sqlx::Postgres>(
            r#"
            INSERT INTO escrows (escrow_id, depositor_container_id, beneficiary_container_id, escrow_container_id, status, secret_hash, expires_at_epoch_ms, created_at_epoch_ms, updated_at_epoch_ms)
            VALUES ($1, $2, $3, $4, 'held', $5, $6, $7, $8)
            "#,
        )
        .bind(escrow_id)
        .bind(depositor_container_id as i64)
        .bind(beneficiary_container_id as i64)
        .bind(escrow_container_id as i64)
        .bind(&secret_hash)
        .bind(expires_at_ms)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(EscrowRecord {
            escrow_id: escrow_id.to_string(),
            depositor_container_id,
            beneficiary_container_id,
            escrow_container_id,
            status: EscrowStatus::Held,
            secret_hash,
            expires_at_epoch_ms: expires_at_ms,
            created_at_epoch_ms: now_ms,
            updated_at_epoch_ms: now_ms,
        })
    }

    async fn release_escrow_postgres(
        pool: &sqlx::PgPool,
        escrow_id: &str,
        candidate_hash: &str,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<bool, StorageError> {
        let mut tx = pool.begin().await?;

        let row = sqlx::query::<sqlx::Postgres>(
            "SELECT beneficiary_container_id, escrow_container_id, status, secret_hash, expires_at_epoch_ms FROM escrows WHERE escrow_id = $1 FOR UPDATE",
        )
        .bind(escrow_id)
        .fetch_optional(&mut *tx)
        .await?;

        let r = row.ok_or_else(|| StorageError::EscrowNotFound(escrow_id.to_string()))?;
        let status: String = r.try_get("status")?;
        let stored_hash: String = r.try_get("secret_hash")?;
        let expires_at: i64 = r.try_get("expires_at_epoch_ms")?;
        let ben_cid: i64 = r.try_get("beneficiary_container_id")?;
        let esc_cid: i64 = r.try_get("escrow_container_id")?;

        if status != "held" {
            return Err(StorageError::EscrowAlreadySettled(escrow_id.to_string()));
        }
        if now_ms >= expires_at {
            return Err(StorageError::EscrowExpired(escrow_id.to_string()));
        }

        if candidate_hash
            .as_bytes()
            .ct_eq(stored_hash.as_bytes())
            .unwrap_u8()
            != 1
        {
            return Err(StorageError::EscrowSecretMismatch(escrow_id.to_string()));
        }

        let ben_row = sqlx::query::<sqlx::Postgres>(
            "SELECT capacity FROM containers WHERE container_id = $1 FOR UPDATE",
        )
        .bind(ben_cid)
        .fetch_optional(&mut *tx)
        .await?;
        let ben_cap: i32 = ben_row
            .ok_or(StorageError::ContainerNotFound(ben_cid as u64))?
            .try_get("capacity")?;

        let ben_occupied =
            sqlx::query::<sqlx::Postgres>("SELECT slot FROM items WHERE container_id = $1")
                .bind(ben_cid)
                .fetch_all(&mut *tx)
                .await?;
        let occupied_slots: std::collections::HashSet<u32> = ben_occupied
            .into_iter()
            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
            .collect();

        let mut free_slots = Vec::new();
        for s in 0..(ben_cap as u32) {
            if !occupied_slots.contains(&s) {
                free_slots.push(s);
            }
        }

        let escrow_items = sqlx::query::<sqlx::Postgres>(
            "SELECT item_instance_id FROM items WHERE container_id = $1",
        )
        .bind(esc_cid)
        .fetch_all(&mut *tx)
        .await?;

        if free_slots.len() < escrow_items.len() {
            return Err(StorageError::ContainerFull(ben_cid as u64, ben_cap as u32));
        }

        for (item_row, target_slot) in escrow_items.iter().zip(free_slots.iter()) {
            let item_id: String = item_row.try_get("item_instance_id")?;
            sqlx::query::<sqlx::Postgres>(
                "UPDATE items SET container_id = $1, slot = $2, updated_at = $3 WHERE item_instance_id = $4",
            )
            .bind(ben_cid)
            .bind(*target_slot as i32)
            .bind(now_secs)
            .bind(&item_id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query::<sqlx::Postgres>(
            "UPDATE escrows SET status = 'settled', updated_at_epoch_ms = $1 WHERE escrow_id = $2",
        )
        .bind(now_ms)
        .bind(escrow_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    }

    async fn refund_escrow_postgres(
        pool: &sqlx::PgPool,
        escrow_id: &str,
        now_ms: i64,
        now_secs: i64,
    ) -> Result<bool, StorageError> {
        let mut tx = pool.begin().await?;

        let row = sqlx::query::<sqlx::Postgres>(
            "SELECT depositor_container_id, escrow_container_id, status FROM escrows WHERE escrow_id = $1 FOR UPDATE",
        )
        .bind(escrow_id)
        .fetch_optional(&mut *tx)
        .await?;

        let r = row.ok_or_else(|| StorageError::EscrowNotFound(escrow_id.to_string()))?;
        let status: String = r.try_get("status")?;
        let dep_cid: i64 = r.try_get("depositor_container_id")?;
        let esc_cid: i64 = r.try_get("escrow_container_id")?;

        if status != "held" {
            return Ok(false);
        }

        let dep_row = sqlx::query::<sqlx::Postgres>(
            "SELECT capacity FROM containers WHERE container_id = $1 FOR UPDATE",
        )
        .bind(dep_cid)
        .fetch_optional(&mut *tx)
        .await?;
        let dep_cap: i32 = dep_row
            .ok_or(StorageError::ContainerNotFound(dep_cid as u64))?
            .try_get("capacity")?;

        let dep_occupied =
            sqlx::query::<sqlx::Postgres>("SELECT slot FROM items WHERE container_id = $1")
                .bind(dep_cid)
                .fetch_all(&mut *tx)
                .await?;
        let occupied_slots: std::collections::HashSet<u32> = dep_occupied
            .into_iter()
            .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
            .collect();

        let mut free_slots = Vec::new();
        for s in 0..(dep_cap as u32) {
            if !occupied_slots.contains(&s) {
                free_slots.push(s);
            }
        }

        let escrow_items = sqlx::query::<sqlx::Postgres>(
            "SELECT item_instance_id FROM items WHERE container_id = $1",
        )
        .bind(esc_cid)
        .fetch_all(&mut *tx)
        .await?;

        for (item_row, target_slot) in escrow_items.iter().zip(free_slots.iter()) {
            let item_id: String = item_row.try_get("item_instance_id")?;
            sqlx::query::<sqlx::Postgres>(
                "UPDATE items SET container_id = $1, slot = $2, updated_at = $3 WHERE item_instance_id = $4",
            )
            .bind(dep_cid)
            .bind(*target_slot as i32)
            .bind(now_secs)
            .bind(&item_id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query::<sqlx::Postgres>(
            "UPDATE escrows SET status = 'refunded', updated_at_epoch_ms = $1 WHERE escrow_id = $2",
        )
        .bind(now_ms)
        .bind(escrow_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    }

    async fn reap_expired_escrows_postgres(
        pool: &sqlx::PgPool,
        query_str: &str,
        now_ms: i64,
    ) -> Result<Vec<String>, StorageError> {
        let rows = sqlx::query::<sqlx::Postgres>(query_str)
            .bind(now_ms)
            .fetch_all(pool)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.try_get("escrow_id").ok())
            .collect())
    }

    async fn get_escrow_postgres(
        pool: &sqlx::PgPool,
        query_sql: &str,
        escrow_id: &str,
    ) -> Result<Option<EscrowRecord>, StorageError> {
        let row = sqlx::query::<sqlx::Postgres>(query_sql)
            .bind(escrow_id)
            .fetch_optional(pool)
            .await?;
        match row {
            Some(r) => {
                let status_str: String = r.try_get("status")?;
                let status = match status_str.as_str() {
                    "held" => EscrowStatus::Held,
                    "settled" => EscrowStatus::Settled,
                    "refunded" => EscrowStatus::Refunded,
                    _ => {
                        return Err(StorageError::TradeError(format!(
                            "Unknown escrow status: {}",
                            status_str
                        )))
                    }
                };
                let dep_id: i64 = r.try_get("depositor_container_id")?;
                let ben_id: i64 = r.try_get("beneficiary_container_id")?;
                let esc_id: i64 = r.try_get("escrow_container_id")?;
                let expires: i64 = r.try_get("expires_at_epoch_ms")?;
                let created: i64 = r.try_get("created_at_epoch_ms")?;
                let updated: i64 = r.try_get("updated_at_epoch_ms")?;

                Ok(Some(EscrowRecord {
                    escrow_id: r.try_get("escrow_id")?,
                    depositor_container_id: dep_id as u64,
                    beneficiary_container_id: ben_id as u64,
                    escrow_container_id: esc_id as u64,
                    status,
                    secret_hash: r.try_get("secret_hash")?,
                    expires_at_epoch_ms: expires,
                    created_at_epoch_ms: created,
                    updated_at_epoch_ms: updated,
                }))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container_repo::ContainerRepository;
    use crate::repository::CharacterRepository;
    use crate::test_helpers::{ContainerRecordBuilder, ItemRecordBuilder};

    async fn setup_escrow_containers(db: &Database) -> (u64, u64, u64) {
        let container_repo = ContainerRepository::new(db.clone());
        let c1 = ContainerRecordBuilder::new(7001)
            .owner_id("1001")
            .name("Player A")
            .build();
        let c2 = ContainerRecordBuilder::new(7002)
            .owner_id("1002")
            .name("Player B")
            .build();
        let c3 = ContainerRecordBuilder::new(7003)
            .owner_type("system")
            .owner_id("escrow")
            .name("System Escrow")
            .build();
        container_repo.create_container(&c1).await.unwrap();
        container_repo.create_container(&c2).await.unwrap();
        container_repo.create_container(&c3).await.unwrap();
        (7001, 7002, 7003)
    }

    #[tokio::test]
    async fn test_escrow_deposit_and_release() {
        let db = Database::connect_memory().await.unwrap();
        let (c_dep, c_ben, c_esc) = setup_escrow_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let escrow_coord = EscrowCoordinator::new(db.clone());

        let item = ItemRecordBuilder::new("escrow_gem")
            .container_id(c_dep)
            .item_def_id(88)
            .build();
        char_repo.add_item(&item).await.unwrap();

        let now_ms = current_time_ms();
        // 1. Create escrow
        let escrow = escrow_coord
            .create_escrow(
                "escrow_01",
                c_dep,
                c_ben,
                c_esc,
                &["escrow_gem".to_string()],
                "secret_key_123",
                60,
            )
            .await
            .unwrap();
        assert_eq!(escrow.status, EscrowStatus::Held);
        assert!(
            escrow.expires_at_epoch_ms >= now_ms + 50_000
                && escrow.expires_at_epoch_ms <= now_ms + 70_000
        );

        // Verify item is now in escrow container
        let item_in_esc = char_repo.get_item("escrow_gem").await.unwrap().unwrap();
        assert_eq!(item_in_esc.container_id, c_esc);

        // 2. Reject release with invalid secret
        let bad_release = escrow_coord
            .release_escrow("escrow_01", "wrong_secret")
            .await;
        assert!(matches!(
            bad_release,
            Err(StorageError::EscrowSecretMismatch(_))
        ));

        // 3. Release with correct secret
        let released = escrow_coord
            .release_escrow("escrow_01", "secret_key_123")
            .await
            .unwrap();
        assert!(released);

        // Verify item is now with beneficiary
        let item_in_ben = char_repo.get_item("escrow_gem").await.unwrap().unwrap();
        assert_eq!(item_in_ben.container_id, c_ben);

        // 4. Repeated release rejected
        let second_release = escrow_coord
            .release_escrow("escrow_01", "secret_key_123")
            .await;
        assert!(matches!(
            second_release,
            Err(StorageError::EscrowAlreadySettled(_))
        ));
    }

    #[tokio::test]
    async fn test_escrow_refund_manual() {
        let db = Database::connect_memory().await.unwrap();
        let (c_dep, c_ben, c_esc) = setup_escrow_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let escrow_coord = EscrowCoordinator::new(db.clone());

        let item = ItemRecordBuilder::new("refund_gem")
            .container_id(c_dep)
            .item_def_id(89)
            .build();
        char_repo.add_item(&item).await.unwrap();

        escrow_coord
            .create_escrow(
                "escrow_refund_01",
                c_dep,
                c_ben,
                c_esc,
                &["refund_gem".to_string()],
                "secret_refund",
                60,
            )
            .await
            .unwrap();

        // Refund
        let refunded = escrow_coord
            .refund_escrow("escrow_refund_01")
            .await
            .unwrap();
        assert!(refunded);

        // Verify item returned to depositor
        let item_after = char_repo.get_item("refund_gem").await.unwrap().unwrap();
        assert_eq!(item_after.container_id, c_dep);
    }

    #[tokio::test]
    async fn test_escrow_expiry_and_reaper() {
        let db = Database::connect_memory().await.unwrap();
        let (c_dep, c_ben, c_esc) = setup_escrow_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let escrow_coord = EscrowCoordinator::new(db.clone());

        let item = ItemRecordBuilder::new("expire_gem")
            .container_id(c_dep)
            .item_def_id(90)
            .build();
        char_repo.add_item(&item).await.unwrap();

        // Create with 0s TTL (immediately expired)
        escrow_coord
            .create_escrow(
                "escrow_expire_01",
                c_dep,
                c_ben,
                c_esc,
                &["expire_gem".to_string()],
                "secret_expire",
                0,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let reaped = escrow_coord.reap_expired_escrows().await.unwrap();
        assert_eq!(reaped, 1);

        let item_after = char_repo.get_item("expire_gem").await.unwrap().unwrap();
        assert_eq!(item_after.container_id, c_dep);
    }

    #[tokio::test]
    async fn test_escrow_expiry_multiple_concurrent_reaper() {
        let db = Database::connect_memory().await.unwrap();
        let container_repo = ContainerRepository::new(db.clone());
        let c_dep = 7101;
        let c_ben = 7102;
        let c_esc = 7103;
        container_repo
            .create_container(&ContainerRecordBuilder::new(c_dep).capacity(30).build())
            .await
            .unwrap();
        container_repo
            .create_container(&ContainerRecordBuilder::new(c_ben).capacity(30).build())
            .await
            .unwrap();
        container_repo
            .create_container(
                &ContainerRecordBuilder::new(c_esc)
                    .capacity(30)
                    .owner_type("system")
                    .build(),
            )
            .await
            .unwrap();
        let char_repo = CharacterRepository::new(db.clone());
        let escrow_coord = EscrowCoordinator::new(db.clone());

        for i in 0..15 {
            let item_id = format!("expire_gem_{i}");
            let item = ItemRecordBuilder::new(&item_id)
                .container_id(c_dep)
                .slot(i)
                .item_def_id(90)
                .build();
            char_repo.add_item(&item).await.unwrap();

            escrow_coord
                .create_escrow(
                    &format!("escrow_expire_{i}"),
                    c_dep,
                    c_ben,
                    c_esc,
                    &[item_id],
                    &format!("secret_{i}"),
                    0,
                )
                .await
                .unwrap();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let reaped = escrow_coord.reap_expired_escrows().await.unwrap();
        assert_eq!(reaped, 15);

        for i in 0..15 {
            let item_after = char_repo
                .get_item(&format!("expire_gem_{i}"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(item_after.container_id, c_dep);
        }
    }

    #[tokio::test]
    async fn test_escrow_error_branches_and_helpers() {
        assert_eq!(hash_secret("abc"), hash_secret("abc"));
        assert_ne!(hash_secret("abc"), hash_secret("def"));

        let db = Database::connect_memory().await.unwrap();
        let (c_dep, c_ben, c_esc) = setup_escrow_containers(&db).await;
        let escrow_coord = EscrowCoordinator::new(db.clone());

        // 1. get_escrow for non-existent returns None
        assert!(escrow_coord
            .get_escrow("no_such_escrow")
            .await
            .unwrap()
            .is_none());

        // 2. release_escrow on non-existent returns EscrowNotFound
        let release_err = escrow_coord.release_escrow("missing", "secret").await;
        assert!(matches!(release_err, Err(StorageError::EscrowNotFound(_))));

        // 3. refund_escrow on non-existent returns EscrowNotFound
        let refund_err = escrow_coord.refund_escrow("missing").await;
        assert!(matches!(refund_err, Err(StorageError::EscrowNotFound(_))));

        // 4. create_escrow with invalid container returns ContainerNotFound
        let bad_cont = escrow_coord
            .create_escrow(
                "err_esc_1",
                99999,
                c_ben,
                c_esc,
                &["dummy".to_string()],
                "s",
                60,
            )
            .await;
        assert!(matches!(
            bad_cont,
            Err(StorageError::ContainerNotFound(99999))
        ));

        // 5. create_escrow with missing item returns ItemNotFound
        let bad_item = escrow_coord
            .create_escrow(
                "err_esc_2",
                c_dep,
                c_ben,
                c_esc,
                &["non_existent_item".to_string()],
                "s",
                60,
            )
            .await;
        assert!(matches!(bad_item, Err(StorageError::ItemNotFound(..))));
    }

    #[tokio::test]
    async fn test_escrow_exact_capacity_boundaries() {
        let db = Database::connect_memory().await.unwrap();
        let container_repo = ContainerRepository::new(db.clone());
        let char_repo = CharacterRepository::new(db.clone());
        let escrow_coord = EscrowCoordinator::new(db.clone());

        let c_dep = ContainerRecordBuilder::new(8001)
            .owner_id("1001")
            .capacity(10)
            .build();
        let c_ben_1 = ContainerRecordBuilder::new(8002)
            .owner_id("1002")
            .capacity(1)
            .build();
        let c_esc_1 = ContainerRecordBuilder::new(8003)
            .owner_type("system")
            .owner_id("escrow")
            .capacity(1)
            .build();
        container_repo.create_container(&c_dep).await.unwrap();
        container_repo.create_container(&c_ben_1).await.unwrap();
        container_repo.create_container(&c_esc_1).await.unwrap();

        let item1 = ItemRecordBuilder::new("exact_gem_1")
            .container_id(8001)
            .slot(0)
            .build();
        let item2 = ItemRecordBuilder::new("exact_gem_2")
            .container_id(8001)
            .slot(1)
            .build();
        char_repo.add_item(&item1).await.unwrap();
        char_repo.add_item(&item2).await.unwrap();

        // 1. Exact capacity match for create_escrow (1 free slot, 1 item) -> SUCCEEDS
        let esc1 = escrow_coord
            .create_escrow(
                "escrow_exact_1",
                8001,
                8002,
                8003,
                &["exact_gem_1".to_string()],
                "secret_exact_1",
                60,
            )
            .await;
        assert!(esc1.is_ok());

        // 2. Escrow container 8003 is now full (0 free slots, 1 item) -> FAILS ContainerFull
        let esc_full = escrow_coord
            .create_escrow(
                "escrow_exact_full",
                8001,
                8002,
                8003,
                &["exact_gem_2".to_string()],
                "secret_exact_full",
                60,
            )
            .await;
        assert!(matches!(
            esc_full,
            Err(StorageError::ContainerFull(8003, 1))
        ));

        // 3. Exact capacity match for release_escrow (c_ben_1 has capacity 1, 0 items) -> SUCCEEDS
        let rel1 = escrow_coord
            .release_escrow("escrow_exact_1", "secret_exact_1")
            .await;
        assert!(rel1.unwrap());

        // 4. Beneficiary 8002 is now full (capacity 1, 1 item).
        // Create another escrow into a fresh escrow container (8004) with capacity 5
        let c_esc_2 = ContainerRecordBuilder::new(8004)
            .owner_type("system")
            .owner_id("escrow")
            .capacity(5)
            .build();
        container_repo.create_container(&c_esc_2).await.unwrap();

        escrow_coord
            .create_escrow(
                "escrow_exact_2",
                8001,
                8002,
                8004,
                &["exact_gem_2".to_string()],
                "secret_exact_2",
                60,
            )
            .await
            .unwrap();

        // Releasing to full beneficiary (0 free slots) -> FAILS ContainerFull
        let rel_full = escrow_coord
            .release_escrow("escrow_exact_2", "secret_exact_2")
            .await;
        assert!(matches!(
            rel_full,
            Err(StorageError::ContainerFull(8002, 1))
        ));
    }

    #[test]
    fn test_time_helpers() {
        let ms = current_time_ms();
        let secs = current_time_secs();

        // Timestamps should be positive and well past year 2023 (> 1_700_000_000)
        assert!(secs > 1_700_000_000);
        assert!(ms > 1_700_000_000_000);
        assert!(ms > secs * 900);
        assert!(ms >= secs * 1000);
    }

    #[tokio::test]
    async fn test_get_escrow_status_parsing_and_transitions() {
        let db = Database::connect_memory().await.unwrap();
        let (c_dep, c_ben, c_esc) = setup_escrow_containers(&db).await;
        let char_repo = CharacterRepository::new(db.clone());
        let escrow_coord = EscrowCoordinator::new(db.clone());

        // Add items to depositor container
        let item1 = ItemRecordBuilder::new("gem_status_1")
            .container_id(c_dep)
            .item_def_id(91)
            .slot(0)
            .build();
        let item2 = ItemRecordBuilder::new("gem_status_2")
            .container_id(c_dep)
            .item_def_id(92)
            .slot(1)
            .build();
        char_repo.add_item(&item1).await.unwrap();
        char_repo.add_item(&item2).await.unwrap();

        // 1. Create escrow -> status "held"
        escrow_coord
            .create_escrow(
                "escrow_status_held",
                c_dep,
                c_ben,
                c_esc,
                &["gem_status_1".to_string()],
                "secret_1",
                60,
            )
            .await
            .unwrap();

        let held_record = escrow_coord
            .get_escrow("escrow_status_held")
            .await
            .unwrap()
            .expect("record exists");
        assert_eq!(held_record.status, EscrowStatus::Held);

        // Release escrow -> status "settled"
        let released = escrow_coord
            .release_escrow("escrow_status_held", "secret_1")
            .await
            .unwrap();
        assert!(released);

        let settled_record = escrow_coord
            .get_escrow("escrow_status_held")
            .await
            .unwrap()
            .expect("record exists");
        assert_eq!(settled_record.status, EscrowStatus::Settled);

        // Releasing already settled escrow returns EscrowAlreadySettled
        let re_release = escrow_coord
            .release_escrow("escrow_status_held", "secret_1")
            .await;
        assert!(matches!(
            re_release,
            Err(StorageError::EscrowAlreadySettled(_))
        ));

        // Refunding already settled escrow returns false
        let re_refund = escrow_coord
            .refund_escrow("escrow_status_held")
            .await
            .unwrap();
        assert!(!re_refund);

        // 2. Create second escrow -> refund -> status "refunded"
        escrow_coord
            .create_escrow(
                "escrow_status_refund",
                c_dep,
                c_ben,
                c_esc,
                &["gem_status_2".to_string()],
                "secret_2",
                60,
            )
            .await
            .unwrap();

        let refunded = escrow_coord
            .refund_escrow("escrow_status_refund")
            .await
            .unwrap();
        assert!(refunded);

        let refunded_record = escrow_coord
            .get_escrow("escrow_status_refund")
            .await
            .unwrap()
            .expect("record exists");
        assert_eq!(refunded_record.status, EscrowStatus::Refunded);

        // Releasing already refunded escrow returns EscrowAlreadySettled (status != "held")
        let release_refunded = escrow_coord
            .release_escrow("escrow_status_refund", "secret_2")
            .await;
        assert!(matches!(
            release_refunded,
            Err(StorageError::EscrowAlreadySettled(_))
        ));

        // Refunding already refunded escrow returns false
        let refund_again = escrow_coord
            .refund_escrow("escrow_status_refund")
            .await
            .unwrap();
        assert!(!refund_again);

        // 3. Test unknown status parsing error branch
        if let DbBackend::Sqlite(pool) = db.backend() {
            sqlx::query("UPDATE escrows SET status = 'corrupted_status' WHERE escrow_id = 'escrow_status_held'")
                .execute(pool)
                .await
                .unwrap();
            let err_parse = escrow_coord.get_escrow("escrow_status_held").await;
            assert!(matches!(err_parse, Err(StorageError::TradeError(_))));
        }
    }
}
