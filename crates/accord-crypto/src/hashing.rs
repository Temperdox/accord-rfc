//! BLAKE3 hashing for identity anchors.
//!
//! Per the "Pseudonymous Usernames + Tiered Identity Anchoring" design, the
//! server stores only a hash of a user's identity anchor (email/phone), never
//! the plaintext: `BLAKE3(identifier + server_salt)` (data export, Identity &
//! Auth). This gives sybil resistance and a recovery handle without the server
//! ever learning the anchor itself.
//!
//! BLAKE3 is used for its speed and strong security margin. The server-side
//! salt is a deployment secret that thwarts precomputation across the whole
//! user base.

/// Length of a BLAKE3 digest in bytes.
pub const DIGEST_LEN: usize = 32;

/// Hash an identity anchor (e.g. a normalized email address) together with a
/// server-wide salt.
///
/// Callers should normalize `identifier` (trim, lowercase) before hashing so the
/// same email always maps to the same digest.
#[must_use]
pub fn identity_anchor_hash(identifier: &str, server_salt: &[u8]) -> [u8; DIGEST_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(identifier.as_bytes());
    hasher.update(server_salt);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_input() {
        let a = identity_anchor_hash("user@example.com", b"server-salt");
        let b = identity_anchor_hash("user@example.com", b"server-salt");
        assert_eq!(a, b);
    }

    #[test]
    fn salt_changes_digest() {
        let a = identity_anchor_hash("user@example.com", b"salt-1");
        let b = identity_anchor_hash("user@example.com", b"salt-2");
        assert_ne!(a, b);
    }
}
