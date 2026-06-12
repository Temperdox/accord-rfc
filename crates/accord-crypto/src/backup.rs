//! Password-encrypted key backup.
//!
//! Implements the wrapping scheme from ARCHITECTURE.md section 6: derive a 256-bit
//! wrapping key from the user's password with **Argon2id**, then encrypt the
//! private key material with **XChaCha20-Poly1305**. Only the resulting
//! ciphertext blob + the (public) KDF parameters and salt are uploaded to the
//! server, which can never decrypt them.
//!
//! Blob layout produced by [`encrypt_backup`]:
//! ```text
//! [ 24-byte XChaCha20 nonce | ciphertext+16-byte Poly1305 tag ]
//! ```
//! The nonce is stored inline because XChaCha20's 192-bit nonce makes random
//! generation collision-safe (ARCHITECTURE section 6.4).

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::CryptoError;

/// Length of the XChaCha20-Poly1305 nonce.
pub const NONCE_LEN: usize = 24;
/// Length of the Argon2id-derived wrapping key.
pub const WRAPPING_KEY_LEN: usize = 32;
/// Length of the per-user salt.
pub const SALT_LEN: usize = 32;

/// Argon2id cost parameters. Stored alongside the blob (they are not secret) so
/// any device can reproduce the wrapping key from the password.
///
/// Defaults follow ARCHITECTURE.md section 6.3: 256 MiB memory, 4 iterations,
/// parallelism 4 - tuned for ~500 ms derivation on modern hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Argon2Params {
    /// Memory cost in KiB.
    pub memory_kib: u32,
    /// Number of passes.
    pub iterations: u32,
    /// Degree of parallelism (lanes).
    pub parallelism: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            memory_kib: 256 * 1024,
            iterations: 4,
            parallelism: 4,
        }
    }
}

impl Argon2Params {
    /// Serialize to 12 bytes (three little-endian u32s) for the `argon2_params`
    /// wire field.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut out = [0u8; 12];
        out[0..4].copy_from_slice(&self.memory_kib.to_le_bytes());
        out[4..8].copy_from_slice(&self.iterations.to_le_bytes());
        out[8..12].copy_from_slice(&self.parallelism.to_le_bytes());
        out
    }

    /// Parse from the 12-byte wire form.
    ///
    /// # Errors
    /// Returns [`CryptoError::KeyDerivation`] if `bytes` is not exactly 12 long.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != 12 {
            return Err(CryptoError::KeyDerivation(format!(
                "argon2_params must be 12 bytes, got {}",
                bytes.len()
            )));
        }
        let read =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        Ok(Self {
            memory_kib: read(0),
            iterations: read(4),
            parallelism: read(8),
        })
    }
}

/// The product of [`encrypt_backup`]: everything needed to later decrypt, with
/// secrets already removed (the blob is ciphertext).
#[derive(Debug, Clone)]
pub struct EncryptedBackup {
    /// `nonce || ciphertext || tag`.
    pub blob: Vec<u8>,
    /// Per-user random salt used for Argon2id.
    pub salt: [u8; SALT_LEN],
    /// KDF parameters used.
    pub params: Argon2Params,
}

/// Generate a fresh random salt from the OS CSPRNG.
#[must_use]
pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    salt
}

/// Generate a random 32-byte key from the OS CSPRNG (e.g. a local
/// data-encryption key kept in the OS keyring for at-rest encryption).
#[must_use]
pub fn random_key() -> [u8; WRAPPING_KEY_LEN] {
    let mut key = [0u8; WRAPPING_KEY_LEN];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

/// Derive the 256-bit wrapping key from a password using Argon2id.
///
/// The returned key is wrapped in [`Zeroizing`] so it is scrubbed on drop.
///
/// # Errors
/// Returns [`CryptoError::KeyDerivation`] for invalid parameters or KDF failure.
pub fn derive_wrapping_key(
    password: &[u8],
    salt: &[u8],
    params: Argon2Params,
) -> Result<Zeroizing<[u8; WRAPPING_KEY_LEN]>, CryptoError> {
    use argon2::{Algorithm, Argon2, Params, Version};

    let kdf_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(WRAPPING_KEY_LEN),
    )
    .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;

    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, kdf_params);

    let mut key = Zeroizing::new([0u8; WRAPPING_KEY_LEN]);
    argon
        .hash_password_into(password, salt, key.as_mut_slice())
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// Encrypt `plaintext` (the user's serialized private key material) under a key
/// derived from `password`, generating a fresh salt and nonce.
///
/// # Errors
/// Returns [`CryptoError`] if key derivation or AEAD encryption fails.
pub fn encrypt_backup(
    password: &[u8],
    plaintext: &[u8],
    params: Argon2Params,
) -> Result<EncryptedBackup, CryptoError> {
    let salt = generate_salt();
    let key = derive_wrapping_key(password, &salt, params)?;

    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_slice()).map_err(|_| CryptoError::Decryption)?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Decryption)?;

    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);

    Ok(EncryptedBackup { blob, salt, params })
}

