//! Group/channel management gRPC service.

pub mod service;

pub use service::GroupSvc;

/// Deterministic id of the seeded default public channel (`#general`).
///
/// Seeded by migration `0002` and auto-joined on login so every account lands in
/// a shared channel out of the box (walking-skeleton convenience). It is a valid
/// UUIDv7-shaped constant.
pub const DEFAULT_PUBLIC_CHANNEL_ID: &str = "01900000-0000-7000-8000-000000000001";

/// Deterministic id of the singleton tavern-identity row (one server = one
/// tavern). Ensured at startup (`ensure_tavern`), like the `@everyone` role.
pub const TAVERN_ID: &str = "01900000-0000-7000-8000-000000000003";
