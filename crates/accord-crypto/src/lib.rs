//! # accord-crypto
//!
//! Client-side cryptographic primitives for Accord. **This crate is never used
//! by the server** - keeping it out of `accord-server`'s dependency tree is how
//! we structurally guarantee the server cannot touch plaintext key material
//! (ARCHITECTURE.md section 5, section 16).
//!
//! Modules:
//! * [`identity`] - Ed25519 identity keypairs (sign MLS messages, prove identity).
//! * [`backup`] - password-derived key wrapping (Argon2id + XChaCha20-Poly1305)
//! for the encrypted key-backup blob uploaded to the server (ARCHITECTURE section 6).
//! * [`hashing`] - BLAKE3 identity-anchor hashing (e.g. `BLAKE3(email + salt)`).
//!
//! ## Secret handling
//! Private key material is wrapped in [`zeroize`]-aware types so it is scrubbed
//! from memory on drop. Never log, serialize, or `Debug`-print raw secrets.

pub mod backup;
pub mod hashing;
pub mod identity;

/// Errors surfaced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// A key or signature had the wrong length / encoding.
    #[error("invalid key material: {0}")]
    InvalidKey(String),

    /// An Ed25519 signature failed verification.
    #[error("signature verification failed")]
    BadSignature,

    /// Argon2 key derivation failed (bad parameters).
    #[error("key derivation failed: {0}")]
    KeyDerivation(String),

    /// AEAD encryption/decryption failed (wrong password, or tampered blob).
    #[error("decryption failed: wrong password or corrupted backup")]
    Decryption,
}
