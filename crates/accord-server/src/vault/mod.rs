//! Vault gRPC service: a per-account store of opaque, client-encrypted blobs.
//!
//! The client uses this to back up state that should survive a reinstall (the
//! MLS session state and the encrypted message archive). The server only ever
//! stores and returns bytes; it can never decrypt them.

pub mod service;

pub use service::VaultSvc;
