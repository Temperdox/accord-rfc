//! TLS for the **no-PKI** world.
//!
//! Self-hosted Accord servers have no domain and no CA-signed certificate, so we
//! use a **self-signed** cert and **pin it via the invite key**: the host's cert
//! travels inside the invite, and the joining client trusts exactly that cert
//! (and nothing else). This gives an authenticated, encrypted channel with zero
//! certificate authorities.
//!
//! The cert's SAN is a fixed name ([`PINNED_DOMAIN`]); clients set their
//! verification domain to it and pin the cert, so it works regardless of the
//! actual IP / mesh address the server is reachable at.

use rcgen::{CertifiedKey, generate_simple_self_signed};

/// The fixed SAN baked into self-signed certs; clients verify against this name
/// (the real host/IP can be anything since the exact cert is pinned).
pub const PINNED_DOMAIN: &str = "accord.local";

/// Generate a fresh self-signed certificate, returning `(cert_pem, key_pem)`.
///
/// # Errors
/// Returns an error string if certificate generation fails.
pub fn generate_self_signed() -> Result<(String, String), String> {
    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec![PINNED_DOMAIN.to_owned()])
            .map_err(|e| format!("cert generation failed: {e}"))?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}
