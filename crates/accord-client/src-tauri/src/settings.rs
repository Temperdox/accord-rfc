//! Local client settings (persisted as JSON in the app-data dir).
//!
//! `encrypt_at_rest`: whether to encrypt **my own** local data at rest (defaults
//! off, faster to load). Received messages are always encrypted at rest, and the
//! identity key + MLS state are always encrypted regardless.
//!
//! `friend_request_policy`: who may send me a friend request. This is a privacy
//! gate against random strangers; enforcement happens at the recipient's host when
//! the request-delivery flow lands (see FEDERATION-PLAN.md). Stored now so the
//! preference is set ahead of that.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

/// A rendezvous / mailbox node the user routes DMs through (for offline delivery
/// and reaching peers behind NAT). See FEDERATION-PLAN.md "Public rendezvous
/// nodes": `mine` = your own/trusted node (no third-party metadata); otherwise a
/// public node whose operator may see metadata (never message content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendezvousNode {
    pub url: String,
    pub label: String,
    pub mine: bool,
}

/// Which Yggdrasil peers the mesh connects through (Settings > Network).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum YggPeerMode {
    /// Peers we host (trustworthy; metadata may be logged per our policies).
    Authorized,
    /// Peers the user hosts themselves.
    Private,
    /// Community peers from the official public-peers lists (default).
    #[default]
    Public,
}

/// Who may send this user a friend request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FriendRequestPolicy {
    /// Anyone with my fr code can request (default).
    #[default]
    Everyone,
    /// Only people who share a tavern with me.
    TavernMembers,
    /// Only friends of my friends.
    FriendsOfFriends,
    /// No one - I can only add others, not be added.
    NoOne,
}

/// Default number of taverns this client will host at once.
pub const DEFAULT_MAX_HOSTED_TAVERNS: u32 = 16;
fn default_max_hosted_taverns() -> u32 {
    DEFAULT_MAX_HOSTED_TAVERNS
}

/// Persisted client settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Encrypt my own messages at rest (received messages are always encrypted).
    #[serde(default)]
    pub encrypt_at_rest: bool,
    /// Who may send me a friend request.
    #[serde(default)]
    pub friend_request_policy: FriendRequestPolicy,
    /// Run the Yggdrasil mesh node (for internet DMs without port-forwarding).
    /// Requires a mesh-enabled build and elevated privileges (TUN). Auto-started
    /// on launch when set.
    #[serde(default)]
    pub mesh_enabled: bool,
    /// The rendezvous / mailbox node to route DMs through, if any.
    #[serde(default)]
    pub rendezvous_node: Option<RendezvousNode>,
    /// Which Yggdrasil peers to connect through.
    #[serde(default)]
    pub ygg_peer_mode: YggPeerMode,
    /// User-hosted peer URIs (used when `ygg_peer_mode` is `Private`).
    #[serde(default)]
    pub ygg_private_peers: Vec<String>,
    /// Max number of taverns this client will host at once. Each hosted tavern
    /// uses a TCP port from 50052 upward, so raising this consumes more ports and
    /// may collide with other software (the UI warns about this).
    #[serde(default = "default_max_hosted_taverns")]
    pub max_hosted_taverns: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            encrypt_at_rest: false,
            friend_request_policy: FriendRequestPolicy::default(),
            mesh_enabled: false,
            rendezvous_node: None,
            ygg_peer_mode: YggPeerMode::default(),
            ygg_private_peers: Vec::new(),
            max_hosted_taverns: DEFAULT_MAX_HOSTED_TAVERNS,
        }
    }
}

/// Snapshot of the Yggdrasil peer config (mode + private peers).
#[must_use]
pub fn ygg_config(app: &AppHandle) -> (YggPeerMode, Vec<String>) {
    app.state::<SharedSettings>()
        .lock()
        .map(|s| (s.ygg_peer_mode, s.ygg_private_peers.clone()))
        .unwrap_or_default()
}

