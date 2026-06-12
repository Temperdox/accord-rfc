//! # accord-types
//!
//! Shared domain types for Accord, used by both the server and the client.
//!
//! The central types here are **strongly-typed identifier newtypes** -
//! [`UserId`], [`DeviceId`], [`GroupId`], and [`MessageId`]. On the wire these
//! are plain UUIDv7 strings (see `proto/common.proto`), but inside Rust we wrap
//! them in distinct types so the compiler stops us from, say, passing a
//! `GroupId` where a `UserId` is expected.
//!
//! Why UUIDv7? It embeds a millisecond timestamp in its high bits, so IDs are
//! naturally time-sortable. The server can use a freshly-minted ID as a coarse
//! ordering key without a separate timestamp column.

pub mod contact;
pub mod invite;
pub mod perms;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Errors that can occur when constructing domain types from untrusted input.
#[derive(Debug, thiserror::Error)]
pub enum IdError {
    /// The provided string was not a valid UUID.
    #[error("invalid id: not a valid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
}

/// Generates a UUIDv7-backed identifier newtype with a consistent API.
///
/// Each generated type provides:
/// * [`generate`](UserId::generate) - mint a fresh time-sortable ID.
/// * [`from_uuid`](UserId::from_uuid) / [`as_uuid`](UserId::as_uuid) - interop.
/// * `Display`, `FromStr`, and serde (serialized transparently as a string).
macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Mint a fresh, time-sortable identifier (UUIDv7).
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an existing [`Uuid`].
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Borrow the inner [`Uuid`].
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// Parse from the canonical hyphenated string form.
            ///
            /// # Errors
            /// Returns [`IdError::InvalidUuid`] if `s` is not a valid UUID.
            pub fn parse(s: &str) -> Result<Self, IdError> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Hyphenated lowercase - the canonical form used on the wire.
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // e.g. `UserId(0190...)` - keeps the type visible in logs.
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

id_newtype! {
    /// Identifies a user account. Stable for the account's lifetime.
    UserId
}
id_newtype! {
    /// Identifies a single device (one MLS ratchet-tree leaf). A user may own many.
    DeviceId
}
id_newtype! {
    /// Identifies a chat - a public channel or a private MLS group.
    GroupId
}
id_newtype! {
    /// Server-assigned identifier for a stored message.
    MessageId
}

/// The two fundamentally different chat models Accord supports.
///
/// See `ARCHITECTURE.md` section 4. The distinction is total: public chats are
/// plaintext and server-readable; private chats are MLS end-to-end encrypted and
/// opaque to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    /// Unencrypted, community-scale channel.
    Public,
    /// End-to-end encrypted (MLS) DM or group.
    Private,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_unique_and_roundtrip() {
        let a = UserId::generate();
        let b = UserId::generate();
        assert_ne!(a, b);

        let parsed = UserId::parse(&a.to_string()).expect("should parse own output");
        assert_eq!(a, parsed);
    }

    #[test]
    fn rejects_invalid_uuid() {
        assert!(GroupId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn uuidv7_ids_sort_by_creation_time() {
        let first = MessageId::generate();
        let second = MessageId::generate();
        // UUIDv7 high bits are a timestamp, so later IDs sort after earlier ones.
        assert!(first <= second);
    }
}
