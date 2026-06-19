-- =============================================================================
-- 0016_categories.sql (Postgres) - channel categories + ordering.
-- =============================================================================
-- Channels (public groups) can belong to a named, ordered category (e.g. "Text
-- Channels", "Voice Channels"). The two defaults are seeded at startup
-- (ensure_default_categories), like @everyone / tavern_info. `category_id` = ''
-- means uncategorized (rendered above the categories). `position` orders items
-- within their list (categories among categories; channels within a category).
-- =============================================================================
CREATE TABLE categories (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE groups ADD COLUMN category_id TEXT NOT NULL DEFAULT '';
ALTER TABLE groups ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
