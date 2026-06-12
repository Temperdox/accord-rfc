-- =============================================================================
-- 0002_seed_general.sql - Seed the default public channel.
-- =============================================================================
-- Every account is auto-joined to this channel on login (see auth/service.rs and
-- groups/mod.rs::DEFAULT_PUBLIC_CHANNEL_ID) so a fresh install has somewhere to
-- chat immediately. The id is a fixed, well-known UUIDv7-shaped constant.
-- =============================================================================

INSERT INTO groups (id, name, description, kind)
VALUES (
    '01900000-0000-7000-8000-000000000001',
    'general',
    'Default public channel - welcome to Accord!',
    'public'
)
ON CONFLICT (id) DO NOTHING;
