//! Tauri-managed application state.
//!
//! The client can be connected to several servers at once (Discord-style). Each
//! connection is a [`Session`]; [`Sessions`] holds them keyed by a client-chosen
//! server id, plus which one the UI is currently viewing (`active`). Switching
//! servers just changes `active` - the other sessions stay connected in the
//! background, each with its own stream supervisor.

use std::collections::HashMap;
use std::sync::Arc;

use accord_mls::MlsEngine;
use accord_proto::ClientMessage;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tonic::transport::Channel;

/// Shared handle to one server's MLS engine. Shared between command handlers and
/// that session's inbound stream task, hence `Arc<Mutex<…>>`.
pub type SharedEngine = Arc<Mutex<MlsEngine>>;

/// One connected server.
#[derive(Default)]
pub struct Session {
    /// Connected gRPC channel to the server (set by the `connect` command).
    pub channel: Option<Channel>,
    /// The endpoint URL this session connected to. Fallback per-server context
    /// for deriving the account identity key.
    pub endpoint: Option<String>,
    /// The server's pinned TLS cert, if any. Preferred per-server identity-key
    /// context (stable across address changes).
    pub cert: Option<String>,
    /// The logged-in user's id on this server (keys per-account local state).
    pub user_id: Option<String>,
    /// JWT access token (refreshed periodically / on demand).
    pub token: Option<String>,
    /// Long-lived refresh token used to mint fresh access tokens.
    pub refresh_token: Option<String>,
    /// Background tasks for this session (stream supervisor + token refresh).
    /// Aborted when the session is replaced/reconnected.
    pub session_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Sends `ClientMessage`s into this session's open stream.
    pub outbound: Option<mpsc::Sender<ClientMessage>>,
    /// This device's MLS engine for this server.
    pub engine: Option<SharedEngine>,
}

/// All connected servers plus which one the UI is viewing.
#[derive(Default)]
pub struct Sessions {
    pub map: HashMap<String, Session>,
    pub active: Option<String>,
}

impl Sessions {
    /// The active session, if any.
    #[must_use]
    pub fn active(&self) -> Option<&Session> {
        self.active.as_ref().and_then(|id| self.map.get(id))
    }

    /// Mutable access to the active session.
    pub fn active_mut(&mut self) -> Option<&mut Session> {
        match &self.active {
            Some(id) => self.map.get_mut(id),
            None => None,
        }
    }

    /// Get a session by id, inserting a default if absent.
    pub fn entry(&mut self, id: &str) -> &mut Session {
        self.map.entry(id.to_owned()).or_default()
    }

    /// The active session's channel + access token, if logged in.
    #[must_use]
    pub fn active_channel_token(&self) -> Option<(Channel, String)> {
        let s = self.active()?;
        Some((s.channel.clone()?, s.token.clone()?))
    }

    /// The active session's channel, if connected.
    #[must_use]
    pub fn active_channel(&self) -> Option<Channel> {
        self.active()?.channel.clone()
    }

    /// Channel + token for the session owning `user_id` (for per-session vault
    /// uploads from background sessions).
    #[must_use]
    pub fn creds_for_user(&self, user_id: &str) -> Option<(Channel, String)> {
        let s = self
            .map
            .values()
            .find(|s| s.user_id.as_deref() == Some(user_id))?;
        Some((s.channel.clone()?, s.token.clone()?))
    }
}

/// The managed-state handle type used throughout the commands.
pub type SharedSessions = Mutex<Sessions>;
