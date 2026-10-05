//! Cross-Gateway Session Handover Protocol.
//!
//! Provides cryptographic [`HandoverTicket`] generation with HMAC-SHA256,
//! constant-time verification, and token-bucket rate limiting against BGP route flapping.

use dashmap::DashMap;
use ring::hmac::{self, Key, HMAC_SHA256};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

use mmo_core::guid::Guid;

/// Errors that can occur during session handover processing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HandoverError {
    #[error("Invalid or tampered ticket HMAC signature")]
    InvalidSignature,

    #[error("Handover ticket has expired")]
    Expired,

    #[error("Handover rate limit exceeded (BGP flapping protection)")]
    RateLimited,

    #[error("Malformed handover ticket payload: {0}")]
    MalformedTicket(String),

    #[error("Simulation node session takeover rejected: {0}")]
    NodeTakeoverFailed(String),

    #[error("Distributed session cache error: {0}")]
    CacheError(String),
}

/// Cryptographically signed handover ticket authorizing session takeover at an edge gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverTicket {
    pub guid: Guid,
    pub session_id: u64,
    pub session_epoch: u64,
    pub source_gateway_id: String,
    pub issued_at_epoch_ms: u64,
    pub expires_at_epoch_ms: u64,
    pub signature: Vec<u8>,
}

impl HandoverTicket {
    /// Builds the binary digest payload over which HMAC-SHA256 is computed.
    fn payload_bytes(
        guid: Guid,
        session_id: u64,
        session_epoch: u64,
        source_gateway_id: &str,
        issued_at_epoch_ms: u64,
        expires_at_epoch_ms: u64,
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(64 + source_gateway_id.len());
        bytes.extend_from_slice(&guid.raw().to_be_bytes());
        bytes.extend_from_slice(&session_id.to_be_bytes());
        bytes.extend_from_slice(&session_epoch.to_be_bytes());
        bytes.extend_from_slice(&issued_at_epoch_ms.to_be_bytes());
        bytes.extend_from_slice(&expires_at_epoch_ms.to_be_bytes());
        bytes.extend_from_slice(&(source_gateway_id.len() as u16).to_be_bytes());
        bytes.extend_from_slice(source_gateway_id.as_bytes());
        bytes
    }

    /// Creates and signs a new `HandoverTicket`.
    pub fn create(
        guid: Guid,
        session_id: u64,
        session_epoch: u64,
        source_gateway_id: &str,
        secret_key: &[u8],
        ttl_ms: u64,
    ) -> Self {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let expires_at_epoch_ms = now_ms + ttl_ms;

        let payload = Self::payload_bytes(
            guid,
            session_id,
            session_epoch,
            source_gateway_id,
            now_ms,
            expires_at_epoch_ms,
        );

        let key = Key::new(HMAC_SHA256, secret_key);
        let tag = hmac::sign(&key, &payload);

        Self {
            guid,
            session_id,
            session_epoch,
            source_gateway_id: source_gateway_id.to_string(),
            issued_at_epoch_ms: now_ms,
            expires_at_epoch_ms,
            signature: tag.as_ref().to_vec(),
        }
    }

    /// Verifies the HMAC-SHA256 signature and validates expiration timestamp.
    pub fn verify(&self, secret_key: &[u8]) -> Result<(), HandoverError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if now_ms > self.expires_at_epoch_ms {
            return Err(HandoverError::Expired);
        }

        let payload = Self::payload_bytes(
            self.guid,
            self.session_id,
            self.session_epoch,
            &self.source_gateway_id,
            self.issued_at_epoch_ms,
            self.expires_at_epoch_ms,
        );

        let key = Key::new(HMAC_SHA256, secret_key);
        hmac::verify(&key, &payload, &self.signature).map_err(|_| HandoverError::InvalidSignature)
    }

    /// Serializes the ticket to raw binary bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(80 + self.source_gateway_id.len() + self.signature.len());
        buf.extend_from_slice(&self.guid.raw().to_be_bytes());
        buf.extend_from_slice(&self.session_id.to_be_bytes());
        buf.extend_from_slice(&self.session_epoch.to_be_bytes());
        buf.extend_from_slice(&self.issued_at_epoch_ms.to_be_bytes());
        buf.extend_from_slice(&self.expires_at_epoch_ms.to_be_bytes());

        let gw_bytes = self.source_gateway_id.as_bytes();
        buf.extend_from_slice(&(gw_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(gw_bytes);

        buf.extend_from_slice(&(self.signature.len() as u16).to_be_bytes());
        buf.extend_from_slice(&self.signature);

        buf
    }

    /// Deserializes a ticket from binary bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HandoverError> {
        if bytes.len() < 44 {
            return Err(HandoverError::MalformedTicket(
                "Ticket buffer too small".to_string(),
            ));
        }

        let mut offset = 0;
        let guid_raw = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let session_id = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let session_epoch = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let issued_at_epoch_ms = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let expires_at_epoch_ms = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        offset += 8;

        if bytes.len() < offset + 2 {
            return Err(HandoverError::MalformedTicket(
                "Truncated gateway id length".to_string(),
            ));
        }
        let gw_len = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;

        if bytes.len() < offset + gw_len + 2 {
            return Err(HandoverError::MalformedTicket(
                "Truncated gateway id".to_string(),
            ));
        }
        let source_gateway_id = String::from_utf8(bytes[offset..offset + gw_len].to_vec())
            .map_err(|e| HandoverError::MalformedTicket(e.to_string()))?;
        offset += gw_len;

        let sig_len = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;

        if bytes.len() < offset + sig_len {
            return Err(HandoverError::MalformedTicket(
                "Truncated signature".to_string(),
            ));
        }
        let signature = bytes[offset..offset + sig_len].to_vec();

        Ok(Self {
            guid: Guid::from_raw(guid_raw),
            session_id,
            session_epoch,
            source_gateway_id,
            issued_at_epoch_ms,
            expires_at_epoch_ms,
            signature,
        })
    }
}

