//! Real-time messaging: the delivery [`hub`] and the gRPC [`service`].

pub mod hub;
pub mod service;

pub use hub::Hub;
pub use service::MessagingSvc;
