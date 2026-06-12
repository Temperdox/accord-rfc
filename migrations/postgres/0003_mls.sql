-- =============================================================================
-- 0003_mls.sql - Tables for private (MLS / RFC 9420) chats.
-- =============================================================================
-- The server stores ONLY opaque bytes here (KeyPackages, ciphertext, Welcomes,
-- Commits). It never decrypts or parses them - it is a relay (ARCHITECTURE section 5,
-- section 8.3). All MLS logic lives in the client (accord-mls).
-- =============================================================================

-- Optional MLS credential for a device (uploaded via RegisterDevice). Opaque.
ALTER TABLE devices ADD COLUMN mls_credential BYTEA;

-- Published KeyPackages, one row per package. Others consume one per device when
-- adding that device to a group (consumed = true so it is not reused).
CREATE TABLE key_packages (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_package BYTEA NOT NULL,
    consumed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_key_packages_unconsumed ON key_packages(user_id, consumed);

-- Encrypted application messages for private groups. `ciphertext` is opaque MLS
-- bytes; `epoch` is carried for the client's bookkeeping only.
CREATE TABLE private_messages (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    sender_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sender_device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    ciphertext BYTEA NOT NULL,
    epoch BIGINT NOT NULL,
    client_message_id UUID NOT NULL,
    seq BIGSERIAL NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (group_id, client_message_id)
);
CREATE INDEX idx_private_messages_group_seq ON private_messages(group_id, seq);

-- Per-device queue for MLS handshake messages (Welcome/Commit) delivered while a
-- device was offline. Drained when the device opens its message stream.
CREATE TABLE mls_inbox (
    id UUID PRIMARY KEY,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('welcome', 'commit')),
    group_id UUID NOT NULL,
    payload BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_mls_inbox_device ON mls_inbox(device_id, created_at);
