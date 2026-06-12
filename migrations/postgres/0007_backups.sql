-- =============================================================================
-- 0007_backups.sql (Postgres) - password-encrypted key backup.
-- =============================================================================
-- One opaque, client-encrypted blob per account. The server stores ciphertext
-- plus the public KDF inputs (salt + Argon2 params) so any of the user's devices
-- can reproduce the wrapping key from the password. The server can never decrypt
-- this; it only holds bytes.
-- =============================================================================

CREATE TABLE key_backups (
    user_id       UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    encrypted_blob BYTEA NOT NULL,
    salt          BYTEA NOT NULL,
    argon2_params BYTEA NOT NULL,
    version       INTEGER NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
