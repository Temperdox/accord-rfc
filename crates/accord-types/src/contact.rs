//! Contact codes - the paste-able string you share so someone can add you.
//!
//! A [`ContactCode`] is how cross-user DMs are addressed (FEDERATION-PLAN.md,
//! approach D). You hand it to a friend deliberately; it carries:
//!
//! * `identity_pubkey` - your stable **contact identity** public key (Ed25519,
//!   derived from your master key for the `contact-identity` context, so it does
//!   not expose the master root). Its fingerprint is what you verify out of band.
//! * `name` - an optional display name.
//! * `addresses` - where you're reachable (home server endpoint, mesh address)
//!   for the later DM-routing phases.
//! * `cert` - your home server's pinned TLS cert (for later pinning).
//!
//! Encoded as JSON, base64url, behind an `accordc:` prefix. Unlike an invite key
//! this carries no secret token - the public identity key is meant to be shared
//! with the contact you give it to.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// Prefix identifying an Accord contact code and its format version.
const PREFIX: &str = "accordc:";

/// Everything needed to add and (later) reach a contact, in one opaque string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactCode {
    /// Format version (currently 1).
    pub v: u8,
    /// The contact's 32-byte Ed25519 contact-identity public key.
    pub identity_pubkey: Vec<u8>,
    /// Optional display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Reachable addresses (home endpoint, mesh address) for DM routing later.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// The contact's home server TLS cert (PEM), for pinning later.
    #[serde(default)]
    pub cert: Option<String>,
    /// The contact's user id on their own home server, so a peer can fetch their
    /// KeyPackage and open a DM there (the recipient-hosts-the-DM model).
    #[serde(default)]
    pub host_user_id: Option<String>,
}

/// Errors decoding a contact code.
#[derive(Debug, thiserror::Error)]
pub enum ContactError {
    /// Missing/wrong `accordc:` prefix.
    #[error("not an Accord contact code")]
    BadPrefix,
    /// Base64 payload could not be decoded.
    #[error("contact code is malformed (base64)")]
    BadBase64,
    /// JSON payload could not be parsed.
    #[error("contact code is malformed (payload)")]
    BadPayload,
}

impl ContactCode {
    /// Build a contact code for an identity public key.
    #[must_use]
    pub fn new(identity_pubkey: Vec<u8>) -> Self {
        Self {
            v: 1,
            identity_pubkey,
            name: None,
            addresses: Vec::new(),
            cert: None,
            host_user_id: None,
        }
    }

    /// Attach a display name (builder style).
    #[must_use]
    pub fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    /// Attach reachable addresses (builder style).
    #[must_use]
    pub fn with_addresses(mut self, addresses: Vec<String>) -> Self {
        self.addresses = addresses;
        self
    }

    /// Attach the home server cert (builder style).
    #[must_use]
    pub fn with_cert(mut self, cert: Option<String>) -> Self {
        self.cert = cert;
        self
    }

    /// Attach the contact's home-server user id (builder style).
    #[must_use]
    pub fn with_host_user_id(mut self, host_user_id: Option<String>) -> Self {
        self.host_user_id = host_user_id;
        self
    }

    /// Encode to the opaque `accordc:<base64url>` string.
    #[must_use]
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("ContactCode always serializes");
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    /// Decode from an `accordc:` string.
    ///
    /// # Errors
    /// Returns [`ContactError`] if the prefix, base64, or payload is invalid.
    pub fn decode(s: &str) -> Result<Self, ContactError> {
        let body = s
            .trim()
            .strip_prefix(PREFIX)
            .ok_or(ContactError::BadPrefix)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| ContactError::BadBase64)?;
        serde_json::from_slice(&bytes).map_err(|_| ContactError::BadPayload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let code = ContactCode::new(vec![7u8; 32])
            .with_name(Some("Bob".to_owned()))
            .with_addresses(vec!["https://host:50051".to_owned()]);
        let encoded = code.encode();
        assert!(encoded.starts_with("accordc:"));
        assert_eq!(ContactCode::decode(&encoded).unwrap(), code);
    }

    #[test]
    fn rejects_bad_prefix() {
        assert!(matches!(
            ContactCode::decode("nope:xxx"),
            Err(ContactError::BadPrefix)
        ));
    }
}
