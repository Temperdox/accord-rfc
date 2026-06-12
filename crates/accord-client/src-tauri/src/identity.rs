//! The device's master identity key.
//!
//! Key-based identity in Accord starts here. I keep one long-lived master
//! Ed25519 key on the device. It is the user's real identity, it never leaves the
//! machine, and I never show it. For each server I derive a separate per-server
//! key from it (see `accord_crypto::identity::IdentityKeyPair::derive_for_context`)
//! and the server only ever sees that derived public key. That gives a stable
//! identity per server while keeping the user unlinkable across servers.

use accord_crypto::identity::IdentityKeyPair;
use tauri::{AppHandle, Manager};

/// Path to the locally-cached master key (raw secret bytes).
///
/// This is a local cache. The durable copy is the password-encrypted backup the
/// client uploads to the server on first login (see `commands::auth::login`), so
/// the identity survives a reinstall and can move to a new device.
fn key_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("identity.key"))
}

/// Load the locally-cached master key, if present and valid. The file is
/// encrypted at rest; an older plaintext file is read once and re-saved encrypted.
#[must_use]
pub fn load_master(app: &AppHandle) -> Option<IdentityKeyPair> {
    let path = key_path(app).ok()?;
    let bytes = std::fs::read(path).ok()?;

    // Current format: encrypted at rest.
    if let Some(plaintext) = crate::at_rest::open_bytes(&bytes) {
        if let Ok(secret) = <[u8; 32]>::try_from(plaintext.as_slice()) {
            return Some(IdentityKeyPair::from_secret_bytes(&secret));
        }
    }
    // Migration fallback: an older unencrypted 32-byte key. Re-save it encrypted.
    if let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice()) {
        let keypair = IdentityKeyPair::from_secret_bytes(&secret);
        let _ = save_master(app, &keypair);
        return Some(keypair);
    }
    None
}

/// Persist the master key locally, encrypted at rest.
pub fn save_master(app: &AppHandle, keypair: &IdentityKeyPair) -> Result<(), String> {
    let path = key_path(app)?;
    let blob = crate::at_rest::seal_bytes(&keypair.secret_bytes())?;
    std::fs::write(&path, blob).map_err(|e| format!("could not persist identity key: {e}"))
}

/// Load the master key, or create and persist a fresh one.
pub fn load_or_create_master(app: &AppHandle) -> Result<IdentityKeyPair, String> {
    if let Some(keypair) = load_master(app) {
        return Ok(keypair);
    }
    let keypair = IdentityKeyPair::generate();
    save_master(app, &keypair)?;
    Ok(keypair)
}

/// Domain for the stable **contact identity** - the key in your fr code that
/// peers recognize you by. Used as the MLS credential for contact DMs so the
/// recipient can attribute the DM to you (not to a per-host derived key).
pub const CONTACT_IDENTITY_CONTEXT: &[u8] = b"accord:contact-identity";

/// A stable, machine-unique device name like `Desktop-3fa2bc01`.
///
/// Servers key the offline mailbox and Welcome inbox by device id and reuse the
/// device row for a repeated name, so the name must be (a) stable across
/// restarts on this install and (b) different on another machine even after the
/// identity is restored from backup. A persisted random install id (not anything
/// derived from the master key) gives exactly that.
pub fn device_name(app: &AppHandle, label: &str) -> String {
    let install_id = load_or_create_install_id(app).unwrap_or_else(|_| "00000000".to_owned());
    format!("{label}-{install_id}")
}

/// Load (or mint + persist) this install's random 8-hex-char id.
fn load_or_create_install_id(app: &AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("install-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_owned());
        }
    }
    // 4 random bytes from the OS CSPRNG (via the keypair generator we link).
    let id: String = IdentityKeyPair::generate().secret_bytes()[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(&path, &id).map_err(|e| e.to_string())?;
    Ok(id)
}

/// The stable contact-identity keypair derived from the master key.
#[must_use]
pub fn contact_identity(master: &IdentityKeyPair) -> IdentityKeyPair {
    master.derive_for_context(CONTACT_IDENTITY_CONTEXT)
}

/// Derive the per-server identity keypair from the master key.
///
/// The context is the server's pinned TLS cert when there is one (stable across
/// address changes), otherwise the endpoint. Different servers get different
/// contexts, so the keys they see can't be correlated. This is both the account's
/// registered identity key and the MLS signing key for that server.
#[must_use]
pub fn derive_for(master: &IdentityKeyPair, cert: Option<&str>, endpoint: &str) -> IdentityKeyPair {
    let context = cert.unwrap_or(endpoint);
    master.derive_for_context(context.as_bytes())
}

/// The public half of the per-server identity key, to present to a server.
#[must_use]
pub fn derived_pubkey_for(master: &IdentityKeyPair, cert: Option<&str>, endpoint: &str) -> Vec<u8> {
    derive_for(master, cert, endpoint)
        .public()
        .to_bytes()
        .to_vec()
}
