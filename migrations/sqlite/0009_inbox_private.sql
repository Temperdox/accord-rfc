-- =============================================================================
-- 0009_inbox_private.sql (SQLite) - allow 'private' in the MLS inbox kind.
-- =============================================================================
-- The offline mailbox now queues private application messages too, not just
-- handshake (welcome/commit) messages. SQLite can't alter a CHECK constraint, so
-- rebuild the table without the restrictive kind check (the server controls the
-- kind values). Data is copied across.
-- =============================================================================

CREATE TABLE mls_inbox_new (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    group_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
INSERT INTO mls_inbox_new (id, device_id, kind, group_id, payload, created_at)
    SELECT id, device_id, kind, group_id, payload, created_at FROM mls_inbox;
DROP TABLE mls_inbox;
ALTER TABLE mls_inbox_new RENAME TO mls_inbox;
CREATE INDEX idx_mls_inbox_device ON mls_inbox(device_id, created_at);
