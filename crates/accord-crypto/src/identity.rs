//! Ed25519 identity keys.
//!
//! Every user has a long-lived **identity keypair**. The private half signs MLS
//! messages and proves the user controls their account; the public half is what
//! peers verify against (ARCHITECTURE.md section 6.5). The private key is part of what
//! the encrypted key backup protects.
//!
//! We use Ed25519 (the signature scheme in Accord's chosen MLS ciphersuite -
//! `..._Ed25519`) for fast signing/verification of Commits and KeyPackages.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::CryptoError;

/// Length of a raw Ed25519 private/public key in bytes.
pub const KEY_LEN: usize = 32;
/// Length of a raw Ed25519 signature in bytes.
pub const SIGNATURE_LEN: usize = 64;

/// A user's identity keypair (private + public).
///
/// The inner [`SigningKey`] zeroizes its secret on drop. We deliberately do not
/// derive `Debug`/`Serialize` to avoid accidentally leaking the secret; use
/// [`secret_bytes`](IdentityKeyPair::secret_bytes) explicitly when you must
/// persist it (always via the encrypted backup path).
pub struct IdentityKeyPair {
    signing: SigningKey,
}

impl IdentityKeyPair {
    /// Generate a fresh identity keypair from the OS CSPRNG.
    #[must_use]
    pub fn generate() -> Self {
        // rand 0.8's OsRng implements the rand_core 0.6 traits ed25519-dalek 2
        // expects, so the two crates interoperate without a shim.
        Self {
            signing: SigningKey::generate(&mut rand::rngs::OsRng),
        }
    }

    /// Reconstruct a keypair from previously-exported secret bytes (e.g. after
    /// decrypting a key backup).
    #[must_use]
    pub fn from_secret_bytes(bytes: &[u8; KEY_LEN]) -> Self {
        Self {
            signing: SigningKey::from_bytes(bytes),
        }
    }

    /// Export the 32-byte secret key. **Only ever persist this encrypted.**
    #[must_use]
    pub fn secret_bytes(&self) -> [u8; KEY_LEN] {
        self.signing.to_bytes()
    }

    /// The public half, safe to share.
    #[must_use]
    pub fn public(&self) -> IdentityPublicKey {
        IdentityPublicKey(self.signing.verifying_key())
    }

    /// Sign a message, returning the raw 64-byte signature.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.signing.sign(message).to_bytes()
    }

    /// Derive a deterministic, per-context child identity keypair from this
    /// master key.
    ///
    /// I use this for key-based identity. The master key is the user's real
    /// identity and stays secret on their device; it is never sent anywhere and
    /// never shown. For each server I derive a child keypair with a per-server
    /// context, and the server only ever sees the child's public key. The
    /// properties I rely on:
    ///
    /// - Deterministic: the same (master, context) always produces the same
    ///   child, so the user keeps a stable identity on a given server across
    ///   reinstalls (as long as they restore their master key).
    /// - Unlinkable: different contexts produce unrelated children, so two
    ///   servers cannot correlate that the same person is on both by comparing
    ///   the public keys they were given.
    /// - One-way: BLAKE3 acts as a PRF keyed by the master secret, so a child
    ///   key (public or private) does not reveal the master.
    #[must_use]
    pub fn derive_for_context(&self, context: &[u8]) -> IdentityKeyPair {
        let seed = blake3::keyed_hash(&self.secret_bytes(), context);
        IdentityKeyPair::from_secret_bytes(seed.as_bytes())
    }

    /// Derive a 32-byte symmetric key from the master key for `context`.
    ///
    /// I use this to encrypt state I back up to the server (MLS session state,
    /// message history) under a key only this user can reproduce. A `sym:` domain
    /// prefix keeps these keys separate from the signing seeds produced by
    /// [`derive_for_context`], so the same context never yields the same bytes for
    /// two different purposes.
    #[must_use]
    pub fn derive_symmetric(&self, context: &[u8]) -> [u8; KEY_LEN] {
        let mut input = Vec::with_capacity(4 + context.len());
        input.extend_from_slice(b"sym:");
        input.extend_from_slice(context);
        *blake3::keyed_hash(&self.secret_bytes(), &input).as_bytes()
    }
}

impl std::fmt::Debug for IdentityKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret. Show only the (public) verifying key.
        f.debug_struct("IdentityKeyPair")
            .field("public", &self.public())
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The public half of an [`IdentityKeyPair`]; verifies signatures.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IdentityPublicKey(VerifyingKey);