/// Persist the Yggdrasil peer config (called by the mesh connect command).
pub fn store_ygg_config(
    app: &AppHandle,
    mode: YggPeerMode,
    private_peers: Vec<String>,
) -> Result<(), String> {
    let state = app.state::<SharedSettings>();
    {
        let mut s = state
            .lock()
            .map_err(|_| "settings lock poisoned".to_owned())?;
        s.ygg_peer_mode = mode;
        s.ygg_private_peers = private_peers;
    }
    let snapshot = state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .clone();
    save_to_disk(app, &snapshot)
}

/// The user's friend-request policy (cheap, synchronous).
#[must_use]
pub fn friend_request_policy(app: &AppHandle) -> FriendRequestPolicy {
    app.state::<SharedSettings>()
        .lock()
        .map(|s| s.friend_request_policy)
        .unwrap_or_default()
}

/// Whether "run the mesh on launch" is set (cheap, synchronous).
#[must_use]
pub fn is_mesh_enabled(app: &AppHandle) -> bool {
    app.state::<SharedSettings>()
        .lock()
        .map(|s| s.mesh_enabled)
        .unwrap_or(false)
}

/// Persist the mesh-enabled preference (the actual start/stop is in [`crate::mesh`]).
pub fn store_mesh_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let state = app.state::<SharedSettings>();
    state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .mesh_enabled = enabled;
    let snapshot = state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .clone();
    save_to_disk(app, &snapshot)
}

/// Managed-state alias.
pub type SharedSettings = Mutex<Settings>;

fn path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

/// Read settings from disk, or defaults if absent/unreadable.
#[must_use]
pub fn load_from_disk(app: &AppHandle) -> Settings {
    path(app)
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_to_disk(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path(app)?, bytes).map_err(|e| e.to_string())
}

/// Persist the current in-memory settings to disk.
fn persist(app: &AppHandle, state: &State<'_, SharedSettings>) -> Result<(), String> {
    let snapshot = state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .clone();
    save_to_disk(app, &snapshot)
}

/// Whether "encrypt my own data at rest" is enabled (cheap, synchronous).
#[must_use]
pub fn is_encrypt_at_rest(app: &AppHandle) -> bool {
    app.state::<SharedSettings>()
        .lock()
        .map(|s| s.encrypt_at_rest)
        .unwrap_or(false)
}

/// Get the current settings (for the UI).
#[tauri::command]
pub fn get_settings(state: State<'_, SharedSettings>) -> Settings {
    state.lock().map(|s| s.clone()).unwrap_or_default()
}

/// Toggle "encrypt my own data at rest" and persist it.
#[tauri::command]
pub fn set_encrypt_at_rest(
    app: AppHandle,
    state: State<'_, SharedSettings>,
    enabled: bool,
) -> Result<(), String> {
    state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .encrypt_at_rest = enabled;
    persist(&app, &state)
}

/// Set who may send me a friend request and persist it.
#[tauri::command]
pub fn set_friend_request_policy(
    app: AppHandle,
    state: State<'_, SharedSettings>,
    policy: FriendRequestPolicy,
) -> Result<(), String> {
    state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .friend_request_policy = policy;
    persist(&app, &state)
}

/// Max taverns this client will host at once (cheap, synchronous).
#[must_use]
pub fn max_hosted_taverns(app: &AppHandle) -> u32 {
    app.state::<SharedSettings>()
        .lock()
        .map(|s| s.max_hosted_taverns)
        .unwrap_or(DEFAULT_MAX_HOSTED_TAVERNS)
}

/// Set the max number of hosted taverns and persist it. Clamped to a sane range;
/// the UI warns that higher values consume more ports (50052+).
#[tauri::command]
pub fn set_max_hosted_taverns(
    app: AppHandle,
    state: State<'_, SharedSettings>,
    max: u32,
) -> Result<(), String> {
    state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .max_hosted_taverns = max.clamp(1, 200);
    persist(&app, &state)
}

/// Set (or clear, when `node` is null) the rendezvous node and persist it.
#[tauri::command]
pub fn set_rendezvous_node(
    app: AppHandle,
    state: State<'_, SharedSettings>,
    node: Option<RendezvousNode>,
) -> Result<(), String> {
    state
        .lock()
        .map_err(|_| "settings lock poisoned".to_owned())?
        .rendezvous_node = node;
    persist(&app, &state)
}
