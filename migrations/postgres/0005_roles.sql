-- =============================================================================
-- 0005_roles.sql (Postgres) - roles & permissions (Discord-style RBAC).
-- =============================================================================
-- `permissions` is a 64-bit bitfield (see accord-types::perms). A member's
-- effective permissions = the `@everyone` default role OR'd with their assigned
-- roles; the server owner and ADMINISTRATOR override all checks. The default
-- `@everyone` role is created at startup (ensure_default_role), not seeded here,
-- so its default bits live in Rust.
-- =============================================================================

CREATE TABLE roles (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    permissions BIGINT NOT NULL DEFAULT 0,
    position INTEGER NOT NULL DEFAULT 0,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE member_roles (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
);
CREATE INDEX idx_member_roles_user ON member_roles(user_id);
