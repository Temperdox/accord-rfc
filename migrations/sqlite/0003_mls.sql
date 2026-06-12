-- =============================================================================
-- 0003_mls.sql (SQLite) - private-chat (MLS) tables. All bytes are opaque.
-- =============================================================================

ALTER TABLE devices ADD COLUMN mls_credential BLOB;

CREATE TABLE key_packages (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_package BLOB NOT NULL,
    consumed INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_key_packages_unconsumed ON key_packages(user_id, consumed);

CREATE TABLE private_messages (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    sender_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sender_device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    ciphertext BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    client_message_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (group_id, client_message_id)
);
CREATE INDEX idx_private_messages_group_seq ON private_messages(group_id, seq);

CREATE TABLE mls_inbox (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('welcome', 'commit')),
    group_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_mls_inbox_device ON mls_inbox(device_id, created_at);
