//! Invite keys - the single paste-able string that lets someone join a server.
//!
//! An [`InviteKey`] bundles **everything** a client needs to connect, so a user
//! never has to configure addresses, mesh peers, or anything else by hand:
//!
//! * `transport` - how to reach the host ([`Transport::Direct`] IP/host, or
//! [`Transport::Mesh`] Yggdrasil overlay).
//! * `host` / `port` - the server's address.
//! * `peers` - mesh bootstrap peers (so the joiner auto-joins the same overlay).
//! * `token` - the secret invite token (empty for open/public servers).
//! * `name` - optional display name.
//!
//! It is serialized to JSON and base64url-encoded behind an `accord1:` prefix,
//! producing an opaque blob ("a string of random-looking characters") that only
//! the app knows how to read. The token inside is the real secret; the envelope
//! is encoding, not encryption - treat the whole key as sensitive.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// Prefix identifying an Accord invite key and its format version.
const PREFIX: &str = "accord1:";

/// How to reach the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A directly-reachable IP/hostname (LAN, or a public IP).
    Direct,
    /// A Yggdrasil mesh address; `peers` bootstrap the joiner onto the overlay.
    Mesh,
}

/// Everything needed to join a server, encoded into one opaque string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteKey {
    /// Format version (currently 1).
    pub v: u8,
    /// Transport to reach the host.
    pub transport: Transport,
    /// Host address (IP / hostname / mesh IPv6).
    pub host: String,
    /// Host port.
    pub port: u16,
    /// Secret invite token; empty string for open/public servers.
    pub token: String,
    /// Mesh bootstrap peers (empty for direct transport).
    #[serde(default)]
    pub peers: Vec<String>,
    /// Optional human-readable server name.
    #[serde(default)]
    pub name: Option<String>,
    /// The server's self-signed TLS certificate (PEM). When present, the client
    /// connects over TLS and pins exactly this cert (authenticated, no CA).
    #[serde(default)]
    pub cert: Option<String>,
}

/// Errors decoding an invite key.
#[derive(Debug, thiserror::Error)]
pub enum InviteError {
    /// Missing/!wrong `accord1:` prefix.
    #[error("not an Accord invite key")]
    BadPrefix,
    /// Base64 payload could not be decoded.
    #[error("invite key is malformed (base64)")]
    BadBase64,
    /// JSON payload could not be parsed.
    #[error("invite key is malformed (payload)")]
    BadPayload,
}

impl InviteKey {
    /// Build a direct (IP/host) invite key.
    #[must_use]
    pub fn direct(host: impl Into<String>, port: u16, token: impl Into<String>) -> Self {
        Self {
            v: 1,
            transport: Transport::Direct,
            host: host.into(),
            port,
            token: token.into(),
            peers: Vec::new(),
            name: None,
            cert: None,
        }
    }

    /// Build a mesh invite key (carrying bootstrap peers).
    #[must_use]
    pub fn mesh(
        host: impl Into<String>,
        port: u16,
        token: impl Into<String>,
        peers: Vec<String>,
    ) -> Self {
        Self {
            v: 1,
            transport: Transport::Mesh,
            host: host.into(),
            port,
            token: token.into(),
            peers,
            name: None,
            cert: None,
        }
    }

    /// Attach a display name (builder style).
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach the server's TLS cert (PEM) for pinning (builder style).
    #[must_use]
    pub fn with_cert(mut self, cert: Option<String>) -> Self {
        self.cert = cert;
        self
    }

    /// Encode to the opaque `accord1:<base64url>` string.
    #[must_use]
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("InviteKey always serializes");
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    /// Decode from an `accord1:` string.
    ///
    /// # Errors
    /// Returns [`InviteError`] if the prefix, base64, or payload is invalid.
    pub fn decode(s: &str) -> Result<Self, InviteError> {
        let body = s
            .trim()
            .strip_prefix(PREFIX)
            .ok_or(InviteError::BadPrefix)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| InviteError::BadBase64)?;
        serde_json::from_slice(&bytes).map_err(|_| InviteError::BadPayload)
    }

    /// The gRPC endpoint URL to connect to (brackets IPv6 mesh addresses; uses
    /// `https` when a pinned cert is present, else `http`).
    #[must_use]
    pub fn endpoint(&self) -> String {
        let scheme = if self.cert.is_some() { "https" } else { "http" };
        if self.host.contains(':') {
            // IPv6 literal (e.g. a mesh address) must be bracketed in a URL.
            format!("{scheme}://[{}]:{}", self.host, self.port)
        } else {
            format!("{scheme}://{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_roundtrip() {
        let key = InviteKey::direct("192.168.1.10", 50051, "secret-token").with_name("My Server");
        let encoded = key.encode();
        assert!(encoded.starts_with("accord1:"));
        // Opaque: the raw token/host shouldn't be plainly readable in the blob.
        assert!(!encoded.contains("192.168"));
        let decoded = InviteKey::decode(&encoded).unwrap();
        assert_eq!(decoded, key);
        assert_eq!(decoded.endpoint(), "http://192.168.1.10:50051");
    }

    #[test]
    fn mesh_roundtrip_brackets_ipv6() {
        let key = InviteKey::mesh(
            "200:2b6b:2738:3ac7:5bea:8215:5f09:d9a2",
            50051,
            "tok",
            vec!["tls://peer.example:443".into()],
        );
        let decoded = InviteKey::decode(&key.encode()).unwrap();
        assert_eq!(decoded, key);
        assert_eq!(
            decoded.endpoint(),
            "http://[200:2b6b:2738:3ac7:5bea:8215:5f09:d9a2]:50051"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            InviteKey::decode("nope"),
            Err(InviteError::BadPrefix)
        ));
        assert!(matches!(
            InviteKey::decode("accord1:!!!"),
            Err(InviteError::BadBase64)
        ));
    }
}
