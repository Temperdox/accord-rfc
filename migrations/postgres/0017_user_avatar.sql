-- =============================================================================
-- 0017_user_avatar.sql (Postgres) - per-account avatar.
-- =============================================================================
-- A small inline avatar image (base64 data URL, like role/tavern icons) stored
-- on the account. Each tavern is its own account, so this doubles as the
-- per-server avatar; the home account's is the "main" avatar.
-- =============================================================================
ALTER TABLE users ADD COLUMN avatar_url TEXT NOT NULL DEFAULT '';
