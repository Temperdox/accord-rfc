//! Roles & permissions (RBAC) gRPC service.

pub mod service;

pub use service::RoleSvc;

/// Fixed id of the seeded `@everyone` default role (created at startup).
pub const DEFAULT_ROLE_ID: &str = "01900000-0000-7000-8000-000000000002";
