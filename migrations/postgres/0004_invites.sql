-- =============================================================================
-- 0004_invites.sql (Postgres) - invite tokens + server owner.
-- =============================================================================
-- Private servers are invite-only. The first account registered on a server is
-- its owner; owners mint invite tokens that others present at registration.
-- =============================================================================

ALTER TABLE users ADD COLUMN is_owner BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE invites (
    token TEXT PRIMARY KEY,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
