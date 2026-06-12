//! MLS distribution relay gRPC service.
//!
//! Implements `MlsService` (`proto/mls.proto`) as a pure opaque relay: it stores
//! and forwards KeyPackages, Welcomes, and Commits without ever parsing them
//! (ARCHITECTURE.md section 5, section 8.3). All MLS logic is client-side.

pub mod service;

pub use service::MlsRelaySvc;
