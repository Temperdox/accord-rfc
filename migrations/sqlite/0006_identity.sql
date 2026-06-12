-- =============================================================================
-- 0006_identity.sql (SQLite) - key-based account identity.
-- =============================================================================
-- Public identity key (Ed25519, 32 bytes), derived per-server by the client from
-- a hidden master key. SQLite can't add a UNIQUE column via ALTER TABLE, so I add
-- the column and then a unique index. The column is nullable, and SQLite allows
-- multiple NULLs under a unique index, so only real keys must be unique.
-- =============================================================================

ALTER TABLE users ADD COLUMN identity_key BLOB;
CREATE UNIQUE INDEX idx_users_identity_key ON users(identity_key);
