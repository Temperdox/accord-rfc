-- =============================================================================
-- 0015_tavern_banner.sql (Postgres) - tavern banner image.
-- =============================================================================
-- The tavern icon already exists (tavern_info.icon_url, 0012). This adds a wide
-- banner image shown atop the channel sidebar. Like role icons, both the icon and
-- banner are stored inline as small base64 data URLs (no blob store needed); the
-- *_url column names are historical.
-- =============================================================================
ALTER TABLE tavern_info ADD COLUMN banner_url TEXT NOT NULL DEFAULT '';
