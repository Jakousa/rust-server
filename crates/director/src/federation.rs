//! Cross-Cluster Realm Federation and Cryptographic Transfer Claims.
//!
//! Provides the [`RealmFederationAuthority`] issuing and verifying cryptographically signed
//! HMAC-SHA256 transfer claim tokens with 64-bit nonces, bounded TTL, and replay protection.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ring::hmac::{self, Key, HMAC_SHA256};
use thiserror::Error;

/// Errors that can occur during realm transfer claim issuance or verification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FederationError {
    #[error("Invalid or tampered HMAC signature on transfer claim")]
    InvalidSignature,

    #[error("Transfer claim has expired")]
    Expired,

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
    /// Cache of redeemed nonces mapped to their expiry Instant.
    redeemed_nonces: HashMap<u64, Instant>,
    /// Nonce counter for generating unique tokens.
    next_nonce: u64,
}

impl RealmFederationAuthority {
    /// Creates a new `RealmFederationAuthority` for the given `realm_id`.
    pub fn new(realm_id: impl Into<String>, secret_key: Vec<u8>) -> Result<Self, FederationError> {
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
            next_nonce: now_nanos,
        })
    }

    /// Returns the local realm identifier managed by this authority.
    pub fn realm_id(&self) -> &str {
        &self.realm_id
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

        if self.redeemed_nonces.len() >= 100_000 {
            self.clean_expired_nonces();
            if self.redeemed_nonces.len() >= 100_000 {
                self.redeemed_nonces.clear();
            }
        }
        self.redeemed_nonces.insert(claim.nonce, expiry_instant);

        Ok(())
    }

    /// Evicts expired nonce entries from the replay cache.
    pub fn clean_expired_nonces(&mut self) {
        let now = Instant::now();
        self.redeemed_nonces.retain(|_, expiry| *expiry > now);
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
}
