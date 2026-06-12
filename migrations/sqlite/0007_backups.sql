-- =============================================================================
-- 0007_backups.sql (SQLite) - password-encrypted key backup.
-- =============================================================================
-- One opaque, client-encrypted blob per account (ciphertext + public KDF inputs).
-- The server can never decrypt it; it only stores bytes.
-- =============================================================================

CREATE TABLE key_backups (
    user_id        TEXT PRIMARY KEY,
    encrypted_blob BLOB NOT NULL,
    salt           BLOB NOT NULL,
    argon2_params  BLOB NOT NULL,
    version        INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
