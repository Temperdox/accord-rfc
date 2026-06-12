-- =============================================================================
-- 0006_identity.sql (Postgres) - key-based account identity.
-- =============================================================================
-- Each account can carry a public identity key (Ed25519, 32 bytes). The client
-- derives this per-server from a hidden master key, so it uniquely identifies an
-- account with no central authority. The column is nullable (accounts may opt
-- out), and Postgres allows multiple NULLs under a UNIQUE constraint, so only
-- real keys are forced to be unique.
-- =============================================================================

ALTER TABLE users ADD COLUMN identity_key BYTEA UNIQUE;
