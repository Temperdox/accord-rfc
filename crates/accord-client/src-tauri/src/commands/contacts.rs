//! Contacts: your contact code, an encrypted contact list, and verification.
//!
//! This is phase 1 of cross-user DMs (FEDERATION-PLAN.md, approach D). You share
//! a contact code (your stable contact-identity public key + where you're
//! reachable); others add you and verify your fingerprint out of band. The DM
//! transport/mailbox phases build on this.
//!
//! The contact list is stored locally, encrypted at rest ([`crate::at_rest`]).

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use accord_crypto::identity::IdentityPublicKey;
use accord_types::contact::ContactCode;

/// Domain for deriving the stable contact-identity key from the master key.
const CONTACT_IDENTITY_CONTEXT: &[u8] = b"accord:contact-identity";

/// A stored contact.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredContact {
    pubkey: Vec<u8>,
    name: String,
    addresses: Vec<String>,
    cert: Option<String>,
    #[serde(default)]
    host_user_id: Option<String>,
    verified: bool,
}

/// Everything needed to open a DM on a contact's host (see commands::mls).
pub struct ContactTarget {
    pub name: String,
    pub addresses: Vec<String>,
    pub cert: Option<String>,
    pub host_user_id: Option<String>,
}

/// Look up a contact by id (hex of their public key).
#[must_use]
pub fn lookup(app: &AppHandle, id: &str) -> Option<ContactTarget> {
    load(app)
        .into_iter()
        .find(|c| to_hex(&c.pubkey) == id)
        .map(|c| ContactTarget {
            name: c.name,
            addresses: c.addresses,
            cert: c.cert,
            host_user_id: c.host_user_id,
        })
}

/// A contact as shown in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactDto {
    /// Hex of the contact's public key - the stable id used to verify/remove.
    pub id: String,
    pub name: String,
    /// Short fingerprint to compare out of band (safety-number style).
    pub fingerprint: String,
    pub addresses: Vec<String>,
    pub verified: bool,
}

fn path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("contacts.bin"))
}

fn load(app: &AppHandle) -> Vec<StoredContact> {
    let Ok(path) = path(app) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    crate::at_rest::open_bytes(&bytes)
        .and_then(|pt| serde_json::from_slice(&pt).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, contacts: &[StoredContact]) -> Result<(), String> {
    let json = serde_json::to_vec(contacts).map_err(|e| e.to_string())?;
    let blob = crate::at_rest::seal_bytes(&json)?;
    std::fs::write(path(app)?, blob).map_err(|e| e.to_string())
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Short fingerprint of a contact public key (empty if malformed).
pub(crate) fn fingerprint(pubkey: &[u8]) -> String {
    <[u8; 32]>::try_from(pubkey)
        .ok()
        .and_then(|arr| IdentityPublicKey::from_bytes(&arr).ok())
        .map(|pk| pk.fingerprint())
        .unwrap_or_default()
}

fn to_dto(c: StoredContact) -> ContactDto {
    ContactDto {
        id: to_hex(&c.pubkey),
        fingerprint: fingerprint(&c.pubkey),
        name: c.name,
        addresses: c.addresses,
        verified: c.verified,
    }
}

/// Build this device's shareable contact code.
///
/// The contact identity is derived from the master key (so it's stable and
/// recognizable, without exposing the master root). It carries where to reach
/// this device's home server (LAN + mesh address), its cert (for pinning), and
/// this user's home-server user id - everything a peer needs to open a DM here.
#[tauri::command]
pub async fn my_contact_code(app: AppHandle, name: Option<String>) -> Result<String, String> {
    use tauri::Manager;

    let master = crate::identity::load_or_create_master(&app)?;
    let pubkey = master
        .derive_for_context(CONTACT_IDENTITY_CONTEXT)
        .public()
        .to_bytes()
        .to_vec();

    let cert = crate::hosting::host_cert(&app).await;
    let scheme = if cert.is_some() { "https" } else { "http" };

    let shareable = crate::hosting::shareable(&app).await;
    let port = shareable
        .as_ref()
        .map(|(_, p, _)| *p)
        .unwrap_or(crate::hosting::DEFAULT_HOST_PORT);

    // LAN address first (fast on the same network), then the mesh address (works
    // across the internet when both peers run the mesh). Both are full endpoints
    // the peer can dial directly; the IPv6 mesh address is bracketed for the URL.
    let mut addresses = Vec::new();
    if let Some((host, p, _private)) = &shareable {
        addresses.push(format!("{scheme}://{host}:{p}"));
    }
    if let Some(addr) = crate::mesh::current_address(&app).await {
        addresses.push(format!("{scheme}://[{addr}]:{port}"));
    }

    // The user's id on their own home server, so a peer can fetch our KeyPackage.
    let host_user_id = app
        .state::<crate::state::SharedSessions>()
        .lock()
        .await
        .map
        .get("home")
        .and_then(|s| s.user_id.clone());

    Ok(ContactCode::new(pubkey)
        .with_name(name)
        .with_addresses(addresses)
        .with_cert(cert)
        .with_host_user_id(host_user_id)
        .encode())
}

/// Add (or update) a contact from a pasted contact code.
#[tauri::command]
pub fn add_contact(app: AppHandle, code: String) -> Result<ContactDto, String> {
    let parsed = ContactCode::decode(&code).map_err(|e| e.to_string())?;
    if parsed.identity_pubkey.len() != 32 {
        return Err("contact code has an invalid identity key".to_owned());
    }
    let mut contacts = load(&app);
    let name = parsed.name.clone().unwrap_or_else(|| "Unknown".to_owned());
    let dto;
    if let Some(existing) = contacts
        .iter_mut()
        .find(|c| c.pubkey == parsed.identity_pubkey)
    {
        existing.name = name;
        existing.addresses = parsed.addresses;
        existing.cert = parsed.cert;
        existing.host_user_id = parsed.host_user_id;
        dto = to_dto(existing.clone());
    } else {
        let contact = StoredContact {
            pubkey: parsed.identity_pubkey,
            name,
            addresses: parsed.addresses,
            cert: parsed.cert,
            host_user_id: parsed.host_user_id,
            verified: false,
        };
        dto = to_dto(contact.clone());
        contacts.push(contact);
    }
    save(&app, &contacts)?;
    Ok(dto)
}

/// List the saved contacts.
#[tauri::command]
pub fn list_contacts(app: AppHandle) -> Result<Vec<ContactDto>, String> {
    Ok(load(&app).into_iter().map(to_dto).collect())
}

/// Remove a contact by its id (hex public key).
#[tauri::command]
pub fn remove_contact(app: AppHandle, id: String) -> Result<(), String> {
    let mut contacts = load(&app);
    contacts.retain(|c| to_hex(&c.pubkey) != id);
    save(&app, &contacts)
}

/// Mark a contact verified (or not) after comparing fingerprints out of band.
#[tauri::command]
pub fn set_contact_verified(app: AppHandle, id: String, verified: bool) -> Result<(), String> {
    let mut contacts = load(&app);
    if let Some(c) = contacts.iter_mut().find(|c| to_hex(&c.pubkey) == id) {
        c.verified = verified;
    }
    save(&app, &contacts)
}
