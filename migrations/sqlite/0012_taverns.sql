-- =============================================================================
-- 0012_taverns.sql (SQLite) - channel kinds + tavern (server) identity.
-- =============================================================================
ALTER TABLE groups ADD COLUMN channel_kind TEXT NOT NULL DEFAULT 'text';

CREATE TABLE tavern_info (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    icon_url TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    linking_enabled INTEGER NOT NULL DEFAULT 0
);
