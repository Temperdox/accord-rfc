-- =============================================================================
-- 0012_taverns.sql (Postgres) - channel kinds + tavern (server) identity.
-- =============================================================================
-- channel_kind distinguishes text vs voice channels; it is orthogonal to
-- groups.kind (public/private = the encryption model). tavern_info is a single
-- server-level identity row (name/icon/description); a fixed-id row is ensured at
-- startup (ensure_tavern), mirroring the @everyone default role. linking_enabled
-- is the BAN-PLAN.md Layer-2 per-server account-linking toggle (placeholder; the
-- cryptographic ban-tag system is a later layer).
-- =============================================================================

ALTER TABLE groups ADD COLUMN channel_kind TEXT NOT NULL DEFAULT 'text';

CREATE TABLE tavern_info (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    icon_url TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    linking_enabled BOOLEAN NOT NULL DEFAULT FALSE
);
