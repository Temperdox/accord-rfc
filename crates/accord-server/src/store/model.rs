//! Plain data structs returned by the [`Store`](super::Store) trait.
//!
//! These are backend-agnostic. The Postgres implementation can decode straight
//! into them via `sqlx::FromRow`; the SQLite implementation builds them by hand
//! (it stores ids as TEXT and timestamps as integer milliseconds, so column
//! types differ).

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A user account row.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)] // username/display_name selected for completeness
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub password_hash: String,
    /// Guest accounts (open_dms) carry DMs only - no channels, no permissions.
    pub is_guest: bool,
}

/// A group plus its current member count.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GroupSummaryRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub member_count: i64,
}

/// A stored public (plaintext) message joined with the sender's display name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PublicMessageRow {
    pub id: Uuid,
    pub group_id: Uuid,
    pub sender_id: Uuid,
    pub sender_display_name: String,
    pub content: String,
    pub seq: i64,
    pub created_at: DateTime<Utc>,
}

/// A stored private (encrypted) message.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)] // id/sender_device_id selected for completeness
pub struct PrivateMessageRow {
    pub id: Uuid,
    pub group_id: Uuid,
    pub sender_id: Uuid,
    pub sender_device_id: Uuid,
    pub ciphertext: Vec<u8>,
    pub epoch: i64,
    pub seq: i64,
    pub created_at: DateTime<Utc>,
}

/// A refresh token's owner + expiry.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RefreshRow {
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// A claimed KeyPackage (which device, and its bytes).
#[derive(Debug, Clone)]
pub struct ClaimedKeyPackage {
    pub device_id: Uuid,
    pub key_package: Vec<u8>,
}

/// A queued MLS handshake message for a device.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InboxRow {
    pub kind: String,
    pub group_id: Uuid,
    pub payload: Vec<u8>,
}

/// A role row. `permissions` is the 64-bit bitfield stored as i64 bit-pattern.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoleRow {
    pub id: Uuid,
    pub name: String,
    pub permissions: i64,
    pub position: i32,
    pub is_default: bool,
}

/// A password-encrypted key backup (opaque ciphertext + public KDF inputs).
#[derive(Debug, Clone)]
pub struct BackupRow {
    pub encrypted_blob: Vec<u8>,
    pub salt: Vec<u8>,
    pub argon2_params: Vec<u8>,
    pub version: i32,
}

/// A friend request parked on this node for a local user. Carries the sender's
/// fr code so the recipient can add them back on accept.
#[derive(Debug, Clone)]
pub struct FriendRequestRow {
    pub id: String,
    pub kind: String,
    pub contact_code: String,
    pub created_at_ms: i64,
}
