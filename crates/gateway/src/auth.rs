//! Edge Gateway Authentication and Zero-Trust Admission Control.
//!
//! Provides cryptographic JWT/OIDC token verification, cross-gateway handover
//! ticket validation, and identity binding for incoming client connections.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use mmo_core::guid::Guid;
use mmo_schema::client::auth_error_codes;

use crate::handover::{HandoverError, HandoverRateLimiter, HandoverTicket};

/// Authentication errors occurring during token or ticket validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("Invalid token format or encoding: {0}")]
    InvalidToken(String),

    #[error("Token has expired")]
    ExpiredToken,

    #[error("Cryptographic signature verification failed")]
    InvalidSignature,

    #[error("Token claims validation failed: {0}")]
    InvalidClaims(String),

    #[error("Algorithm not allowed: {0}")]
    AlgorithmNotAllowed(String),

    #[error("Handover ticket error: {0}")]
    Handover(#[from] HandoverError),

    #[error("Authentication timed out")]
    Timeout,

    #[error("Client protocol version mismatch")]
    VersionMismatch,
}

impl AuthError {
    /// Maps internal authentication error to standard client protocol error code.
    pub fn to_error_code(&self) -> u32 {
        match self {
            AuthError::InvalidToken(_) => auth_error_codes::INVALID_TOKEN,
            AuthError::ExpiredToken => auth_error_codes::EXPIRED_TOKEN,
            AuthError::InvalidSignature => auth_error_codes::INVALID_SIGNATURE,
            AuthError::InvalidClaims(_) => auth_error_codes::INVALID_CLAIMS,
            AuthError::AlgorithmNotAllowed(_) => auth_error_codes::INVALID_TOKEN,
            AuthError::Handover(HandoverError::Expired) => auth_error_codes::EXPIRED_TOKEN,
            AuthError::Handover(HandoverError::InvalidSignature) => {
                auth_error_codes::INVALID_SIGNATURE
            }
            AuthError::Handover(HandoverError::RateLimited) => auth_error_codes::RATE_LIMITED,
            AuthError::Handover(_) => auth_error_codes::INVALID_TOKEN,
            AuthError::Timeout => auth_error_codes::TIMEOUT,
            AuthError::VersionMismatch => auth_error_codes::VERSION_MISMATCH,
        }
    }
}

/// Standard OIDC/JWT claims payload parsed from client identity tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwtClaims {
    /// Subject identifier (numeric player GUID string or account ID).
    pub sub: String,
    /// Issuer URL or authority string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// Target audience identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// Expiration timestamp in seconds since UNIX epoch.
    pub exp: u64,
    /// Not before timestamp in seconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
    /// Issued at timestamp in seconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
    /// Optional explicit raw 64-bit GUID override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<u64>,
    /// Optional user authorization roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}

/// Cryptographic JWT validator enforcing strict algorithm whitelisting and claims validation.
#[derive(Clone)]
pub struct TokenValidator {
    algorithm: Algorithm,
    decoding_key: DecodingKey,
    expected_issuer: Option<String>,
    expected_audience: Option<String>,
}

impl TokenValidator {
    /// Creates a new validator using HMAC-SHA256 (HS256) and a shared secret.
    pub fn new_hs256(
        secret: &[u8],
        expected_issuer: Option<String>,
        expected_audience: Option<String>,
    ) -> Self {
        Self {
            algorithm: Algorithm::HS256,
            decoding_key: DecodingKey::from_secret(secret),
            expected_issuer,
            expected_audience,
        }
    }

    /// Creates a new validator using Ed25519 (EdDSA) and a public key in PEM or DER format.
    pub fn new_eddsa(
        public_key_pem: &[u8],
        expected_issuer: Option<String>,
        expected_audience: Option<String>,
    ) -> Result<Self, AuthError> {
        let decoding_key = DecodingKey::from_ed_pem(public_key_pem)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;
        Ok(Self {
            algorithm: Algorithm::EdDSA,
            decoding_key,
            expected_issuer,
            expected_audience,
        })
    }

