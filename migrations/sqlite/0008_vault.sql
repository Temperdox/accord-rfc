-- =============================================================================
-- 0008_vault.sql (SQLite) - per-account store of opaque encrypted blobs.
-- =============================================================================
-- Client-encrypted state that should survive a reinstall (MLS session state,
-- message-history archive). The server only stores bytes; it can never decrypt
-- them. Rows are removed when the account is deleted.
-- =============================================================================

CREATE TABLE vault_blobs (
    user_id    TEXT NOT NULL,
    name       TEXT NOT NULL,
    blob       BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, name)
);