/// Token-bucket rate limiter per entity GUID protecting against BGP Anycast flapping.
///
/// Refills at 0.2 tokens/second (1 op per 5.0 seconds) with a burst capacity of 2.0.
#[derive(Debug, Default)]
pub struct HandoverRateLimiter {
    states: DashMap<Guid, (f32, Instant)>,
}

impl HandoverRateLimiter {
    pub const REFILL_RATE_PER_SEC: f32 = 0.2; // 1 token every 5.0s
    pub const MAX_BURST: f32 = 2.0;

    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
        }
    }

    /// Evaluates rate limiting for a GUID. Consumes 1.0 token if allowed.
    pub fn check_and_consume(&self, guid: Guid) -> bool {
        self.check_and_consume_at(guid, Instant::now())
    }

    pub fn check_and_consume_at(&self, guid: Guid, now: Instant) -> bool {
        let mut entry = self.states.entry(guid).or_insert((Self::MAX_BURST, now));
        let (tokens, last_refill) = entry.value_mut();

        let elapsed = now.duration_since(*last_refill).as_secs_f32();
        let refilled = (*tokens + elapsed * Self::REFILL_RATE_PER_SEC).min(Self::MAX_BURST);

        *last_refill = now;

        if refilled >= 1.0 {
            *tokens = refilled - 1.0;
            true
        } else {
            *tokens = refilled;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_handover_ticket_creation_and_verification() {
        let secret = b"cluster_shared_secret_key_12345";
        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 42);
        let ticket = HandoverTicket::create(guid, 100, 1, "gw-us-1", secret, 5000);

        assert!(ticket.verify(secret).is_ok());
        assert_eq!(ticket.guid, guid);
        assert_eq!(ticket.session_id, 100);
        assert_eq!(ticket.session_epoch, 1);
        assert_eq!(ticket.source_gateway_id, "gw-us-1");
    }

    #[test]
    fn test_handover_ticket_tampered_signature() {
        let secret = b"cluster_shared_secret_key_12345";
        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 42);
        let mut ticket = HandoverTicket::create(guid, 100, 1, "gw-us-1", secret, 5000);

        ticket.signature[0] ^= 0xFF;
        assert_eq!(
            ticket.verify(secret).unwrap_err(),
            HandoverError::InvalidSignature
        );
    }

    #[test]
    fn test_handover_ticket_expired() {
        let secret = b"cluster_shared_secret_key_12345";
        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 42);
        let mut ticket = HandoverTicket::create(guid, 100, 1, "gw-us-1", secret, 5000);

        // Fast forward expiration timestamp into the past
        ticket.expires_at_epoch_ms = 1;
        assert_eq!(ticket.verify(secret).unwrap_err(), HandoverError::Expired);
    }

    #[test]
    fn test_handover_ticket_bytes_roundtrip() {
        let secret = b"cluster_shared_secret_key_12345";
        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 42);
        let ticket = HandoverTicket::create(guid, 100, 1, "gw-eu-1", secret, 5000);

        let bytes = ticket.to_bytes();
        let decoded = HandoverTicket::from_bytes(&bytes).expect("decoding ticket should succeed");
        assert_eq!(ticket, decoded);
    }

    #[test]
    fn test_handover_rate_limiter_burst_and_refill() {
        let limiter = HandoverRateLimiter::new();
        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 99);
        let t0 = Instant::now();

        // Initial burst capacity allows 2 operations immediately
        assert!(limiter.check_and_consume_at(guid, t0)); // 2.0 -> 1.0
        assert!(limiter.check_and_consume_at(guid, t0)); // 1.0 -> 0.0

        // 3rd immediate attempt is rejected
        assert!(!limiter.check_and_consume_at(guid, t0));

        // After 2.5 seconds, refilled 0.5 tokens (total 0.5) -> still rejected (< 1.0)
        let t1 = t0 + Duration::from_millis(2500);
        assert!(!limiter.check_and_consume_at(guid, t1));

        // After 5.0 seconds total, refilled 1.0 tokens -> accepted
        let t2 = t0 + Duration::from_secs(5);
        assert!(limiter.check_and_consume_at(guid, t2));
    }
}
