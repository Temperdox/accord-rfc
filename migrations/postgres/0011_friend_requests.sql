-- =============================================================================
-- 0011_friend_requests.sql (Postgres) - friend requests parked on the
-- recipient's home node.
-- =============================================================================
-- A request carries the sender's fr code so the recipient can add them back on
-- accept. Stored server-side so requests survive restarts and wait for a
-- logged-out recipient. sender_identity (the 32-byte contact key inside the
-- code) dedupes re-sends per (recipient, sender, kind).
-- =============================================================================

CREATE TABLE friend_requests (
    id                TEXT PRIMARY KEY,
    recipient_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sender_identity   BYTEA NOT NULL,
    kind              TEXT NOT NULL CHECK (kind IN ('request', 'accept')),
    contact_code      TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (recipient_user_id, sender_identity, kind)
);
CREATE INDEX idx_friend_requests_recipient ON friend_requests(recipient_user_id, created_at);
