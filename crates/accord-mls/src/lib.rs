//! # accord-mls
//!
//! Client-side wrapper around the **MLS** protocol (RFC 9420), backed by
//! [OpenMLS](https://github.com/openmls/openmls). This crate drives Accord's
//! private (end-to-end encrypted) chats. It is **client-only** - the server
//! never links it, which structurally guarantees the server cannot perform MLS
//! operations or read plaintext (ARCHITECTURE.md section 5, section 8.3).
//!
//! ## The engine model
//! OpenMLS centralizes all cryptographic state (signature keys, KeyPackage
//! private material, and per-group ratchet state) inside a *provider*'s storage.
//! [`MlsEngine`] owns that provider plus the device's signing identity, and
//! exposes the handful of operations Accord needs, each taking/returning plain
//! `Vec<u8>` wire bytes so the rest of the app never touches OpenMLS types:
//!
//! * [`MlsEngine::new`] / [`MlsEngine::from_serialized`] - create or restore a device.
//! * [`MlsEngine::generate_key_packages`] - KeyPackages to publish to the server.
//! * [`MlsEngine::create_group`] - start a new group with this device as sole member.
//! * [`MlsEngine::add_member`] - add a device, producing a Commit + Welcome.
//! * [`MlsEngine::join_from_welcome`] - join a group from a received Welcome.
//! * [`MlsEngine::process_commit`] - apply a Commit from another member.
//! * [`MlsEngine::encrypt`] / [`MlsEngine::decrypt`] - application messages.
//!
//! ## Ciphersuite (ARCHITECTURE.md section 5.5)
//! `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`.

pub mod engine;

/// The Ed25519 identity keypair an engine signs with (re-exported from
/// `accord-crypto`), so callers can construct an [`MlsEngine`] without depending
/// on `accord-crypto` directly.
pub use accord_crypto::identity::IdentityKeyPair;
pub use engine::{DecryptOutcome, MlsEngine};

/// Opaque MLS Commit bytes (advances the group epoch). The server relays these
/// without parsing them.
#[derive(Debug, Clone)]
pub struct Commit(pub Vec<u8>);

/// Opaque MLS Welcome bytes (bootstraps a newly-added member into the group).
#[derive(Debug, Clone)]
pub struct Welcome(pub Vec<u8>);

/// Opaque MLS application-message ciphertext.
#[derive(Debug, Clone)]
pub struct Ciphertext(pub Vec<u8>);

/// Errors from MLS operations.
#[derive(Debug, thiserror::Error)]
pub enum MlsError {
    /// A protocol-level MLS error from the underlying OpenMLS engine.
    #[error("mls protocol error: {0}")]
    Protocol(String),

    /// Wire bytes could not be (de)serialized.
    #[error("mls codec error: {0}")]
    Codec(String),

    /// The referenced group is not present in local storage.
    #[error("unknown group")]
    UnknownGroup,

    /// Engine state could not be (de)serialized for persistence/backup.
    #[error("engine state (de)serialization failed: {0}")]
    State(String),
}
