-- =============================================================================
-- 0013_moderation.sql (Postgres) - guardrail audit log, bans, bot principal.
-- =============================================================================
-- audit_log records sensitive/throttled actions decided by the guardrail layer
-- (src/guardrails/); the live ModAlert fan-out is separate (the hub). bans is
-- account-level for now: ban_tag_commitment is the BAN-PLAN.md Layer-2 seam (a
-- per-server PRF commitment H(T)) and stays NULL until that layer ships. is_bot
-- is the BOT-API-PLAN.md Phase-1 principal flag (no BotService yet).
-- =============================================================================

ALTER TABLE users ADD COLUMN is_bot BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE bans (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    ban_tag_commitment BYTEA,
    banned_by UUID NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    device_ban BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE audit_log (
    id UUID PRIMARY KEY,
    actor_id UUID NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT '',
    verdict TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_audit_log_created ON audit_log(created_at);
