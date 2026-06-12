//! Permissions - a Discord-style 64-bit permission bitfield shared by client and
//! server.
//!
//! A [`Permissions`] value is a set of capability bits. Roles carry a
//! `Permissions` value; a member's **effective** permissions are the union of
//! their roles (plus the base `@everyone` role). [`Permissions::ADMINISTRATOR`]
//! short-circuits every check (grants everything), exactly like Discord.
//!
//! ## Wire format
//! Permissions serialize as a **decimal string**, not a number - a `u64` can
//! exceed JavaScript's safe-integer range, so (like Discord) we pass them as
//! strings across the API.
//!
//! Add new permissions by appending a constant + a [`NAMES`] entry; existing
//! stored values keep working (bits are stable).

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A set of permission bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Permissions(u64);

impl Permissions {
    /// Grants every permission and overrides all checks.
    pub const ADMINISTRATOR: Self = Self(1 << 0);
    /// See channels and read messages.
    pub const VIEW_CHANNELS: Self = Self(1 << 1);
    /// Send messages in channels.
    pub const SEND_MESSAGES: Self = Self(1 << 2);
    /// Delete or pin others' messages (moderation).
    pub const MANAGE_MESSAGES: Self = Self(1 << 3);
    /// Create, edit, delete channels.
    pub const MANAGE_CHANNELS: Self = Self(1 << 4);
    /// Create, edit, delete, and assign roles.
    pub const MANAGE_ROLES: Self = Self(1 << 5);
    /// Create invite keys.
    pub const CREATE_INVITE: Self = Self(1 << 6);
    /// Remove members from the server.
    pub const KICK_MEMBERS: Self = Self(1 << 7);
    /// Ban members from the server.
    pub const BAN_MEMBERS: Self = Self(1 << 8);
    /// Change server settings.
    pub const MANAGE_SERVER: Self = Self(1 << 9);
    /// Start private (MLS) chats / DMs.
    pub const CREATE_PRIVATE_CHAT: Self = Self(1 << 10);
    /// Mention @everyone.
    pub const MENTION_EVERYONE: Self = Self(1 << 11);
    /// Attach files.
    pub const ATTACH_FILES: Self = Self(1 << 12);
    /// Add reactions.
    pub const ADD_REACTIONS: Self = Self(1 << 13);

    /// No permissions.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Every defined permission.
    #[must_use]
    pub const fn all() -> Self {
        let mut bits = 0u64;
        let mut i = 0;
        while i < NAMES.len() {
            bits |= NAMES[i].1.0;
            i += 1;
        }
        Self(bits)
    }

    /// The base permissions granted to every member via `@everyone`.
    #[must_use]
    pub const fn default_everyone() -> Self {
        Self(
            Self::VIEW_CHANNELS.0
                | Self::SEND_MESSAGES.0
                | Self::CREATE_INVITE.0
                | Self::CREATE_PRIVATE_CHAT.0
                | Self::ADD_REACTIONS.0
                | Self::ATTACH_FILES.0,
        )
    }

    /// Raw bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Construct from raw bits (unknown bits are kept; introspect via [`NAMES`]).
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Whether `self` includes all bits in `other`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether this set has ADMINISTRATOR (which grants everything).
    #[must_use]
    pub const fn is_admin(self) -> bool {
        self.contains(Self::ADMINISTRATOR)
    }

    /// Union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// **Authorization check**: does this set allow `perm`? ADMINISTRATOR always
    /// passes.
    #[must_use]
    pub const fn allows(self, perm: Self) -> bool {
        self.is_admin() || self.contains(perm)
    }

    /// The names of the permissions present in this set (for UIs/logging).
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        NAMES
            .iter()
            .filter(|(_, p)| self.contains(*p))
            .map(|(n, _)| *n)
            .collect()
    }
}

/// Stable (name, bit) table - the source of truth for [`Permissions::all`],
/// introspection, and permission editors.
pub const NAMES: &[(&str, Permissions)] = &[
    ("ADMINISTRATOR", Permissions::ADMINISTRATOR),
    ("VIEW_CHANNELS", Permissions::VIEW_CHANNELS),
    ("SEND_MESSAGES", Permissions::SEND_MESSAGES),
    ("MANAGE_MESSAGES", Permissions::MANAGE_MESSAGES),
    ("MANAGE_CHANNELS", Permissions::MANAGE_CHANNELS),
    ("MANAGE_ROLES", Permissions::MANAGE_ROLES),
    ("CREATE_INVITE", Permissions::CREATE_INVITE),
    ("KICK_MEMBERS", Permissions::KICK_MEMBERS),
    ("BAN_MEMBERS", Permissions::BAN_MEMBERS),
    ("MANAGE_SERVER", Permissions::MANAGE_SERVER),
    ("CREATE_PRIVATE_CHAT", Permissions::CREATE_PRIVATE_CHAT),
    ("MENTION_EVERYONE", Permissions::MENTION_EVERYONE),
    ("ATTACH_FILES", Permissions::ATTACH_FILES),
    ("ADD_REACTIONS", Permissions::ADD_REACTIONS),
];

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Permissions {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(Self)
    }
}

// Serialize as a decimal string (JS-safe).
impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct PermVisitor;
        impl Visitor<'_> for PermVisitor {
            type Value = Permissions;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a permissions bitfield as a string or unsigned integer")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Permissions, E> {
                v.parse::<u64>().map(Permissions).map_err(de::Error::custom)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Permissions, E> {
                Ok(Permissions(v))
            }
        }
        d.deserialize_any(PermVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_allows_everything() {
        let admin = Permissions::ADMINISTRATOR;
        assert!(admin.allows(Permissions::BAN_MEMBERS));
        assert!(admin.allows(Permissions::MANAGE_ROLES));
        assert!(admin.is_admin());
    }

    #[test]
    fn union_and_contains() {
        let p = Permissions::SEND_MESSAGES.union(Permissions::CREATE_INVITE);
        assert!(p.allows(Permissions::SEND_MESSAGES));
        assert!(p.allows(Permissions::CREATE_INVITE));
        assert!(!p.allows(Permissions::BAN_MEMBERS));
    }

    #[test]
    fn default_everyone_can_chat_not_moderate() {
        let p = Permissions::default_everyone();
        assert!(p.allows(Permissions::SEND_MESSAGES));
        assert!(p.allows(Permissions::CREATE_INVITE));
        assert!(!p.allows(Permissions::MANAGE_ROLES));
        assert!(!p.allows(Permissions::BAN_MEMBERS));
    }

    #[test]
    fn serde_is_a_string() {
        let p = Permissions::from_bits(1094);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"1094\"");
        assert_eq!(serde_json::from_str::<Permissions>(&json).unwrap(), p);
        // also accepts a raw integer
        assert_eq!(serde_json::from_str::<Permissions>("1094").unwrap(), p);
    }

    #[test]
    fn all_includes_named() {
        assert!(Permissions::all().contains(Permissions::ADD_REACTIONS));
        assert_eq!(Permissions::all().names().len(), NAMES.len());
    }
}
