-- =============================================================================
-- 0014_role_display.sql (Postgres) - role display + behaviour fields.
-- =============================================================================
-- Adds Discord-style role presentation: a color (members use the color of their
-- highest role), an optional small icon stored inline as a base64 data URL,
-- whether members are hoisted (shown in a separate member-list section), and
-- whether anyone may @mention the role. `position` already existed (0005).
-- =============================================================================
ALTER TABLE roles ADD COLUMN color TEXT NOT NULL DEFAULT '';
ALTER TABLE roles ADD COLUMN icon TEXT NOT NULL DEFAULT '';
ALTER TABLE roles ADD COLUMN hoist BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE roles ADD COLUMN mentionable BOOLEAN NOT NULL DEFAULT FALSE;
