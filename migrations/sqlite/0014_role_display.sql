-- =============================================================================
-- 0014_role_display.sql (SQLite) - role display + behaviour fields.
-- =============================================================================
-- Adds Discord-style role presentation: a color (members use the color of their
-- highest role), an optional small icon stored inline as a base64 data URL,
-- whether members are hoisted (shown in a separate member-list section), and
-- whether anyone may @mention the role. `position` already existed (0005).
-- =============================================================================
ALTER TABLE roles ADD COLUMN color TEXT NOT NULL DEFAULT '';
ALTER TABLE roles ADD COLUMN icon TEXT NOT NULL DEFAULT '';
ALTER TABLE roles ADD COLUMN hoist INTEGER NOT NULL DEFAULT 0;
ALTER TABLE roles ADD COLUMN mentionable INTEGER NOT NULL DEFAULT 0;
