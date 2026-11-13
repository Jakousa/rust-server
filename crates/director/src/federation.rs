//! Cross-Cluster Realm Federation and Cryptographic Transfer Claims.
//!
//! Provides the [`RealmFederationAuthority`] issuing and verifying cryptographically signed
//! HMAC-SHA256 transfer claim tokens with 64-bit nonces, bounded TTL, and replay protection.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ring::hmac::{self, Key, HMAC_SHA256};
use thiserror::Error;

/// Default maximum number of unexpired nonces retained in the replay protection cache.
pub const DEFAULT_MAX_NONCE_CACHE: usize = 100_000;

/// Errors that can occur during realm transfer claim issuance or verification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FederationError {
    #[error("Invalid or tampered HMAC signature on transfer claim")]
    InvalidSignature,

    #[error("Transfer claim has expired")]
    Expired,

    #[error("Transfer claim falls within evicted replay watermark window")]
    ReplayWindowExceeded,

    #[error("Replay attack detected: nonce {0} was already redeemed")]
    ReplayDetected(u64),

    #[error("Target realm mismatch: expected {expected}, got {got}")]
    TargetRealmMismatch { expected: String, got: String },

    #[error("Secret key is too short (must be at least 16 bytes)")]
    KeyTooShort,
}

/// A cryptographically verified realm transfer claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmTransferClaim {
    pub guid: u64,
    pub source_realm_id: String,
    pub target_realm_id: String,
    pub session_epoch: u64,
    pub nonce: u64,
    pub expires_at_epoch_ms: u64,
    pub signature: Vec<u8>,
}

/// Authority managing cross-realm identity claims and replay prevention.
pub struct RealmFederationAuthority {
    realm_id: String,
    secret_key: Vec<u8>,
    /// Cache of redeemed nonces mapped to `(expiry_instant, expires_at_epoch_ms)`.
    redeemed_nonces: HashMap<u64, (Instant, u64)>,
    /// Ordered index of `(expires_at_epoch_ms, nonce)` for O(log N) oldest-entry eviction.
    expiry_order: BTreeSet<(u64, u64)>,
    /// Floor watermark (in epoch ms) of evicted nonces; claims at or below this are rejected.
    min_issued_at_ms: u64,
    /// Maximum capacity of the redeemed nonce cache before oldest-entry eviction occurs.
    max_nonce_cache: usize,
    /// Nonce counter for generating unique tokens.
    next_nonce: u64,
}

impl RealmFederationAuthority {
    /// Creates a new `RealmFederationAuthority` for the given `realm_id`.
    pub fn new(realm_id: impl Into<String>, secret_key: Vec<u8>) -> Result<Self, FederationError> {
        Self::with_max_nonce_cache(realm_id, secret_key, DEFAULT_MAX_NONCE_CACHE)
    }

    /// Creates a new `RealmFederationAuthority` with a configurable nonce cache capacity.
    pub fn with_max_nonce_cache(
        realm_id: impl Into<String>,
        secret_key: Vec<u8>,
        max_nonce_cache: usize,
    ) -> Result<Self, FederationError> {
        if secret_key.len() < 16 {
            return Err(FederationError::KeyTooShort);
        }
        let now_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Ok(Self {
            realm_id: realm_id.into(),
            secret_key,
            redeemed_nonces: HashMap::new(),
            expiry_order: BTreeSet::new(),
            min_issued_at_ms: 0,
            max_nonce_cache: max_nonce_cache.max(1),
            next_nonce: now_nanos,
        })
    }

    /// Returns the local realm identifier managed by this authority.
    pub fn realm_id(&self) -> &str {
        &self.realm_id
    }

    /// Returns the current replay eviction watermark (epoch milliseconds).
    pub fn min_issued_at_ms(&self) -> u64 {
        self.min_issued_at_ms
    }

    /// Returns the number of nonces currently tracked in the replay cache.
    pub fn redeemed_nonce_count(&self) -> usize {
        self.redeemed_nonces.len()
    }

    /// Constructs the canonical binary digest over which HMAC-SHA256 is evaluated.
    fn payload_bytes(
        guid: u64,
        source_realm_id: &str,
        target_realm_id: &str,
        session_epoch: u64,
        nonce: u64,
        expires_at_epoch_ms: u64,
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(64 + source_realm_id.len() + target_realm_id.len());
        bytes.extend_from_slice(&guid.to_be_bytes());
        bytes.extend_from_slice(&(source_realm_id.len() as u16).to_be_bytes());
        bytes.extend_from_slice(source_realm_id.as_bytes());
        bytes.extend_from_slice(&(target_realm_id.len() as u16).to_be_bytes());
        bytes.extend_from_slice(target_realm_id.as_bytes());
        bytes.extend_from_slice(&session_epoch.to_be_bytes());
        bytes.extend_from_slice(&nonce.to_be_bytes());
        bytes.extend_from_slice(&expires_at_epoch_ms.to_be_bytes());
        bytes
    }