    /// Creates a new validator using RSA (RS256) and an RSA public key in PEM format.
    pub fn new_rs256(
        public_key_pem: &[u8],
        expected_issuer: Option<String>,
        expected_audience: Option<String>,
    ) -> Result<Self, AuthError> {
        let decoding_key = DecodingKey::from_rsa_pem(public_key_pem)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;
        Ok(Self {
            algorithm: Algorithm::RS256,
            decoding_key,
            expected_issuer,
            expected_audience,
        })
    }

    /// Validates an incoming JWT token string and returns parsed claims on success.
    pub fn validate(&self, token_str: &str) -> Result<JwtClaims, AuthError> {
        // Enforce strict allowed algorithms and reject 'none' or unapproved algorithms
        let mut validation = Validation::new(self.algorithm);
        validation.algorithms = vec![self.algorithm];
        validation.validate_exp = true;
        validation.leeway = 60; // 60 seconds clock skew tolerance

        if let Some(ref iss) = self.expected_issuer {
            validation.set_issuer(&[iss]);
        } else {
            validation.validate_nbf = false;
        }

        if let Some(ref aud) = self.expected_audience {
            validation.set_audience(&[aud]);
        } else {
            validation.aud = None;
        }

        let token_data = decode::<JwtClaims>(token_str, &self.decoding_key, &validation).map_err(
            |err| match err.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::ExpiredToken,
                jsonwebtoken::errors::ErrorKind::InvalidSignature => AuthError::InvalidSignature,
                jsonwebtoken::errors::ErrorKind::InvalidAlgorithmName
                | jsonwebtoken::errors::ErrorKind::InvalidAlgorithm => {
                    AuthError::AlgorithmNotAllowed("Algorithm not permitted".to_string())
                }
                jsonwebtoken::errors::ErrorKind::InvalidIssuer
                | jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                    AuthError::InvalidClaims(err.to_string())
                }
                _ => AuthError::InvalidToken(err.to_string()),
            },
        )?;

        Ok(token_data.claims)
    }

    /// Resolves an authoritative `Guid` from validated claims.
    pub fn resolve_guid(claims: &JwtClaims) -> Result<Guid, AuthError> {
        if let Some(raw_guid) = claims.guid {
            return Ok(Guid::from_raw(raw_guid));
        }

        if let Ok(raw_numeric) = claims.sub.parse::<u64>() {
            return Ok(Guid::from_raw(raw_numeric));
        }

        Err(AuthError::InvalidClaims(format!(
            "Subject '{}' cannot be converted into entity GUID",
            claims.sub
        )))
    }
}

/// Result of successful client admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResult {
    /// Authoritative entity identity.
    pub guid: Guid,
    /// Authorized roles assigned to player.
    pub roles: Vec<String>,
    /// Whether authentication was performed via a cross-gateway handover ticket.
    pub is_handover: bool,
    /// Monotonic session epoch (e.g. from handover ticket or 1 for fresh session).
    pub session_epoch: u64,
}

/// Unified Edge Authenticator managing JWT/OIDC tokens and handover ticket validation.
#[derive(Clone)]
pub struct EdgeAuthenticator {
    token_validator: Option<TokenValidator>,
    handover_secret: Option<Vec<u8>>,
    handover_rate_limiter: Arc<HandoverRateLimiter>,
    auth_timeout: Duration,
}

impl EdgeAuthenticator {
    /// Creates a new `EdgeAuthenticator` backed by an HS256 JWT validator.
    pub fn new_hs256(
        secret: &[u8],
        expected_issuer: Option<String>,
        expected_audience: Option<String>,
    ) -> Self {
        Self {
            token_validator: Some(TokenValidator::new_hs256(
                secret,
                expected_issuer,
                expected_audience,
            )),
            handover_secret: None,
            handover_rate_limiter: Arc::new(HandoverRateLimiter::new()),
            auth_timeout: Duration::from_secs(5),
        }
    }

    /// Creates a new `EdgeAuthenticator` with custom `TokenValidator`.
    pub fn with_validator(validator: TokenValidator) -> Self {
        Self {
            token_validator: Some(validator),
            handover_secret: None,
            handover_rate_limiter: Arc::new(HandoverRateLimiter::new()),
            auth_timeout: Duration::from_secs(5),
        }
    }

