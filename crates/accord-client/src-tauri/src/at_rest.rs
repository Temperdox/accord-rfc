//! Encryption at rest for local client state.
//!
//! All the sensitive state the client keeps on disk (the master identity key, the
//! MLS session state, the message-history archive) is encrypted with a local
//! data-encryption key (DEK). The DEK is a random 32-byte key kept in the OS
//! keyring (Windows Credential Manager / macOS Keychain / Linux Secret Service),
//! so it is protected by the OS user account and never written to disk in the
//! clear. We never need the user's password to read these files.
//!
//! Files are sealed with XChaCha20-Poly1305 ([`accord_crypto::backup::seal`]).
//! Note this is separate from the *server vault*, whose blobs are sealed under a
//! key derived from the master key (so they are portable to a new device); the
//! DEK only protects the local copies on this machine.

use std::sync::OnceLock;

use accord_crypto::backup::{open, random_key, seal};
use keyring::Entry;

/// Keyring service + account identifying our local data key.
const SERVICE: &str = "dev.accord.desktop";
const ACCOUNT: &str = "local-data-key";

/// Process-lifetime cache of the DEK so we hit the keyring only once.
static DEK: OnceLock<[u8; 32]> = OnceLock::new();

/// Fetch the local data-encryption key, creating and storing it on first use.
fn dek() -> Result<[u8; 32], String> {
    if let Some(key) = DEK.get() {
        return Ok(*key);
    }
    let entry = Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())?;
    let key = match entry.get_secret() {
        Ok(bytes) => <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| "keyring DEK has wrong length".to_owned())?,
        Err(keyring::Error::NoEntry) => {
            let key = random_key();
            entry.set_secret(&key).map_err(|e| e.to_string())?;
            key
        }
        Err(e) => return Err(e.to_string()),
    };
    let _ = DEK.set(key);
    Ok(key)
}

/// Seal `plaintext` for writing to disk.
///
/// # Errors
/// Returns an error if the keyring is unavailable or encryption fails.
pub fn seal_bytes(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let key = dek()?;
    seal(&key, plaintext).map_err(|e| e.to_string())
}

/// Open a blob produced by [`seal_bytes`]. Returns `None` if it can't be
/// decrypted (wrong/absent key, or the bytes are not in our sealed format - e.g.
/// an older plaintext file, which callers may then handle as a fallback).
#[must_use]
pub fn open_bytes(blob: &[u8]) -> Option<Vec<u8>> {
    let key = dek().ok()?;
    open(&key, blob).ok().map(|pt| pt.to_vec())
}