/// Decrypt a backup blob produced by [`encrypt_backup`].
///
/// The returned plaintext is wrapped in [`Zeroizing`].
///
/// # Errors
/// Returns [`CryptoError::Decryption`] on a wrong password or a tampered blob
/// (XChaCha20-Poly1305 is authenticated, so this is a hard, detectable failure
/// rather than garbage output).
pub fn decrypt_backup(
    password: &[u8],
    blob: &[u8],
    salt: &[u8],
    params: Argon2Params,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if blob.len() < NONCE_LEN {
        return Err(CryptoError::Decryption);
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);

    let key = derive_wrapping_key(password, salt, params)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_slice()).map_err(|_| CryptoError::Decryption)?;
    let nonce = XNonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decryption)?;
    Ok(Zeroizing::new(plaintext))
}

/// Seal `plaintext` under a raw 32-byte key with XChaCha20-Poly1305, returning
/// `nonce || ciphertext+tag`.
///
/// Unlike [`encrypt_backup`], this takes a key directly (no password KDF), so it
/// is cheap to call often. I use it for state encrypted under a key *derived from
/// the master key* (see `IdentityKeyPair::derive_symmetric`) - e.g. the MLS-state
/// and message-history blobs synced to the server vault.
///
/// # Errors
/// Returns [`CryptoError::Decryption`] if AEAD encryption fails.
pub fn seal(key: &[u8; WRAPPING_KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::Decryption)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Decryption)?;
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Open a blob produced by [`seal`] under the same 32-byte key.
///
/// # Errors
/// Returns [`CryptoError::Decryption`] on a wrong key or tampered blob.
pub fn open(key: &[u8; WRAPPING_KEY_LEN], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if blob.len() < NONCE_LEN {
        return Err(CryptoError::Decryption);
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::Decryption)?;
    let nonce = XNonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decryption)?;
    Ok(Zeroizing::new(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Use cheap KDF parameters in tests so they run fast (the real defaults
    // take ~500 ms per derivation, which would make the suite crawl).
    fn fast_params() -> Argon2Params {
        Argon2Params {
            memory_kib: 8,
            iterations: 1,
            parallelism: 1,
        }
    }

    #[test]
    fn encrypt_then_decrypt_roundtrips() {
        let secret = b"my-private-mls-keys";
        let backup = encrypt_backup(b"correct horse", secret, fast_params()).unwrap();
        let recovered =
            decrypt_backup(b"correct horse", &backup.blob, &backup.salt, backup.params).unwrap();
        assert_eq!(recovered.as_slice(), secret);
    }

    #[test]
    fn wrong_password_fails() {
        let backup = encrypt_backup(b"right", b"secret", fast_params()).unwrap();
        let result = decrypt_backup(b"wrong", &backup.blob, &backup.salt, backup.params);
        assert!(matches!(result, Err(CryptoError::Decryption)));
    }

    #[test]
    fn tampered_blob_fails() {
        let mut backup = encrypt_backup(b"pw", b"secret", fast_params()).unwrap();
        *backup.blob.last_mut().unwrap() ^= 0xFF; // flip a tag bit
        let result = decrypt_backup(b"pw", &backup.blob, &backup.salt, backup.params);
        assert!(result.is_err());
    }

    #[test]
    fn params_byte_roundtrip() {
        let p = Argon2Params::default();
        assert_eq!(Argon2Params::from_bytes(&p.to_bytes()).unwrap(), p);
    }
}