    /// Configures the shared secret for validating migrating `HandoverTicket` tokens.
    pub fn with_handover_secret(mut self, secret: Vec<u8>) -> Self {
        self.handover_secret = Some(secret);
        self
    }

    /// Sets the maximum allowable handshake timeout window.
    pub fn with_auth_timeout(mut self, timeout: Duration) -> Self {
        self.auth_timeout = timeout;
        self
    }

    /// Returns the configured authentication timeout.
    pub fn auth_timeout(&self) -> Duration {
        self.auth_timeout
    }

    /// Authenticates an incoming token string.
    ///
    /// Supports standard JWT format (`header.payload.signature`) as well as base64-encoded `HandoverTicket`.
    pub fn authenticate(&self, token_str: &str) -> Result<AuthResult, AuthError> {
        metrics::counter!("mmo_gateway_auth_attempts_total").increment(1);

        // Check if token is a 3-part JWT
        let parts: Vec<&str> = token_str.split('.').collect();
        if parts.len() == 3 {
            if let Some(ref validator) = self.token_validator {
                match validator.validate(token_str) {
                    Ok(claims) => {
                        let guid = TokenValidator::resolve_guid(&claims)?;
                        let roles = claims.roles.unwrap_or_default();
                        metrics::counter!("mmo_gateway_auth_success_total").increment(1);
                        return Ok(AuthResult {
                            guid,
                            roles,
                            is_handover: false,
                            session_epoch: 1,
                        });
                    }
                    Err(e) => {
                        let reason = match e {
                            AuthError::ExpiredToken => "expired",
                            AuthError::InvalidSignature => "signature",
                            AuthError::AlgorithmNotAllowed(_) => "algorithm",
                            AuthError::InvalidClaims(_) => "claims",
                            _ => "invalid_token",
                        };
                        metrics::counter!("mmo_gateway_auth_failures_total", "reason" => reason)
                            .increment(1);
                        return Err(e);
                    }
                }
            }
        }

        // Try parsing as base64 HandoverTicket if handover secret is configured
        if let Some(ref secret) = self.handover_secret {
            if let Ok(ticket_bytes) = data_encoding::BASE64URL_NOPAD
                .decode(token_str.as_bytes())
                .or_else(|_| data_encoding::BASE64.decode(token_str.as_bytes()))
            {
                if let Ok(ticket) = HandoverTicket::from_bytes(&ticket_bytes) {
                    if ticket.verify(secret).is_ok() {
                        if !self.handover_rate_limiter.check_and_consume(ticket.guid) {
                            metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "rate_limited").increment(1);
                            return Err(AuthError::Handover(HandoverError::RateLimited));
                        }
                        metrics::counter!("mmo_gateway_auth_success_total").increment(1);
                        return Ok(AuthResult {
                            guid: ticket.guid,
                            roles: vec![],
                            is_handover: true,
                            session_epoch: ticket.session_epoch,
                        });
                    }
                }
            }
        }

        metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "invalid_token")
            .increment(1);
        Err(AuthError::InvalidToken(
            "Unrecognized token format or missing validator".to_string(),
        ))
    }

    /// Convenience helper to create a signed test JWT for unit tests.
    pub fn create_test_jwt(
        guid: Guid,
        secret: &[u8],
        ttl_secs: u64,
        issuer: Option<&str>,
        audience: Option<&str>,
    ) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = JwtClaims {
            sub: guid.raw().to_string(),
            iss: issuer.map(|s| s.to_string()),
            aud: audience.map(|s| s.to_string()),
            exp: now + ttl_secs,
            nbf: Some(now.saturating_sub(10)),
            iat: Some(now),
            guid: Some(guid.raw()),
            roles: Some(vec!["player".to_string()]),
        };

        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .expect("Encoding test JWT should succeed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    const TEST_SECRET: &[u8] = b"super_secret_edge_authentication_key_1234567890";

    #[test]
    fn test_valid_jwt_authentication() {
        let guid = Guid::new(EntityType::Player, 1, 42);
        let auth = EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("mmo-auth".to_string()),
            Some("mmo-gateway".to_string()),
        );

        let token = EdgeAuthenticator::create_test_jwt(
            guid,
            TEST_SECRET,
            3600,
            Some("mmo-auth"),
            Some("mmo-gateway"),
        );

        let result = auth
            .authenticate(&token)
            .expect("authentication should pass");
        assert_eq!(result.guid, guid);
        assert!(!result.is_handover);
        assert_eq!(result.roles, vec!["player".to_string()]);
    }

    #[test]
    fn test_expired_jwt_authentication_rejected() {
        let guid = Guid::new(EntityType::Player, 1, 42);
        let auth = EdgeAuthenticator::new_hs256(TEST_SECRET, None, None);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = JwtClaims {
            sub: guid.raw().to_string(),
            iss: None,
            aud: None,
            exp: now - 100, // Expired in the past
            nbf: None,
            iat: Some(now - 200),
            guid: Some(guid.raw()),
            roles: None,
        };

        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET),
        )
        .unwrap();

        let err = auth.authenticate(&token).unwrap_err();
        assert_eq!(err, AuthError::ExpiredToken);
        assert_eq!(err.to_error_code(), auth_error_codes::EXPIRED_TOKEN);
    }

    #[test]
    fn test_tampered_jwt_signature_rejected() {
        let guid = Guid::new(EntityType::Player, 1, 42);
        let auth = EdgeAuthenticator::new_hs256(TEST_SECRET, None, None);

        let mut token = EdgeAuthenticator::create_test_jwt(guid, TEST_SECRET, 3600, None, None);
        // Tamper with the last signature character
        let last_char = if token.ends_with('A') { 'B' } else { 'A' };
        token.pop();
        token.push(last_char);

        let err = auth.authenticate(&token).unwrap_err();
        assert_eq!(err, AuthError::InvalidSignature);
        assert_eq!(err.to_error_code(), auth_error_codes::INVALID_SIGNATURE);
    }

    #[test]
    fn test_wrong_issuer_and_audience_rejected() {
        let guid = Guid::new(EntityType::Player, 1, 42);
        let auth = EdgeAuthenticator::new_hs256(
            TEST_SECRET,
            Some("correct-issuer".to_string()),
            Some("correct-audience".to_string()),
        );

        let token = EdgeAuthenticator::create_test_jwt(
            guid,
            TEST_SECRET,
            3600,
            Some("wrong-issuer"),
            Some("correct-audience"),
        );

        let err = auth.authenticate(&token).unwrap_err();
        assert!(matches!(err, AuthError::InvalidClaims(_)));
    }

    #[test]
    fn test_handover_ticket_authentication() {
        let guid = Guid::new(EntityType::Player, 1, 999);
        let handover_secret = b"handover_cluster_shared_secret_key_987";
        let auth = EdgeAuthenticator::new_hs256(TEST_SECRET, None, None)
            .with_handover_secret(handover_secret.to_vec());

        let ticket = HandoverTicket::create(guid, 555, 2, "gw-us-east", handover_secret, 10000);
        let ticket_bytes = ticket.to_bytes();
        let token = data_encoding::BASE64URL_NOPAD.encode(&ticket_bytes);

        let result = auth
            .authenticate(&token)
            .expect("handover ticket should authenticate");
        assert_eq!(result.guid, guid);
        assert!(result.is_handover);
        assert_eq!(result.session_epoch, 2);
    }

    #[test]
    fn test_algorithm_none_rejected() {
        let auth = EdgeAuthenticator::new_hs256(TEST_SECRET, None, None);
        // Header {"alg": "none", "typ": "JWT"} in base64url is "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0"
        let header = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";
        let claims = JwtClaims {
            sub: "42".to_string(),
            iss: None,
            aud: None,
            exp: 9999999999,
            nbf: None,
            iat: None,
            guid: Some(42),
            roles: None,
        };
        let claims_json = serde_json::to_string(&claims).unwrap();
        let payload = data_encoding::BASE64URL_NOPAD.encode(claims_json.as_bytes());
        let none_token = format!("{}.{}.", header, payload);

        let err = auth.authenticate(&none_token).unwrap_err();
        assert!(
            matches!(
                err,
                AuthError::AlgorithmNotAllowed(_) | AuthError::InvalidToken(_)
            ),
            "Expected algorithm rejection, got {:?}",
            err
        );
    }
}
