-- =============================================================================
-- 0010_guests.sql (SQLite) - guest accounts (DM-only, no channel access).
-- =============================================================================
-- A guest is an account created via open_dms on a private server: it exists only
-- to carry end-to-end-encrypted DMs with members. Guests get no role permissions,
-- are never auto-joined to channels, and cannot join channels themselves.
-- =============================================================================

ALTER TABLE users ADD COLUMN is_guest INTEGER NOT NULL DEFAULT 0;
