//! JWT access tokens (HS256).
//!
//! Access tokens are short-lived bearer tokens (ARCHITECTURE.md section 7, section 12) that
//! authorize unary RPCs and the message stream. The claims bind a token to both
//! a user and the specific device it was issued to, so per-device logic (each
//! device is its own MLS leaf) can rely on `device_id`.

use accord_types::{DeviceId, UserId};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::error::{ServerError, ServerResult};

/// Claims embedded in an Accord access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject - the user id (string form).
    pub sub: String,
    /// The device this token was issued to.
    pub device_id: String,
    /// Expiry (unix seconds).
    pub exp: u64,
    /// Issued-at (unix seconds).
    pub iat: u64,
}

/// Mints and verifies access tokens with a shared HMAC secret.
#[derive(Clone)]
pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    ttl_secs: u64,
}

impl std::fmt::Debug for JwtKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key material.
        f.debug_struct("JwtKeys")
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl JwtKeys {
    /// Build a key pair from the configured HMAC secret.
    #[must_use]
    pub fn new(secret: &str, ttl_secs: u64) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
            ttl_secs,
        }
    }

    /// Issue a fresh access token for `(user, device)`.
    ///
    /// # Errors
    /// Returns [`ServerError::Token`] if signing fails.
    pub fn issue(&self, user: UserId, device: DeviceId) -> ServerResult<String> {
        let now = now_secs();
        let claims = Claims {
            sub: user.to_string(),
            device_id: device.to_string(),
            iat: now,
            exp: now + self.ttl_secs,
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(|e| ServerError::Token(e.to_string()))
    }

    /// Verify a token and return its claims.
    ///
    /// # Errors
    /// Returns [`ServerError::Unauthenticated`] for any invalid/expired token.
    pub fn verify(&self, token: &str) -> ServerResult<Claims> {
        let validation = Validation::new(Algorithm::HS256);
        decode::<Claims>(token, &self.decoding, &validation)
            .map(|data| data.claims)
            .map_err(|_| ServerError::Unauthenticated)
    }
}

/// Current unix time in seconds.
fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
