//! Encrypted key-backup gRPC service.
//!
//! The server stores one opaque, client-encrypted blob per account and hands it
//! back on request. It never sees plaintext key material; the blob is encrypted
//! on the client with a password-derived key.

pub mod service;

pub use service::BackupSvc;
