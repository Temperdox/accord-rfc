//! Data-transfer objects sent to the React frontend.
//!
//! We map the generated protobuf types onto small `camelCase` serde structs so
//! the UI never depends on protobuf shapes and gets idiomatic JS field names.

use serde::Serialize;

/// Returned from the `login` command.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginInfo {
    pub user_id: String,
    pub device_id: String,
}

/// A channel/group summary for the sidebar.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// "text" or "voice" (public channels). Empty/"" treated as text by the UI.
    pub channel_kind: String,
    pub member_count: u32,
}

/// A tavern (server) member for the member-list panel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberDto {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub is_owner: bool,
    pub online: bool,
    pub role_ids: Vec<String>,
}

/// Tavern (server) identity for the header + settings modal.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernDto {
    pub name: String,
    pub icon_url: String,
    pub description: String,
    pub linking_enabled: bool,
}

/// The caller's effective permissions (decimal-string u64) + owner flag, for
/// gating admin affordances in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyPermsDto {
    pub permissions: String,
    pub is_owner: bool,
}

/// A ban entry for the (gated) bans list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BanDto {
    pub user_id: String,
    pub reason: String,
    pub banned_by: String,
    pub created_at_ms: i64,
}

/// Payload of the `voice-participant` event: a participant's voice state changed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceParticipantDto {
    pub server_id: String,
    pub group_id: String,
    pub user_id: String,
    pub device_id: String,
    pub joined: bool,
    pub muted: bool,
    pub camera_on: bool,
    pub screen_on: bool,
}

/// Payload of the `voice-signal` event: a relayed WebRTC signaling envelope. The
/// frontend WebRTC layer consumes these (the body is the future media seam).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSignalDto {
    pub server_id: String,
    pub group_id: String,
    pub from_device: String,
    /// "offer" | "answer" | "ice" (mapped from SignalKind).
    pub kind: String,
    /// Opaque SDP / ICE payload (serialized as a byte array for the JS side).
    pub data: Vec<u8>,
}

/// Payload of the `mod-alert` event: a guardrail decision broadcast to admins.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModAlertDto {
    pub server_id: String,
    pub actor_id: String,
    pub action: String,
    pub target: String,
    pub reason: String,
    /// "info" | "warn" | "hostile".
    pub severity: String,
    pub timestamp_ms: i64,
}

/// Returned from `open_contact_dm`: the DM group plus the (backend) session id of
/// the contact's host it lives on, so the UI can switch the active session to it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedDm {
    pub server_id: String,
    pub group: GroupDto,
}

/// A DM conversation for the Direct Messages list - a private group on the home or
/// a contact-DM session, attributed to the other member.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmConversation {
    /// Backend session the DM lives on.
    pub server_id: String,
    /// MLS group id (UUID string).
    pub group_id: String,
    /// The other member's contact-identity, hex (matches a contact id).
    pub peer_id: String,
    /// The other member's name if they're a contact, else "Unknown".
    pub peer_name: String,
    /// The other member's fingerprint (for verification / unknown senders).
    pub fingerprint: String,
}

impl GroupDto {
    /// Build from a protobuf `GroupSummary`.
    pub fn from_summary(s: accord_proto::GroupSummary) -> Self {
        // ChatKind enum: 1 = public, 2 = private (see common.proto).
        let kind = match s.kind {
            1 => "public",
            2 => "private",
            _ => "unknown",
        }
        .to_string();
        Self {
            id: s.group_id.map(|g| g.value).unwrap_or_default(),
            name: s.name,
            kind,
            channel_kind: if s.channel_kind.is_empty() {
                "text".to_string()
            } else {
                s.channel_kind
            },
            member_count: s.member_count,
        }
    }
}

/// A chat message for rendering. `timestamp_ms` is epoch milliseconds (a JS
/// `Date`-friendly format).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    /// Which connected server this came from (for routing in the UI).
    pub server_id: String,
    pub id: String,
    pub group_id: String,
    pub sender_id: String,
    pub sender_display_name: String,
    pub content: String,
    pub timestamp_ms: i64,
    pub sequence_number: u64,
}

/// A decrypted private (MLS) message for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateMessageDto {
    /// Which connected server this belongs to (for routing in the UI).
    pub server_id: String,
    pub group_id: String,
    pub sender_id: String,
    /// True when this session's own user sent it (render as "You"; otherwise the
    /// UI resolves the DM peer's name instead of showing a raw id).
    pub mine: bool,
    pub content: String,
    pub timestamp_ms: i64,
}

/// One slot in a private group's loaded history. `message` is `Some` when the
/// record was immediately available (plaintext on disk) and `None` while it is
/// still being decrypted - the UI renders a placeholder for those, keyed by `id`,
/// and fills it in when the matching `private-history-decrypted` event arrives.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub message: Option<PrivateMessageDto>,
}

/// Payload of the `private-history-decrypted` event: a now-decrypted history slot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecryptedHistory {
    pub id: String,
    pub message: PrivateMessageDto,
}

/// Payload of the `connection-status` event: whether a server's live stream is up.
/// The UI uses this to show a reconnecting indicator and to resync on reconnect.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub server_id: String,
    pub connected: bool,
}

/// Payload of the `joined-group` event: this device joined a group from a Welcome.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinedGroup {
    pub server_id: String,
    pub group_id: String,
}

impl MessageDto {
    /// Build from a protobuf `IncomingPublicMessage`.
    pub fn from_incoming_public(m: accord_proto::IncomingPublicMessage) -> Self {
        // The timestamp closure parameter is a prost_types::Timestamp; we read
        // its fields without needing to name the type.
        let timestamp_ms = m
            .timestamp
            .map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
            .unwrap_or(0);
        Self {
            server_id: String::new(), // set by the caller (the owning session)
            id: m.message_id.map(|x| x.value).unwrap_or_default(),
            group_id: m.group_id.map(|x| x.value).unwrap_or_default(),
            sender_id: m.sender_id.map(|x| x.value).unwrap_or_default(),
            sender_display_name: m.sender_display_name,
            content: m.content,
            timestamp_ms,
            sequence_number: m.sequence_number,
        }
    }
}
