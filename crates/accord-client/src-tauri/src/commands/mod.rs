//! Tauri IPC command handlers - the bridge the React frontend calls via
//! `invoke()`. Grouped by concern:
//! * [`auth`] - connect, register, login.
//! * [`messaging`] - channels, sending, history, and the live stream.

pub mod accounts;
pub mod auth;
pub mod blocks;
pub mod contacts;
pub mod dto;
pub mod friends;
pub mod messaging;
pub mod mls;
pub mod server;
pub mod voice;
