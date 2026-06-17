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

/// How long a login challenge is valid (short - it's a single round-trip).
const CHALLENGE_TTL_SECS: u64 = 120;

/// Claims for a key-login challenge: binds the challenge to a specific identity
/// key and a short expiry. Signed with the same HMAC secret as access tokens, so
/// a client cannot forge one - it must come from [`JwtKeys::issue_challenge`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChallengeClaims {
    /// The identity public key (hex) this challenge is for.
    sub: String,
    /// Distinguishes a challenge from an access token (defense in depth).
    purpose: String,
    /// A per-challenge nonce so two challenges for the same key differ.
    nonce: String,
    exp: u64,
    iat: u64,
}

impl JwtKeys {
    /// Issue a short-lived challenge token bound to `identity_pubkey_hex`. The
    /// client signs the returned string with its identity key.
    ///
    /// # Errors
    /// Returns [`ServerError::Token`] if signing fails.
    pub fn issue_challenge(&self, identity_pubkey_hex: &str, nonce: &str) -> ServerResult<String> {
        let now = now_secs();
        let claims = ChallengeClaims {
            sub: identity_pubkey_hex.to_owned(),
            purpose: "challenge".to_owned(),
            nonce: nonce.to_owned(),
            iat: now,
            exp: now + CHALLENGE_TTL_SECS,
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(|e| ServerError::Token(e.to_string()))
    }

    /// Verify a challenge token (signature + expiry + purpose) and return the
    /// identity-pubkey hex it was issued for.
    ///
    /// # Errors
    /// Returns [`ServerError::Unauthenticated`] for any invalid/expired/wrong
    /// token.
    pub fn verify_challenge(&self, token: &str) -> ServerResult<String> {
        let validation = Validation::new(Algorithm::HS256);
        let claims = decode::<ChallengeClaims>(token, &self.decoding, &validation)
            .map(|data| data.claims)
            .map_err(|_| ServerError::Unauthenticated)?;
        if claims.purpose != "challenge" {
            return Err(ServerError::Unauthenticated);
        }
        Ok(claims.sub)
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
