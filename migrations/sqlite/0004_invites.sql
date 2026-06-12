-- =============================================================================
-- 0004_invites.sql (SQLite) - invite tokens + server owner.
-- =============================================================================
ALTER TABLE users ADD COLUMN is_owner INTEGER NOT NULL DEFAULT 0;

CREATE TABLE invites (
    token TEXT PRIMARY KEY,
    created_by TEXT,
    revoked INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
