-- =============================================================================
-- 0009_inbox_private.sql (Postgres) - allow 'private' in the MLS inbox kind.
-- =============================================================================
-- The offline mailbox now queues private application messages for offline member
-- devices, not just handshake (welcome/commit) messages, so the kind check has to
-- admit 'private' too.
-- =============================================================================

ALTER TABLE mls_inbox DROP CONSTRAINT IF EXISTS mls_inbox_kind_check;
ALTER TABLE mls_inbox
    ADD CONSTRAINT mls_inbox_kind_check CHECK (kind IN ('welcome', 'commit', 'private'));