impl IdentityPublicKey {
    /// Parse a public key from its raw 32-byte form.
    ///
    /// # Errors
    /// Returns [`CryptoError::InvalidKey`] if the bytes are not a valid point.
    pub fn from_bytes(bytes: &[u8; KEY_LEN]) -> Result<Self, CryptoError> {
        VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))
    }

    /// Export to raw 32-byte form.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; KEY_LEN] {
        self.0.to_bytes()
    }

    /// A short, human-comparable fingerprint of this public key.
    ///
    /// I only use this for explicit, consensual verification (a safety-number
    /// style screen where two people compare fingerprints out of band). The raw
    /// key is not surfaced casually, because a stable identifier shown freely is
    /// exactly what would let someone track a user, and that is the thing I want
    /// to avoid.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let hash = blake3::hash(&self.to_bytes());
        hex(&hash.as_bytes()[..8])
    }

    /// Verify `signature` over `message`.
    ///
    /// Uses `verify_strict` to reject malleable / non-canonical signatures.
    ///
    /// # Errors
    /// Returns [`CryptoError::BadSignature`] if verification fails.
    pub fn verify(
        &self,
        message: &[u8],
        signature: &[u8; SIGNATURE_LEN],
    ) -> Result<(), CryptoError> {
        let sig = Signature::from_bytes(signature);
        self.0
            .verify_strict(message, &sig)
            .map_err(|_| CryptoError::BadSignature)
    }
}

impl std::fmt::Debug for IdentityPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Public key - safe to show as hex.
        write!(f, "IdentityPublicKey({})", hex(&self.to_bytes()))
    }
}

/// Minimal lowercase-hex encoder (avoids pulling in a hex crate for one use).
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let keypair = IdentityKeyPair::generate();
        let public = keypair.public();
        let msg = b"accord handshake";

        let sig = keypair.sign(msg);
        assert!(public.verify(msg, &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_message() {
        let keypair = IdentityKeyPair::generate();
        let public = keypair.public();
        let sig = keypair.sign(b"original");
        assert!(public.verify(b"tampered", &sig).is_err());
    }

    #[test]
    fn secret_bytes_roundtrip() {
        let keypair = IdentityKeyPair::generate();
        let restored = IdentityKeyPair::from_secret_bytes(&keypair.secret_bytes());
        // Same secret -> same public key.
        assert_eq!(keypair.public(), restored.public());
    }

    #[test]
    fn public_key_bytes_roundtrip() {
        let public = IdentityKeyPair::generate().public();
        let restored = IdentityPublicKey::from_bytes(&public.to_bytes()).unwrap();
        assert_eq!(public, restored);
    }

    #[test]
    fn derivation_is_deterministic() {
        let master = IdentityKeyPair::generate();
        let a = master.derive_for_context(b"server:abc");
        let b = master.derive_for_context(b"server:abc");
        assert_eq!(a.public(), b.public());
        // And the derived key can actually sign/verify.
        let sig = a.sign(b"hi");
        assert!(b.public().verify(b"hi", &sig).is_ok());
    }

    #[test]
    fn different_contexts_are_unlinkable() {
        let master = IdentityKeyPair::generate();
        let a = master.derive_for_context(b"server:abc");
        let b = master.derive_for_context(b"server:xyz");
        assert_ne!(a.public(), b.public());
    }

    #[test]
    fn derived_key_does_not_equal_master() {
        let master = IdentityKeyPair::generate();
        let child = master.derive_for_context(b"server:abc");
        assert_ne!(master.public(), child.public());
        assert_ne!(master.secret_bytes(), child.secret_bytes());
    }

    #[test]
    fn symmetric_key_derivation() {
        let master = IdentityKeyPair::generate();
        // Deterministic per context.
        assert_eq!(
            master.derive_symmetric(b"vault:mls"),
            master.derive_symmetric(b"vault:mls")
        );
        // Different contexts -> different keys.
        assert_ne!(
            master.derive_symmetric(b"vault:mls"),
            master.derive_symmetric(b"vault:hist")
        );
        // Key-separated from the signing seed for the same context.
        assert_ne!(
            master.derive_symmetric(b"x"),
            master.derive_for_context(b"x").secret_bytes()
        );
    }

    #[test]
    fn seal_open_roundtrip() {
        use crate::backup::{open, seal};
        let key = IdentityKeyPair::generate().derive_symmetric(b"vault:mls");
        let blob = seal(&key, b"mls state bytes").unwrap();
        assert_eq!(open(&key, &blob).unwrap().as_slice(), b"mls state bytes");
        // Wrong key fails.
        let other = IdentityKeyPair::generate().derive_symmetric(b"vault:mls");
        assert!(open(&other, &blob).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_distinct() {
        let master = IdentityKeyPair::generate();
        let a = master.derive_for_context(b"server:abc");
        let b = master.derive_for_context(b"server:xyz");
        assert_eq!(a.public().fingerprint(), a.public().fingerprint());
        assert_ne!(a.public().fingerprint(), b.public().fingerprint());
        assert_eq!(a.public().fingerprint().len(), 16); // 8 bytes as hex
    }
}