    /// Issues a signed `RealmTransferClaim` for cross-cluster player transfer.
    pub fn issue_claim(
        &mut self,
        guid: u64,
        target_realm_id: &str,
        session_epoch: u64,
        ttl: Duration,
    ) -> RealmTransferClaim {
        let max_ttl = Duration::from_secs(86_400);
        let safe_ttl = ttl.min(max_ttl);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let expires_at_epoch_ms = now_ms.saturating_add(safe_ttl.as_millis() as u64);

        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.wrapping_add(1);

        let payload = Self::payload_bytes(
            guid,
            &self.realm_id,
            target_realm_id,
            session_epoch,
            nonce,
            expires_at_epoch_ms,
        );

        let key = Key::new(HMAC_SHA256, &self.secret_key);
        let signature = hmac::sign(&key, &payload).as_ref().to_vec();

        RealmTransferClaim {
            guid,
            source_realm_id: self.realm_id.clone(),
            target_realm_id: target_realm_id.to_string(),
            session_epoch,
            nonce,
            expires_at_epoch_ms,
            signature,
        }
    }

    /// Verifies a `RealmTransferClaim` received by this realm.
    ///
    /// Validates HMAC signature in constant time, checks expiration, ensures target matches
    /// this realm, and guards against replay attacks by registering the nonce.
    pub fn verify_and_redeem_claim(
        &mut self,
        claim: &RealmTransferClaim,
    ) -> Result<(), FederationError> {
        self.clean_expired_nonces();

        if claim.target_realm_id != self.realm_id {
            return Err(FederationError::TargetRealmMismatch {
                expected: self.realm_id.clone(),
                got: claim.target_realm_id.clone(),
            });
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if now_ms >= claim.expires_at_epoch_ms {
            return Err(FederationError::Expired);
        }

        if self.min_issued_at_ms > 0 && claim.expires_at_epoch_ms <= self.min_issued_at_ms {
            return Err(FederationError::ReplayWindowExceeded);
        }

        if self.redeemed_nonces.contains_key(&claim.nonce) {
            return Err(FederationError::ReplayDetected(claim.nonce));
        }

        let payload = Self::payload_bytes(
            claim.guid,
            &claim.source_realm_id,
            &claim.target_realm_id,
            claim.session_epoch,
            claim.nonce,
            claim.expires_at_epoch_ms,
        );

        let key = Key::new(HMAC_SHA256, &self.secret_key);
        hmac::verify(&key, &payload, &claim.signature)
            .map_err(|_| FederationError::InvalidSignature)?;

        let remaining_ms = claim.expires_at_epoch_ms.saturating_sub(now_ms);
        let max_ttl = Duration::from_secs(86_400);
        let safe_remaining = Duration::from_millis(remaining_ms).min(max_ttl);
        let expiry_instant = Instant::now()
            .checked_add(safe_remaining)
            .unwrap_or_else(|| Instant::now() + max_ttl);

        if self.redeemed_nonces.len() >= self.max_nonce_cache {
            self.clean_expired_nonces();
            while self.redeemed_nonces.len() >= self.max_nonce_cache {
                let Some(&(oldest_exp_ms, oldest_nonce)) = self.expiry_order.first() else {
                    break;
                };
                if oldest_exp_ms >= claim.expires_at_epoch_ms {
                    return Err(FederationError::ReplayWindowExceeded);
                }
                self.expiry_order.remove(&(oldest_exp_ms, oldest_nonce));
                self.redeemed_nonces.remove(&oldest_nonce);
                self.min_issued_at_ms = self.min_issued_at_ms.max(oldest_exp_ms);
            }
        }
        self.redeemed_nonces
            .insert(claim.nonce, (expiry_instant, claim.expires_at_epoch_ms));
        self.expiry_order
            .insert((claim.expires_at_epoch_ms, claim.nonce));

        Ok(())
    }

    /// Evicts expired nonce entries from the replay cache.
    pub fn clean_expired_nonces(&mut self) {
        let now = Instant::now();
        let expiry_order = &mut self.expiry_order;
        self.redeemed_nonces
            .retain(|&nonce, &mut (expiry, exp_ms)| {
                if expiry > now {
                    true
                } else {
                    expiry_order.remove(&(exp_ms, nonce));
                    false
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_issue_and_verification() {
        let secret = b"super-secret-cluster-federation-key-123456";
        let mut source_auth =
            RealmFederationAuthority::new("realm-us-east", secret.to_vec()).unwrap();
        let mut target_auth =
            RealmFederationAuthority::new("realm-eu-central", secret.to_vec()).unwrap();

        let claim = source_auth.issue_claim(42, "realm-eu-central", 1, Duration::from_secs(10));
        assert_eq!(claim.source_realm_id, "realm-us-east");
        assert_eq!(claim.target_realm_id, "realm-eu-central");

        // First verification should succeed
        assert!(target_auth.verify_and_redeem_claim(&claim).is_ok());

        // Second verification should trigger replay protection
        let err = target_auth
            .verify_and_redeem_claim(&claim)
            .expect_err("replay should fail");
        assert_eq!(err, FederationError::ReplayDetected(claim.nonce));
    }

    #[test]
    fn test_tampered_claim_fails() {
        let secret = b"super-secret-cluster-federation-key-123456";
        let mut source_auth =
            RealmFederationAuthority::new("realm-us-east", secret.to_vec()).unwrap();
        let mut target_auth =
            RealmFederationAuthority::new("realm-eu-central", secret.to_vec()).unwrap();

        let mut claim = source_auth.issue_claim(42, "realm-eu-central", 1, Duration::from_secs(10));
        claim.guid = 999; // Tampered GUID

        let err = target_auth
            .verify_and_redeem_claim(&claim)
            .expect_err("tampered claim must fail");
        assert_eq!(err, FederationError::InvalidSignature);
    }

    #[test]
    fn test_nonce_cache_watermark_eviction_never_clears_unexpired_nonces() {
        let secret = b"super-secret-cluster-federation-key-123456";
        let mut source_auth =
            RealmFederationAuthority::new("realm-us-east", secret.to_vec()).unwrap();
        let mut target_auth =
            RealmFederationAuthority::with_max_nonce_cache("realm-eu-central", secret.to_vec(), 3)
                .unwrap();

        let early_claim =
            source_auth.issue_claim(1, "realm-eu-central", 1, Duration::from_secs(10));
        let mid_claim_a =
            source_auth.issue_claim(2, "realm-eu-central", 1, Duration::from_secs(60));
        let mid_claim_b =
            source_auth.issue_claim(3, "realm-eu-central", 1, Duration::from_secs(60));
        let newer_claim =
            source_auth.issue_claim(4, "realm-eu-central", 1, Duration::from_secs(120));

        target_auth.verify_and_redeem_claim(&early_claim).unwrap();
        target_auth.verify_and_redeem_claim(&mid_claim_a).unwrap();
        target_auth.verify_and_redeem_claim(&mid_claim_b).unwrap();
        assert_eq!(target_auth.redeemed_nonce_count(), 3);

        // Redeeming newer_claim at capacity evicts only the oldest entry (early_claim)
        // and advances min_issued_at_ms without clearing mid_claim_a/mid_claim_b.
        target_auth.verify_and_redeem_claim(&newer_claim).unwrap();
        assert_eq!(target_auth.redeemed_nonce_count(), 3);
        assert_eq!(
            target_auth.min_issued_at_ms(),
            early_claim.expires_at_epoch_ms
        );

        // Unexpired nonces still in cache must still be rejected as replays
        assert_eq!(
            target_auth.verify_and_redeem_claim(&mid_claim_a),
            Err(FederationError::ReplayDetected(mid_claim_a.nonce))
        );
        assert_eq!(
            target_auth.verify_and_redeem_claim(&mid_claim_b),
            Err(FederationError::ReplayDetected(mid_claim_b.nonce))
        );

        // Replaying the evicted early_claim must be blocked by the watermark floor
        assert_eq!(
            target_auth.verify_and_redeem_claim(&early_claim),
            Err(FederationError::ReplayWindowExceeded)
        );

        // A claim older than or equal to the watermark must also be rejected
        let stale_window_claim =
            source_auth.issue_claim(5, "realm-eu-central", 1, Duration::from_secs(5));
        assert_eq!(
            target_auth.verify_and_redeem_claim(&stale_window_claim),
            Err(FederationError::ReplayWindowExceeded)
        );
    }

    #[test]
    fn test_nonce_cache_at_capacity_rejects_older_claim_without_evicting_newer() {
        let secret = b"super-secret-cluster-federation-key-123456";
        let mut source_auth =
            RealmFederationAuthority::new("realm-us-east", secret.to_vec()).unwrap();
        let mut target_auth =
            RealmFederationAuthority::with_max_nonce_cache("realm-eu-central", secret.to_vec(), 2)
                .unwrap();

        let long_a = source_auth.issue_claim(1, "realm-eu-central", 1, Duration::from_secs(120));
        let long_b = source_auth.issue_claim(2, "realm-eu-central", 1, Duration::from_secs(120));
        let shorter = source_auth.issue_claim(3, "realm-eu-central", 1, Duration::from_secs(30));

        target_auth.verify_and_redeem_claim(&long_a).unwrap();
        target_auth.verify_and_redeem_claim(&long_b).unwrap();

        // Shorter claim cannot evict newer claims in cache
        assert_eq!(
            target_auth.verify_and_redeem_claim(&shorter),
            Err(FederationError::ReplayWindowExceeded)
        );
        assert_eq!(target_auth.redeemed_nonce_count(), 2);
        assert_eq!(
            target_auth.verify_and_redeem_claim(&long_a),
            Err(FederationError::ReplayDetected(long_a.nonce))
        );
    }
}
