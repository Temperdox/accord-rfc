-- =============================================================================
-- 0002_seed_general.sql (SQLite) - seed the default public channel.
-- =============================================================================
INSERT INTO groups (id, name, description, kind, created_at)
VALUES (
    '01900000-0000-7000-8000-000000000001',
    'general',
    'Default public channel - welcome to Accord!',
    'public',
    0
)
ON CONFLICT (id) DO NOTHING;
