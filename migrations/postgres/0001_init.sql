-- =============================================================================
-- 0001_init.sql - Initial Accord schema.
-- =============================================================================
-- Covers the walking skeleton: accounts, devices, refresh tokens, groups,
-- membership, and plaintext public messages. Private-chat tables (encrypted
-- message blobs, MLS KeyPackages, key backups) are added in later migrations.
-- =============================================================================

-- User accounts. `username` is the pseudonymous handle users present to each
-- other; `password_hash` is an Argon2id PHC string (see auth/password.rs).
CREATE TABLE users (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Devices belonging to a user. Each device is one MLS ratchet-tree leaf.
CREATE TABLE devices (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ -- NULL = active
);
CREATE INDEX idx_devices_user ON devices(user_id);

-- Long-lived opaque refresh tokens, exchanged for short-lived JWT access tokens.
CREATE TABLE refresh_tokens (
    token TEXT PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Chats. `kind` distinguishes the two models; private groups store only metadata
-- here (the cryptographic group lives on the clients).
CREATE TABLE groups (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL CHECK (kind IN ('public', 'private')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Membership + role. Roles: owner > admin > moderator > member > read-only.
CREATE TABLE group_members (
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member',
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX idx_group_members_user ON group_members(user_id);

-- Plaintext public-channel messages. `seq` (a global BIGSERIAL) gives a
-- monotonic ordering that is also monotonic within each group. The unique
-- (group_id, client_message_id) constraint enforces send idempotency.
CREATE TABLE public_messages (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    sender_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    client_message_id UUID NOT NULL,
    seq BIGSERIAL NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (group_id, client_message_id)
);
CREATE INDEX idx_public_messages_group_seq ON public_messages(group_id, seq);
