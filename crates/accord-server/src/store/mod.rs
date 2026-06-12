//! Storage abstraction.
//!
//! The [`Store`] trait is the single interface every service uses for
//! persistence. Two implementations back it:
//! * [`postgres::PostgresStore`] - for "platform" deployments (large servers).
//! * [`sqlite::SqliteStore`] - for **self-contained / client-hosted** servers
//! (a single file, zero external services).
//!
//! This is what lets Accord ship as a downloadable app that hosts its own server
//! with no Docker (ARCHITECTURE.md section 9 self-hosting). The backend is chosen at
//! runtime from the `database_url` scheme (`sqlite:` vs `postgres:`).
//!
//! Services depend on `Arc<dyn Store>`, never a concrete pool.

pub mod model;
pub mod postgres;
pub mod sqlite;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::ServerResult;
use model::{
    BackupRow, ClaimedKeyPackage, FriendRequestRow, GroupSummaryRow, InboxRow, PrivateMessageRow,
    PublicMessageRow, RefreshRow, RoleRow, UserRow,
};

/// Most unconsumed KeyPackages kept per device. Clients re-publish a batch on
/// every login; the newest N are kept and older surplus is dropped (peers only
/// ever claim one at a time, so a small pool is plenty).
pub const MAX_UNCONSUMED_KEY_PACKAGES: i64 = 30;

/// Backend-agnostic persistence interface. All methods are async and
/// object-safe (via `async_trait`), so services hold `Arc<dyn Store>`.
#[async_trait::async_trait]
pub trait Store: Send + Sync + std::fmt::Debug {
    // --- users ---
    async fn create_user(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
        is_owner: bool,
        is_guest: bool,
        identity_pubkey: Option<&[u8]>,
    ) -> ServerResult<Uuid>;
    async fn find_user_by_username(&self, username: &str) -> ServerResult<Option<UserRow>>;
    /// Whether the given user is a guest (open_dms DM-only account).
    async fn is_user_guest(&self, user_id: Uuid) -> ServerResult<bool>;
    /// Whether any account already uses this public identity key.
    async fn identity_exists(&self, identity_pubkey: &[u8]) -> ServerResult<bool>;
    /// Total number of registered users (used to detect the first/owner signup).
    async fn count_users(&self) -> ServerResult<i64>;
    /// Whether the given user is the server owner.
    async fn is_owner(&self, user_id: Uuid) -> ServerResult<bool>;

    // --- invites (private servers) ---
    async fn create_invite(&self, token: &str, created_by: Uuid) -> ServerResult<()>;
    async fn is_invite_valid(&self, token: &str) -> ServerResult<bool>;
    async fn revoke_invite(&self, token: &str) -> ServerResult<()>;

    // --- roles & permissions (RBAC) ---
    /// Create the `@everyone` default role with `permissions` if it doesn't exist.
    async fn ensure_default_role(&self, id: Uuid, name: &str, permissions: i64)
    -> ServerResult<()>;
    async fn create_role(&self, name: &str, permissions: i64) -> ServerResult<Uuid>;
    async fn list_roles(&self) -> ServerResult<Vec<RoleRow>>;
    async fn get_role(&self, id: Uuid) -> ServerResult<Option<RoleRow>>;
    async fn update_role(&self, id: Uuid, name: &str, permissions: i64) -> ServerResult<()>;
    async fn delete_role(&self, id: Uuid) -> ServerResult<()>;
    async fn assign_role(&self, user_id: Uuid, role_id: Uuid) -> ServerResult<()>;
    async fn unassign_role(&self, user_id: Uuid, role_id: Uuid) -> ServerResult<()>;
    async fn roles_for_user(&self, user_id: Uuid) -> ServerResult<Vec<RoleRow>>;
    /// Combined permission bits: the default role OR'd with the user's assigned
    /// roles. (Owner/ADMINISTRATOR overrides are applied above the store.)
    async fn member_permissions(&self, user_id: Uuid) -> ServerResult<i64>;

    // --- key backups (encrypted, opaque to the server) ---
    /// Insert or replace the account's encrypted key backup.
    async fn upsert_backup(
        &self,
        user_id: Uuid,
        encrypted_blob: &[u8],
        salt: &[u8],
        argon2_params: &[u8],
        version: i32,
    ) -> ServerResult<()>;
    /// Fetch the account's encrypted key backup, if any.
    async fn get_backup(&self, user_id: Uuid) -> ServerResult<Option<BackupRow>>;
    /// Delete the account's encrypted key backup.
    async fn delete_backup(&self, user_id: Uuid) -> ServerResult<()>;

    // --- vault (opaque client-encrypted blobs keyed by name) ---
    /// Insert or replace a named encrypted blob for the account.
    async fn put_vault_blob(&self, user_id: Uuid, name: &str, blob: &[u8]) -> ServerResult<()>;
    /// Fetch a named blob, if present.
    async fn get_vault_blob(&self, user_id: Uuid, name: &str) -> ServerResult<Option<Vec<u8>>>;
    /// List blob names for the account, optionally filtered by a name prefix.
    async fn list_vault_blobs(&self, user_id: Uuid, prefix: &str) -> ServerResult<Vec<String>>;
    /// Delete a named blob.
    async fn delete_vault_blob(&self, user_id: Uuid, name: &str) -> ServerResult<()>;

