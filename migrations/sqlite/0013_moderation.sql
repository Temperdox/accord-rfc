-- =============================================================================
-- 0013_moderation.sql (SQLite) - guardrail audit log, bans, bot principal.
-- =============================================================================
-- Timestamps are integer unix-ms (computed in Rust), per the SQLite convention.
-- =============================================================================

ALTER TABLE users ADD COLUMN is_bot INTEGER NOT NULL DEFAULT 0;

CREATE TABLE bans (
    user_id TEXT PRIMARY KEY,
    ban_tag_commitment BLOB,
    banned_by TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    device_ban INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id TEXT PRIMARY KEY,
    actor_id TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT '',
    verdict TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_audit_log_created ON audit_log(created_at);
