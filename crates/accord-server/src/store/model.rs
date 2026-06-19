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
    /// Encryption model: `"public"` | `"private"` (maps to `ChatKind`).
    pub kind: String,
    /// Channel behaviour: `"text"` | `"voice"` (orthogonal to `kind`).
    pub channel_kind: String,
    pub member_count: i64,
    /// Category id this channel belongs to (`""` = uncategorized).
    pub category_id: String,
    /// Order within the category (or among uncategorized channels).
    pub position: i32,
}

/// A channel category (orders + groups channels in the sidebar).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CategoryRow {
    pub id: Uuid,
    pub name: String,
    pub position: i32,
}

/// A channel/server member with the bits the member list needs. Built by hand in
/// each store impl (no `FromRow`); RBAC role ids + online status are layered on
/// in the service (per-member `roles_for_user` + hub liveness).
#[derive(Debug, Clone)]
pub struct MemberRow {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    /// The server owner flag (`users.is_owner`), not the per-group role string.
    pub is_owner: bool,
    /// Small inline avatar (base64 data URL), or "" for none.
    pub avatar_url: String,
}

/// The single server-level tavern identity row.
#[derive(Debug, Clone)]
pub struct TavernRow {
    pub name: String,
    /// Small inline base64 data URL, or "" for none.
    pub icon_url: String,
    pub description: String,
    /// BAN-PLAN.md Layer-2 per-server account-linking toggle (placeholder).
    pub linking_enabled: bool,
    /// Wide banner image as a base64 data URL, or "" for none.
    pub banner_url: String,
}

/// A persistent ban (account-level). `ban_tag_commitment` (BAN-PLAN.md Layer 2)
/// is not surfaced here yet - the working subset is account-id bans.
#[derive(Debug, Clone)]
pub struct BanRow {
    pub user_id: Uuid,
    pub reason: String,
    pub banned_by: Uuid,
    pub created_at_ms: i64,
}

/// A guardrail audit-log entry (sensitive/throttled/denied action).
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub actor_id: Uuid,
    pub action: String,
    pub target: String,
    pub verdict: String,
    pub reason: String,
    pub created_at_ms: i64,
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
    /// Hex color (e.g. "#5865f2") or "" for the default text color.
    pub color: String,
    /// Small inline icon as a base64 data URL, or "" for none.
    pub icon: String,
    /// Display members with this role in a separate member-list section.
    pub hoist: bool,
    /// Anyone may @mention this role.
    pub mentionable: bool,
}

/// The mutable display + behaviour fields written on create/update of a role.
/// (Position is managed separately via reorder; `is_default` never changes.)
#[derive(Debug, Clone)]
pub struct RoleWrite {
    pub name: String,
    pub permissions: i64,
    pub color: String,
    pub icon: String,
    pub hoist: bool,
    pub mentionable: bool,
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