    // --- devices ---
    async fn create_device(&self, user_id: Uuid, name: &str) -> ServerResult<Uuid>;
    /// The newest non-revoked device with this exact name, if any. Login reuses
    /// it so the same install keeps a stable device id across restarts (the
    /// mailbox and Welcome inbox are keyed by device id).
    async fn find_device(&self, user_id: Uuid, name: &str) -> ServerResult<Option<Uuid>>;
    async fn revoke_device(&self, device_id: Uuid) -> ServerResult<()>;
    async fn set_device_credential(&self, device_id: Uuid, credential: &[u8]) -> ServerResult<()>;

    // --- refresh tokens ---
    async fn store_refresh_token(
        &self,
        token: &str,
        user_id: Uuid,
        device_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> ServerResult<()>;
    async fn lookup_refresh_token(&self, token: &str) -> ServerResult<Option<RefreshRow>>;
    /// Invalidate a refresh token (used on rotation: old token dies on use).
    async fn delete_refresh_token(&self, token: &str) -> ServerResult<()>;

    // --- groups ---
    async fn create_public_group(&self, name: &str, description: &str) -> ServerResult<Uuid>;
    async fn create_private_group_with_id(&self, id: Uuid, name: &str) -> ServerResult<()>;
    async fn add_member(&self, group_id: Uuid, user_id: Uuid, role: &str) -> ServerResult<()>;
    async fn is_member(&self, group_id: Uuid, user_id: Uuid) -> ServerResult<bool>;
    async fn group_ids_for_user(&self, user_id: Uuid) -> ServerResult<Vec<Uuid>>;
    async fn list_groups_for_user(&self, user_id: Uuid) -> ServerResult<Vec<GroupSummaryRow>>;
    async fn get_group(&self, group_id: Uuid) -> ServerResult<GroupSummaryRow>;
    async fn member_ids(&self, group_id: Uuid) -> ServerResult<Vec<Uuid>>;
    /// All non-revoked device ids belonging to the members of a group. Used to
    /// queue private messages into offline devices' mailboxes.
    async fn device_ids_for_group(&self, group_id: Uuid) -> ServerResult<Vec<Uuid>>;

    // --- public messages ---
    async fn insert_public_message(
        &self,
        group_id: Uuid,
        sender_id: Uuid,
        content: &str,
        client_message_id: Uuid,
    ) -> ServerResult<PublicMessageRow>;
    async fn fetch_public_history(
        &self,
        group_id: Uuid,
        before_seq: i64,
        limit: i64,
    ) -> ServerResult<Vec<PublicMessageRow>>;

    // --- MLS: key packages, private messages, handshake inbox ---
    async fn store_key_packages(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        packages: &[Vec<u8>],
    ) -> ServerResult<u32>;
    async fn claim_key_packages_for_user(
        &self,
        user_id: Uuid,
    ) -> ServerResult<Vec<ClaimedKeyPackage>>;
    async fn store_private_message(
        &self,
        group_id: Uuid,
        sender_id: Uuid,
        sender_device_id: Uuid,
        ciphertext: &[u8],
        epoch: i64,
        client_message_id: Uuid,
    ) -> ServerResult<PrivateMessageRow>;
    async fn fetch_private_history(
        &self,
        group_id: Uuid,
        before_seq: i64,
        limit: i64,
    ) -> ServerResult<Vec<PrivateMessageRow>>;
    async fn enqueue_inbox(
        &self,
        device_id: Uuid,
        kind: &str,
        group_id: Uuid,
        payload: &[u8],
    ) -> ServerResult<()>;
    async fn drain_inbox(&self, device_id: Uuid) -> ServerResult<Vec<InboxRow>>;

    // --- friend requests (parked on the recipient's home node) ---
    /// Park (or refresh) a friend request for `recipient`. Upserts on
    /// (recipient, sender_identity, kind) so re-sends update the stored code.
    async fn upsert_friend_request(
        &self,
        recipient: Uuid,
        sender_identity: &[u8],
        kind: &str,
        contact_code: &str,
    ) -> ServerResult<()>;
    /// Pending requests addressed to `recipient`, oldest first.
    async fn list_friend_requests(&self, recipient: Uuid) -> ServerResult<Vec<FriendRequestRow>>;
    /// Remove a handled request (scoped to `recipient` so users can only delete
    /// their own).
    async fn delete_friend_request(&self, recipient: Uuid, id: &str) -> ServerResult<()>;
    /// How many requests are parked for `recipient` (for the anti-spam cap).
    async fn count_friend_requests(&self, recipient: Uuid) -> ServerResult<i64>;
}

/// Connect to the configured database, run migrations, and return a boxed store.
///
/// Backend is selected from the URL scheme: `sqlite:` -> [`sqlite::SqliteStore`]
/// (self-contained), anything else -> [`postgres::PostgresStore`].
///
/// # Errors
/// Returns a [`ServerError`](crate::error::ServerError) if connection or
/// migration fails.
pub async fn connect(database_url: &str, max_connections: u32) -> ServerResult<Arc<dyn Store>> {
    if database_url.starts_with("sqlite") {
        let store = sqlite::SqliteStore::connect(database_url).await?;
        store.migrate().await?;
        Ok(Arc::new(store))
    } else {
        let store = postgres::PostgresStore::connect(database_url, max_connections).await?;
        store.migrate().await?;
        Ok(Arc::new(store))
    }
}
