//! SQLite implementation of [`Store`] - for **self-contained / client-hosted**
//! servers.
//!
//! SQLite is a single file with no external service, which is what lets Accord
//! ship as a downloadable app that hosts its own server (no Docker). To keep
//! things portable we store:
//! * ids as **TEXT** (the UUID string),
//! * binary blobs as **BLOB**,
//! * timestamps as **INTEGER** unix-milliseconds,
//! * `seq` as an `INTEGER PRIMARY KEY AUTOINCREMENT` column.
//!
//! Every method mirrors the Postgres semantics in [`super::postgres`].

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use uuid::Uuid;

use crate::error::{ServerError, ServerResult};
use crate::store::model::*;
use crate::store::{MAX_UNCONSUMED_KEY_PACKAGES, Store};

/// Embedded SQLite migrations (dialect differs from Postgres).
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/sqlite");

/// A [`Store`] backed by a single SQLite file.
#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

/// Parse a stored UUID string; a failure means corrupt data (internal error).
fn parse_id(s: String) -> ServerResult<Uuid> {
    Uuid::parse_str(&s).map_err(|e| ServerError::Internal(format!("corrupt id '{s}': {e}")))
}

/// Convert stored unix-millis to a UTC timestamp.
fn ms_to_dt(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Build a [`RoleRow`] from a SQLite row (ids are TEXT, is_default is INTEGER).
fn role_from_row(r: sqlx::sqlite::SqliteRow) -> ServerResult<RoleRow> {
    Ok(RoleRow {
        id: parse_id(r.get("id"))?,
        name: r.get("name"),
        permissions: r.get("permissions"),
        position: r.get::<i64, _>("position") as i32,
        is_default: r.get::<i64, _>("is_default") != 0,
    })
}

impl SqliteStore {
    /// Open (creating the file if needed) and return the store.
    ///
    /// # Errors
    /// Returns [`ServerError::Database`] if the database cannot be opened.
    pub async fn connect(database_url: &str) -> ServerResult<Self> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(ServerError::Database)?
            .create_if_missing(true);
        // A modest pool; SQLite serializes writes anyway.
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Apply pending migrations.
    ///
    /// # Errors
    /// Returns [`ServerError::Migration`] if a migration fails.
    pub async fn migrate(&self) -> ServerResult<()> {
        self.repair_line_ending_checksums().await?;
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|e| ServerError::Migration(e.to_string()))?;
        Ok(())
    }

    /// Re-stamp applied-migration checksums that differ from the embedded ones
    /// **only by line endings** (see [`super::line_ending_variant_checksums`]).
    ///
    /// Without this, a database created by a CI-built binary (CRLF checkout)
    /// rejects a locally-built binary (LF checkout) with "migration N was
    /// previously applied but has been modified" even though the SQL is
    /// identical - which killed the embedded home node at startup. Genuinely
    /// edited migrations still fail validation.
    async fn repair_line_ending_checksums(&self) -> ServerResult<()> {
        let table: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_optional(&self.pool)
        .await?;
        if table.is_none() {
            return Ok(()); // fresh database, nothing applied yet
        }
        for m in MIGRATOR.iter() {
            let stored: Option<Vec<u8>> =
                sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                    .bind(m.version)
                    .fetch_optional(&self.pool)
                    .await?;
            let Some(stored) = stored else { continue };
            if stored.as_slice() == m.checksum.as_ref() {
                continue;
            }
            let equivalent = super::line_ending_variant_checksums(&m.sql)
                .iter()
                .any(|v| v.as_slice() == stored.as_slice());
            if equivalent {
                sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                    .bind(m.checksum.as_ref())
                    .bind(m.version)
                    .execute(&self.pool)
                    .await?;
                tracing::warn!(
                    version = m.version,
                    "re-stamped migration checksum (line-ending difference only)"
                );
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for SqliteStore {
    async fn create_user(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
        is_owner: bool,
        is_guest: bool,
        identity_pubkey: Option<&[u8]>,
    ) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        let result = sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, is_owner, is_guest, identity_key)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(username)
        .bind(display_name)
        .bind(password_hash)
        .bind(Utc::now().timestamp_millis())
        .bind(i32::from(is_owner))
        .bind(i32::from(is_guest))
        .bind(identity_pubkey)
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => Ok(id),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(
                ServerError::AlreadyExists("username or identity".to_owned()),
            ),
            Err(e) => Err(ServerError::Database(e)),
        }
    }

    async fn identity_exists(&self, identity_pubkey: &[u8]) -> ServerResult<bool> {
        let row = sqlx::query("SELECT 1 FROM users WHERE identity_key = ? LIMIT 1")
            .bind(identity_pubkey)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn find_user_by_username(&self, username: &str) -> ServerResult<Option<UserRow>> {
        let row = sqlx::query(
            "SELECT id, username, display_name, password_hash, is_guest \
             FROM users WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(r) => Ok(Some(UserRow {
                id: parse_id(r.get("id"))?,
                username: r.get("username"),
                display_name: r.get("display_name"),
                password_hash: r.get("password_hash"),
                is_guest: r.get::<i64, _>("is_guest") != 0,
            })),
        }
    }

    async fn is_user_guest(&self, user_id: Uuid) -> ServerResult<bool> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT is_guest FROM users WHERE id = ?")
            .bind(user_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some_and(|(g,)| g != 0))
    }

    async fn user_profile(&self, user_id: Uuid) -> ServerResult<Option<(String, String)>> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT username, display_name FROM users WHERE id = ?")
                .bind(user_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn count_users(&self) -> ServerResult<i64> {
        let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    async fn is_owner(&self, user_id: Uuid) -> ServerResult<bool> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT is_owner FROM users WHERE id = ?")
            .bind(user_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(matches!(row, Some((1,))))
    }

    async fn create_invite(&self, token: &str, created_by: Uuid) -> ServerResult<()> {
        sqlx::query("INSERT INTO invites (token, created_by, created_at) VALUES (?, ?, ?)")
            .bind(token)
            .bind(created_by.to_string())
            .bind(Utc::now().timestamp_millis())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn is_invite_valid(&self, token: &str) -> ServerResult<bool> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT revoked FROM invites WHERE token = ?")
            .bind(token)
            .fetch_optional(&self.pool)
            .await?;
        Ok(matches!(row, Some((0,))))
    }

    async fn revoke_invite(&self, token: &str) -> ServerResult<()> {
        sqlx::query("UPDATE invites SET revoked = 1 WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn ensure_default_role(
        &self,
        id: Uuid,
        name: &str,
        permissions: i64,
    ) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO roles (id, name, permissions, position, is_default, created_at)
             VALUES (?, ?, ?, 0, 1, ?) ON CONFLICT (id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(permissions)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_role(&self, name: &str, permissions: i64) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO roles (id, name, permissions, created_at) VALUES (?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(name)
            .bind(permissions)
            .bind(Utc::now().timestamp_millis())
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    async fn list_roles(&self) -> ServerResult<Vec<RoleRow>> {
        let rows = sqlx::query(
            "SELECT id, name, permissions, position, is_default FROM roles ORDER BY position, name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(role_from_row).collect()
    }

    async fn get_role(&self, id: Uuid) -> ServerResult<Option<RoleRow>> {
        let row = sqlx::query(
            "SELECT id, name, permissions, position, is_default FROM roles WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(role_from_row).transpose()
    }

    async fn update_role(&self, id: Uuid, name: &str, permissions: i64) -> ServerResult<()> {
        sqlx::query("UPDATE roles SET name = ?, permissions = ? WHERE id = ?")
            .bind(name)
            .bind(permissions)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_role(&self, id: Uuid) -> ServerResult<()> {
        sqlx::query("DELETE FROM roles WHERE id = ? AND is_default = 0")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn assign_role(&self, user_id: Uuid, role_id: Uuid) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO member_roles (user_id, role_id) VALUES (?, ?)
             ON CONFLICT (user_id, role_id) DO NOTHING",
        )
        .bind(user_id.to_string())
        .bind(role_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn unassign_role(&self, user_id: Uuid, role_id: Uuid) -> ServerResult<()> {
        sqlx::query("DELETE FROM member_roles WHERE user_id = ? AND role_id = ?")
            .bind(user_id.to_string())
            .bind(role_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn roles_for_user(&self, user_id: Uuid) -> ServerResult<Vec<RoleRow>> {
        let rows = sqlx::query(
            "SELECT r.id, r.name, r.permissions, r.position, r.is_default
             FROM roles r JOIN member_roles m ON m.role_id = r.id
             WHERE m.user_id = ? ORDER BY r.position, r.name",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(role_from_row).collect()
    }

    async fn member_permissions(&self, user_id: Uuid) -> ServerResult<i64> {
        // SQLite has no BIT_OR aggregate, so fold the bits in Rust.
        let rows = sqlx::query(
            "SELECT permissions FROM roles
             WHERE is_default = 1
                OR id IN (SELECT role_id FROM member_roles WHERE user_id = ?)",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let bits = rows
            .into_iter()
            .fold(0i64, |acc, r| acc | r.get::<i64, _>("permissions"));
        Ok(bits)
    }

    async fn upsert_backup(
        &self,
        user_id: Uuid,
        encrypted_blob: &[u8],
        salt: &[u8],
        argon2_params: &[u8],
        version: i32,
    ) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO key_backups (user_id, encrypted_blob, salt, argon2_params, version, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET
                encrypted_blob = excluded.encrypted_blob,
                salt = excluded.salt,
                argon2_params = excluded.argon2_params,
                version = excluded.version,
                updated_at = excluded.updated_at",
        )
        .bind(user_id.to_string())
        .bind(encrypted_blob)
        .bind(salt)
        .bind(argon2_params)
        .bind(version)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_backup(&self, user_id: Uuid) -> ServerResult<Option<BackupRow>> {
        let row = sqlx::query(
            "SELECT encrypted_blob, salt, argon2_params, version FROM key_backups WHERE user_id = ?",
        )
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| BackupRow {
            encrypted_blob: r.get("encrypted_blob"),
            salt: r.get("salt"),
            argon2_params: r.get("argon2_params"),
            version: r.get::<i64, _>("version") as i32,
        }))
    }

    async fn delete_backup(&self, user_id: Uuid) -> ServerResult<()> {
        sqlx::query("DELETE FROM key_backups WHERE user_id = ?")
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn put_vault_blob(&self, user_id: Uuid, name: &str, blob: &[u8]) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO vault_blobs (user_id, name, blob, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id, name) DO UPDATE SET blob = excluded.blob, updated_at = excluded.updated_at",
        )
        .bind(user_id.to_string())
        .bind(name)
        .bind(blob)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_vault_blob(&self, user_id: Uuid, name: &str) -> ServerResult<Option<Vec<u8>>> {
        let row = sqlx::query("SELECT blob FROM vault_blobs WHERE user_id = ? AND name = ?")
            .bind(user_id.to_string())
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<Vec<u8>, _>("blob")))
    }

    async fn list_vault_blobs(&self, user_id: Uuid, prefix: &str) -> ServerResult<Vec<String>> {
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        let rows = sqlx::query(
            "SELECT name FROM vault_blobs WHERE user_id = ? AND name LIKE ? ESCAPE '\\' ORDER BY name",
        )
        .bind(user_id.to_string())
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| r.get::<String, _>("name"))
            .collect())
    }

    async fn delete_vault_blob(&self, user_id: Uuid, name: &str) -> ServerResult<()> {
        sqlx::query("DELETE FROM vault_blobs WHERE user_id = ? AND name = ?")
            .bind(user_id.to_string())
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_device(&self, user_id: Uuid, name: &str) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO devices (id, user_id, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(user_id.to_string())
            .bind(name)
            .bind(Utc::now().timestamp_millis())
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    async fn find_device(&self, user_id: Uuid, name: &str) -> ServerResult<Option<Uuid>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM devices WHERE user_id = ? AND name = ? AND revoked_at IS NULL \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some((id,)) => Ok(Some(parse_id(id)?)),
        }
    }

    async fn revoke_device(&self, device_id: Uuid) -> ServerResult<()> {
        sqlx::query("UPDATE devices SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(Utc::now().timestamp_millis())
            .bind(device_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_device_credential(&self, device_id: Uuid, credential: &[u8]) -> ServerResult<()> {
        sqlx::query("UPDATE devices SET mls_credential = ? WHERE id = ?")
            .bind(credential)
            .bind(device_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn store_refresh_token(
        &self,
        token: &str,
        user_id: Uuid,
        device_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO refresh_tokens (token, user_id, device_id, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(token)
        .bind(user_id.to_string())
        .bind(device_id.to_string())
        .bind(expires_at.timestamp_millis())
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn lookup_refresh_token(&self, token: &str) -> ServerResult<Option<RefreshRow>> {
        let row = sqlx::query(
            "SELECT user_id, device_id, expires_at FROM refresh_tokens WHERE token = ?",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(r) => Ok(Some(RefreshRow {
                user_id: parse_id(r.get("user_id"))?,
                device_id: parse_id(r.get("device_id"))?,
                expires_at: ms_to_dt(r.get("expires_at")),
            })),
        }
    }

    async fn delete_refresh_token(&self, token: &str) -> ServerResult<()> {
        sqlx::query("DELETE FROM refresh_tokens WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_public_group(&self, name: &str, description: &str) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO groups (id, name, description, kind, created_at)
             VALUES (?, ?, ?, 'public', ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(description)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    async fn create_private_group_with_id(&self, id: Uuid, name: &str) -> ServerResult<()> {
        let result = sqlx::query(
            "INSERT INTO groups (id, name, description, kind, created_at)
             VALUES (?, ?, '', 'private', ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(ServerError::AlreadyExists(format!("group {id}")))
            }
            Err(e) => Err(ServerError::Database(e)),
        }
    }

    async fn add_member(&self, group_id: Uuid, user_id: Uuid, role: &str) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO group_members (group_id, user_id, role, joined_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (group_id, user_id) DO NOTHING",
        )
        .bind(group_id.to_string())
        .bind(user_id.to_string())
        .bind(role)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn is_member(&self, group_id: Uuid, user_id: Uuid) -> ServerResult<bool> {
        let row = sqlx::query("SELECT 1 FROM group_members WHERE group_id = ? AND user_id = ?")
            .bind(group_id.to_string())
            .bind(user_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn group_ids_for_user(&self, user_id: Uuid) -> ServerResult<Vec<Uuid>> {
        let rows = sqlx::query("SELECT group_id FROM group_members WHERE user_id = ?")
            .bind(user_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| parse_id(r.get("group_id")))
            .collect()
    }

    async fn list_groups_for_user(&self, user_id: Uuid) -> ServerResult<Vec<GroupSummaryRow>> {
        let rows = sqlx::query(
            "SELECT g.id, g.name, g.description, g.kind,
                    (SELECT count(*) FROM group_members m2 WHERE m2.group_id = g.id) AS member_count
             FROM groups g
             JOIN group_members m ON m.group_id = g.id
             WHERE m.user_id = ?
             ORDER BY g.name",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(GroupSummaryRow {
                    id: parse_id(r.get("id"))?,
                    name: r.get("name"),
                    description: r.get("description"),
                    kind: r.get("kind"),
                    member_count: r.get("member_count"),
                })
            })
            .collect()
    }

    async fn get_group(&self, group_id: Uuid) -> ServerResult<GroupSummaryRow> {
        let row = sqlx::query(
            "SELECT g.id, g.name, g.description, g.kind,
                    (SELECT count(*) FROM group_members m2 WHERE m2.group_id = g.id) AS member_count
             FROM groups g WHERE g.id = ?",
        )
        .bind(group_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ServerError::NotFound(format!("group {group_id}")))?;
        Ok(GroupSummaryRow {
            id: parse_id(row.get("id"))?,
            name: row.get("name"),
            description: row.get("description"),
            kind: row.get("kind"),
            member_count: row.get("member_count"),
        })
    }

    async fn member_ids(&self, group_id: Uuid) -> ServerResult<Vec<Uuid>> {
        let rows = sqlx::query("SELECT user_id FROM group_members WHERE group_id = ?")
            .bind(group_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| parse_id(r.get("user_id")))
            .collect()
    }

    async fn device_ids_for_group(&self, group_id: Uuid) -> ServerResult<Vec<Uuid>> {
        let rows = sqlx::query(
            "SELECT d.id FROM devices d \
             JOIN group_members gm ON gm.user_id = d.user_id \
             WHERE gm.group_id = ? AND d.revoked_at IS NULL",
        )
        .bind(group_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(|r| parse_id(r.get("id"))).collect()
    }

    async fn insert_public_message(
        &self,
        group_id: Uuid,
        sender_id: Uuid,
        content: &str,
        client_message_id: Uuid,
    ) -> ServerResult<PublicMessageRow> {
        let id = Uuid::now_v7();
        let now_ms = Utc::now().timestamp_millis();
        let row = sqlx::query(
            "INSERT INTO public_messages (id, group_id, sender_id, content, client_message_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (group_id, client_message_id)
               DO UPDATE SET content = public_messages.content
             RETURNING seq, created_at",
        )
        .bind(id.to_string())
        .bind(group_id.to_string())
        .bind(sender_id.to_string())
        .bind(content)
        .bind(client_message_id.to_string())
        .bind(now_ms)
        .fetch_one(&self.pool)
        .await?;
        let seq: i64 = row.get("seq");
        let created_at = ms_to_dt(row.get("created_at"));

        let name_row = sqlx::query("SELECT display_name FROM users WHERE id = ?")
            .bind(sender_id.to_string())
            .fetch_one(&self.pool)
            .await?;

        Ok(PublicMessageRow {
            id,
            group_id,
            sender_id,
            sender_display_name: name_row.get("display_name"),
            content: content.to_owned(),
            seq,
            created_at,
        })
    }

    async fn fetch_public_history(
        &self,
        group_id: Uuid,
        before_seq: i64,
        limit: i64,
    ) -> ServerResult<Vec<PublicMessageRow>> {
        let upper = if before_seq <= 0 {
            i64::MAX
        } else {
            before_seq
        };
        let rows = sqlx::query(
            "SELECT pm.id, pm.group_id, pm.sender_id, u.display_name AS sender_display_name,
                    pm.content, pm.seq, pm.created_at
             FROM public_messages pm
             JOIN users u ON u.id = pm.sender_id
             WHERE pm.group_id = ? AND pm.seq < ?
             ORDER BY pm.seq DESC LIMIT ?",
        )
        .bind(group_id.to_string())
        .bind(upper)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(PublicMessageRow {
                    id: parse_id(r.get("id"))?,
                    group_id: parse_id(r.get("group_id"))?,
                    sender_id: parse_id(r.get("sender_id"))?,
                    sender_display_name: r.get("sender_display_name"),
                    content: r.get("content"),
                    seq: r.get("seq"),
                    created_at: ms_to_dt(r.get("created_at")),
                })
            })
            .collect()
    }

    async fn store_key_packages(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        packages: &[Vec<u8>],
    ) -> ServerResult<u32> {
        let mut count = 0u32;
        for pkg in packages {
            sqlx::query(
                "INSERT INTO key_packages (id, user_id, device_id, key_package, consumed, created_at)
                 VALUES (?, ?, ?, ?, 0, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(user_id.to_string())
            .bind(device_id.to_string())
            .bind(pkg)
            .bind(Utc::now().timestamp_millis())
            .execute(&self.pool)
            .await?;
            count += 1;
        }
        // Cap unconsumed packages per device (clients re-publish on every login,
        // so without a bound the table grows forever). Keep the newest.
        sqlx::query(
            "DELETE FROM key_packages WHERE device_id = ? AND consumed = 0 AND id NOT IN \
             (SELECT id FROM key_packages WHERE device_id = ? AND consumed = 0 \
              ORDER BY id DESC LIMIT ?)",
        )
        .bind(device_id.to_string())
        .bind(device_id.to_string())
        .bind(MAX_UNCONSUMED_KEY_PACKAGES)
        .execute(&self.pool)
        .await?;
        Ok(count)
    }

    async fn claim_key_packages_for_user(
        &self,
        user_id: Uuid,
    ) -> ServerResult<Vec<ClaimedKeyPackage>> {
        let device_rows = sqlx::query(
            "SELECT DISTINCT device_id FROM key_packages WHERE user_id = ? AND consumed = 0",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let mut claimed = Vec::new();
        for dr in device_rows {
            let device_id_str: String = dr.get("device_id");
            // SQLite serializes writes, so a plain select-then-update is safe.
            let pkg_row = sqlx::query(
                "SELECT id, key_package FROM key_packages
                 WHERE device_id = ? AND consumed = 0 ORDER BY created_at LIMIT 1",
            )
            .bind(&device_id_str)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(pr) = pkg_row {
                let id: String = pr.get("id");
                sqlx::query("UPDATE key_packages SET consumed = 1 WHERE id = ?")
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                claimed.push(ClaimedKeyPackage {
                    device_id: parse_id(device_id_str)?,
                    key_package: pr.get("key_package"),
                });
            }
        }
        Ok(claimed)
    }

    async fn store_private_message(
        &self,
        group_id: Uuid,
        sender_id: Uuid,
        sender_device_id: Uuid,
        ciphertext: &[u8],
        epoch: i64,
        client_message_id: Uuid,
    ) -> ServerResult<PrivateMessageRow> {
        let id = Uuid::now_v7();
        let now_ms = Utc::now().timestamp_millis();
        let row = sqlx::query(
            "INSERT INTO private_messages
                 (id, group_id, sender_id, sender_device_id, ciphertext, epoch, client_message_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (group_id, client_message_id)
               DO UPDATE SET ciphertext = private_messages.ciphertext
             RETURNING seq, created_at",
        )
        .bind(id.to_string())
        .bind(group_id.to_string())
        .bind(sender_id.to_string())
        .bind(sender_device_id.to_string())
        .bind(ciphertext)
        .bind(epoch)
        .bind(client_message_id.to_string())
        .bind(now_ms)
        .fetch_one(&self.pool)
        .await?;

        Ok(PrivateMessageRow {
            id,
            group_id,
            sender_id,
            sender_device_id,
            ciphertext: ciphertext.to_vec(),
            epoch,
            seq: row.get("seq"),
            created_at: ms_to_dt(row.get("created_at")),
        })
    }

    async fn fetch_private_history(
        &self,
        group_id: Uuid,
        before_seq: i64,
        limit: i64,
    ) -> ServerResult<Vec<PrivateMessageRow>> {
        let upper = if before_seq <= 0 {
            i64::MAX
        } else {
            before_seq
        };
        let rows = sqlx::query(
            "SELECT id, group_id, sender_id, sender_device_id, ciphertext, epoch, seq, created_at
             FROM private_messages
             WHERE group_id = ? AND seq < ?
             ORDER BY seq DESC LIMIT ?",
        )
        .bind(group_id.to_string())
        .bind(upper)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(PrivateMessageRow {
                    id: parse_id(r.get("id"))?,
                    group_id: parse_id(r.get("group_id"))?,
                    sender_id: parse_id(r.get("sender_id"))?,
                    sender_device_id: parse_id(r.get("sender_device_id"))?,
                    ciphertext: r.get("ciphertext"),
                    epoch: r.get("epoch"),
                    seq: r.get("seq"),
                    created_at: ms_to_dt(r.get("created_at")),
                })
            })
            .collect()
    }

    async fn enqueue_inbox(
        &self,
        device_id: Uuid,
        kind: &str,
        group_id: Uuid,
        payload: &[u8],
    ) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO mls_inbox (id, device_id, kind, group_id, payload, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(device_id.to_string())
        .bind(kind)
        .bind(group_id.to_string())
        .bind(payload)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn drain_inbox(&self, device_id: Uuid) -> ServerResult<Vec<InboxRow>> {
        let rows = sqlx::query(
            "DELETE FROM mls_inbox WHERE device_id = ? RETURNING kind, group_id, payload",
        )
        .bind(device_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(InboxRow {
                    kind: r.get("kind"),
                    group_id: parse_id(r.get("group_id"))?,
                    payload: r.get("payload"),
                })
            })
            .collect()
    }

    async fn upsert_friend_request(
        &self,
        recipient: Uuid,
        sender_identity: &[u8],
        kind: &str,
        contact_code: &str,
    ) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO friend_requests \
             (id, recipient_user_id, sender_identity, kind, contact_code, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (recipient_user_id, sender_identity, kind) \
             DO UPDATE SET contact_code = excluded.contact_code",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(recipient.to_string())
        .bind(sender_identity)
        .bind(kind)
        .bind(contact_code)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_friend_requests(&self, recipient: Uuid) -> ServerResult<Vec<FriendRequestRow>> {
        let rows = sqlx::query(
            "SELECT id, kind, contact_code, created_at FROM friend_requests \
             WHERE recipient_user_id = ? ORDER BY created_at",
        )
        .bind(recipient.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| FriendRequestRow {
                id: r.get("id"),
                kind: r.get("kind"),
                contact_code: r.get("contact_code"),
                created_at_ms: r.get("created_at"),
            })
            .collect())
    }

    async fn delete_friend_request(&self, recipient: Uuid, id: &str) -> ServerResult<()> {
        sqlx::query("DELETE FROM friend_requests WHERE id = ? AND recipient_user_id = ?")
            .bind(id)
            .bind(recipient.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn count_friend_requests(&self, recipient: Uuid) -> ServerResult<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM friend_requests WHERE recipient_user_id = ?")
                .bind(recipient.to_string())
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh on-disk store in a temp file (`:memory:` would give each pool
    /// connection its own empty database).
    async fn temp_store() -> (SqliteStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("accord-test-{}.db", Uuid::now_v7()));
        let url = format!("sqlite:{}", path.to_string_lossy().replace('\\', "/"));
        let store = SqliteStore::connect(&url).await.expect("connect");
        (store, path)
    }

    /// A checksum that differs from the embedded one only by line endings must
    /// be repaired in place, so `migrate()` succeeds where stock sqlx fails
    /// with "previously applied but has been modified".
    #[tokio::test]
    async fn migrate_self_heals_line_ending_checksums() {
        let (store, path) = temp_store().await;
        store.migrate().await.expect("initial migrate");

        // Re-stamp the first migration as if it had been applied by a binary
        // built from a checkout with the opposite line endings.
        let first = MIGRATOR.iter().next().expect("at least one migration");
        let variants = crate::store::line_ending_variant_checksums(&first.sql);
        let other = variants
            .iter()
            .find(|v| v.as_slice() != first.checksum.as_ref())
            .expect("sql contains newlines, so the variants differ");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
            .bind(other.as_slice())
            .bind(first.version)
            .execute(&store.pool)
            .await
            .expect("stamp variant checksum");

        store.migrate().await.expect("self-healing migrate");

        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(first.version)
                .fetch_one(&store.pool)
                .await
                .expect("read checksum");
        assert_eq!(stored.as_slice(), first.checksum.as_ref());

        drop(store);
        let _ = std::fs::remove_file(path);
    }

    /// A genuinely different checksum (a real edit) must still fail loudly -
    /// migrations are immutable.
    #[tokio::test]
    async fn migrate_still_rejects_real_modifications() {
        let (store, path) = temp_store().await;
        store.migrate().await.expect("initial migrate");

        let first = MIGRATOR.iter().next().expect("at least one migration");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
            .bind(vec![0u8; 48])
            .bind(first.version)
            .execute(&store.pool)
            .await
            .expect("stamp bogus checksum");

        let err = store.migrate().await.expect_err("must reject");
        assert!(
            err.to_string().contains("modified"),
            "unexpected error: {err}"
        );

        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
