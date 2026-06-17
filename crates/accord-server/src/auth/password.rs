//! Server-side password hashing with Argon2id.
//!
//! This is the *login* password hash (ARCHITECTURE.md section 8.1) - distinct from the
//! client-side Argon2id used to wrap key backups (section 6). Hashes are stored in PHC
//! string format (`$argon2id$v=19$m=...,t=...,p=...$salt$hash`), which embeds the
//! parameters and salt, so verification needs only the stored string.
//!
//! Using Argon2 here (rather than in `accord-crypto`) does not violate the
//! "server has no crypto" rule: this is ordinary auth hashing, never key
//! material, and never touches MLS.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::error::{ServerError, ServerResult};

/// Hash a plaintext password for storage.
///
/// # Errors
/// Returns [`ServerError::PasswordHash`] if hashing fails.
pub fn hash_password(password: &str) -> ServerResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| ServerError::PasswordHash(e.to_string()))
}

/// Verify a plaintext password against a stored PHC hash string.
///
/// Returns `Ok(true)` on match, `Ok(false)` on mismatch. A malformed stored
/// hash is an internal error.
///
/// # Errors
/// Returns [`ServerError::PasswordHash`] if the stored hash cannot be parsed.
pub fn verify_password(password: &str, stored_hash: &str) -> ServerResult<bool> {
    // Key-only accounts (taverns) store no password hash; password login must
    // simply fail for them, not error on an unparseable empty hash.
    if stored_hash.is_empty() {
        return Ok(false);
    }
    let parsed =
        PasswordHash::new(stored_hash).map_err(|e| ServerError::PasswordHash(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let hash = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn empty_hash_never_verifies() {
        // Key-only accounts store no password hash; password login must fail.
        assert!(!verify_password("anything", "").unwrap());
    }
}
