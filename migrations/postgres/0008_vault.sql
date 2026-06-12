-- =============================================================================
-- 0008_vault.sql (Postgres) - per-account store of opaque encrypted blobs.
-- =============================================================================
-- Holds client-encrypted state that should survive a reinstall (MLS session
-- state, message-history archive). The server only stores bytes; it can never
-- decrypt them. Rows are removed when the account is deleted.
-- =============================================================================

CREATE TABLE vault_blobs (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    blob       BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, name)
);
