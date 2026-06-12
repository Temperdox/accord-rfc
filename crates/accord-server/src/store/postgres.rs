//! PostgreSQL implementation of [`Store`] - for large "platform" deployments.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::error::{ServerError, ServerResult};
use crate::store::model::*;
use crate::store::{MAX_UNCONSUMED_KEY_PACKAGES, Store};

/// Embedded Postgres migrations (run at startup).
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

/// A [`Store`] backed by a PostgreSQL connection pool.
#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Open the pool.
    ///
    /// # Errors
    /// Returns [`ServerError::Database`] if the pool cannot connect.
    pub async fn connect(database_url: &str, max_connections: u32) -> ServerResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Apply pending migrations.
    ///
    /// # Errors
    /// Returns [`ServerError`] if a migration fails.
    pub async fn migrate(&self) -> ServerResult<()> {
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|e| ServerError::Migration(e.to_string()))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for PostgresStore {
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
            "INSERT INTO users (id, username, display_name, password_hash, is_owner, is_guest, identity_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(username)
        .bind(display_name)
        .bind(password_hash)
        .bind(is_owner)
        .bind(is_guest)
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
        let (exists,): (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM users WHERE identity_key = $1)")
                .bind(identity_pubkey)
                .fetch_one(&self.pool)
                .await?;
        Ok(exists)
    }

    async fn find_user_by_username(&self, username: &str) -> ServerResult<Option<UserRow>> {
        Ok(sqlx::query_as::<_, UserRow>(
            "SELECT id, username, display_name, password_hash, is_guest \
             FROM users WHERE username = $1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn is_user_guest(&self, user_id: Uuid) -> ServerResult<bool> {
        let row: Option<(bool,)> = sqlx::query_as("SELECT is_guest FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some_and(|(g,)| g))
    }

    async fn count_users(&self) -> ServerResult<i64> {
        let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    async fn is_owner(&self, user_id: Uuid) -> ServerResult<bool> {
        let row: Option<(bool,)> = sqlx::query_as("SELECT is_owner FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(o,)| o).unwrap_or(false))
    }

    async fn create_invite(&self, token: &str, created_by: Uuid) -> ServerResult<()> {
        sqlx::query("INSERT INTO invites (token, created_by) VALUES ($1, $2)")
            .bind(token)
            .bind(created_by)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn is_invite_valid(&self, token: &str) -> ServerResult<bool> {
        let row: Option<(bool,)> = sqlx::query_as("SELECT revoked FROM invites WHERE token = $1")
            .bind(token)
            .fetch_optional(&self.pool)
            .await?;
        Ok(matches!(row, Some((false,))))
    }

    async fn revoke_invite(&self, token: &str) -> ServerResult<()> {
        sqlx::query("UPDATE invites SET revoked = TRUE WHERE token = $1")
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
            "INSERT INTO roles (id, name, permissions, position, is_default)
             VALUES ($1, $2, $3, 0, TRUE) ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(name)
        .bind(permissions)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_role(&self, name: &str, permissions: i64) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO roles (id, name, permissions) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(name)
            .bind(permissions)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    async fn list_roles(&self) -> ServerResult<Vec<RoleRow>> {
        Ok(sqlx::query_as::<_, RoleRow>(
            "SELECT id, name, permissions, position, is_default FROM roles ORDER BY position, name",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    async fn get_role(&self, id: Uuid) -> ServerResult<Option<RoleRow>> {
        Ok(sqlx::query_as::<_, RoleRow>(
            "SELECT id, name, permissions, position, is_default FROM roles WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn update_role(&self, id: Uuid, name: &str, permissions: i64) -> ServerResult<()> {
        sqlx::query("UPDATE roles SET name = $2, permissions = $3 WHERE id = $1")
            .bind(id)
            .bind(name)
            .bind(permissions)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_role(&self, id: Uuid) -> ServerResult<()> {
        sqlx::query("DELETE FROM roles WHERE id = $1 AND is_default = FALSE")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn assign_role(&self, user_id: Uuid, role_id: Uuid) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO member_roles (user_id, role_id) VALUES ($1, $2)
             ON CONFLICT (user_id, role_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(role_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn unassign_role(&self, user_id: Uuid, role_id: Uuid) -> ServerResult<()> {
        sqlx::query("DELETE FROM member_roles WHERE user_id = $1 AND role_id = $2")
            .bind(user_id)
            .bind(role_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn roles_for_user(&self, user_id: Uuid) -> ServerResult<Vec<RoleRow>> {
        Ok(sqlx::query_as::<_, RoleRow>(
            "SELECT r.id, r.name, r.permissions, r.position, r.is_default
             FROM roles r JOIN member_roles m ON m.role_id = r.id
             WHERE m.user_id = $1 ORDER BY r.position, r.name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn member_permissions(&self, user_id: Uuid) -> ServerResult<i64> {
        // BIT_OR across the default role + the user's assigned roles.
        let (bits,): (Option<i64>,) = sqlx::query_as(
            "SELECT BIT_OR(permissions) FROM roles
             WHERE is_default = TRUE
                OR id IN (SELECT role_id FROM member_roles WHERE user_id = $1)",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(bits.unwrap_or(0))
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
            "INSERT INTO key_backups (user_id, encrypted_blob, salt, argon2_params, version)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (user_id) DO UPDATE SET
                encrypted_blob = EXCLUDED.encrypted_blob,
                salt = EXCLUDED.salt,
                argon2_params = EXCLUDED.argon2_params,
                version = EXCLUDED.version,
                updated_at = now()",
        )
        .bind(user_id)
        .bind(encrypted_blob)
        .bind(salt)
        .bind(argon2_params)
        .bind(version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_backup(&self, user_id: Uuid) -> ServerResult<Option<BackupRow>> {
        let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>, i32)> = sqlx::query_as(
            "SELECT encrypted_blob, salt, argon2_params, version FROM key_backups WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(
            row.map(|(encrypted_blob, salt, argon2_params, version)| BackupRow {
                encrypted_blob,
                salt,
                argon2_params,
                version,
            }),
        )
    }

    async fn delete_backup(&self, user_id: Uuid) -> ServerResult<()> {
        sqlx::query("DELETE FROM key_backups WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn put_vault_blob(&self, user_id: Uuid, name: &str, blob: &[u8]) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO vault_blobs (user_id, name, blob) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, name) DO UPDATE SET blob = EXCLUDED.blob, updated_at = now()",
        )
        .bind(user_id)
        .bind(name)
        .bind(blob)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_vault_blob(&self, user_id: Uuid, name: &str) -> ServerResult<Option<Vec<u8>>> {
        let row: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT blob FROM vault_blobs WHERE user_id = $1 AND name = $2")
                .bind(user_id)
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(blob,)| blob))
    }

    async fn list_vault_blobs(&self, user_id: Uuid, prefix: &str) -> ServerResult<Vec<String>> {
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM vault_blobs WHERE user_id = $1 AND name LIKE $2 ORDER BY name",
        )
        .bind(user_id)
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }

    async fn delete_vault_blob(&self, user_id: Uuid, name: &str) -> ServerResult<()> {
        sqlx::query("DELETE FROM vault_blobs WHERE user_id = $1 AND name = $2")
            .bind(user_id)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_device(&self, user_id: Uuid, name: &str) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO devices (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(user_id)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    async fn find_device(&self, user_id: Uuid, name: &str) -> ServerResult<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM devices WHERE user_id = $1 AND name = $2 AND revoked_at IS NULL \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    async fn revoke_device(&self, device_id: Uuid) -> ServerResult<()> {
        sqlx::query("UPDATE devices SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(device_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_device_credential(&self, device_id: Uuid, credential: &[u8]) -> ServerResult<()> {
        sqlx::query("UPDATE devices SET mls_credential = $1 WHERE id = $2")
            .bind(credential)
            .bind(device_id)
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
            "INSERT INTO refresh_tokens (token, user_id, device_id, expires_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(token)
        .bind(user_id)
        .bind(device_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn lookup_refresh_token(&self, token: &str) -> ServerResult<Option<RefreshRow>> {
        Ok(sqlx::query_as::<_, RefreshRow>(
            "SELECT user_id, device_id, expires_at FROM refresh_tokens WHERE token = $1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn delete_refresh_token(&self, token: &str) -> ServerResult<()> {
        sqlx::query("DELETE FROM refresh_tokens WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_public_group(&self, name: &str, description: &str) -> ServerResult<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO groups (id, name, description, kind) VALUES ($1, $2, $3, 'public')",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    async fn create_private_group_with_id(&self, id: Uuid, name: &str) -> ServerResult<()> {
        let result = sqlx::query(
            "INSERT INTO groups (id, name, description, kind) VALUES ($1, $2, '', 'private')",
        )
        .bind(id)
        .bind(name)
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
            "INSERT INTO group_members (group_id, user_id, role) VALUES ($1, $2, $3)
             ON CONFLICT (group_id, user_id) DO NOTHING",
        )
        .bind(group_id)
        .bind(user_id)
        .bind(role)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn is_member(&self, group_id: Uuid, user_id: Uuid) -> ServerResult<bool> {
        let exists: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM group_members WHERE group_id = $1 AND user_id = $2")
                .bind(group_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(exists.is_some())
    }

    async fn group_ids_for_user(&self, user_id: Uuid) -> ServerResult<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT group_id FROM group_members WHERE user_id = $1")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn list_groups_for_user(&self, user_id: Uuid) -> ServerResult<Vec<GroupSummaryRow>> {
        Ok(sqlx::query_as::<_, GroupSummaryRow>(
            "SELECT g.id, g.name, g.description, g.kind,
                    (SELECT count(*) FROM group_members m2 WHERE m2.group_id = g.id) AS member_count
             FROM groups g
             JOIN group_members m ON m.group_id = g.id
             WHERE m.user_id = $1
             ORDER BY g.name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn get_group(&self, group_id: Uuid) -> ServerResult<GroupSummaryRow> {
        sqlx::query_as::<_, GroupSummaryRow>(
            "SELECT g.id, g.name, g.description, g.kind,
                    (SELECT count(*) FROM group_members m2 WHERE m2.group_id = g.id) AS member_count
             FROM groups g WHERE g.id = $1",
        )
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ServerError::NotFound(format!("group {group_id}")))
    }

    async fn member_ids(&self, group_id: Uuid) -> ServerResult<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM group_members WHERE group_id = $1")
                .bind(group_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn device_ids_for_group(&self, group_id: Uuid) -> ServerResult<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT d.id FROM devices d \
             JOIN group_members gm ON gm.user_id = d.user_id \
             WHERE gm.group_id = $1 AND d.revoked_at IS NULL",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn insert_public_message(
        &self,
        group_id: Uuid,
        sender_id: Uuid,
        content: &str,
        client_message_id: Uuid,
    ) -> ServerResult<PublicMessageRow> {
        let id = Uuid::now_v7();
        let (stored_id, seq, created_at) = sqlx::query_as::<_, (Uuid, i64, DateTime<Utc>)>(
            "INSERT INTO public_messages (id, group_id, sender_id, content, client_message_id)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (group_id, client_message_id)
               DO UPDATE SET content = public_messages.content
             RETURNING id, seq, created_at",
        )
        .bind(id)
        .bind(group_id)
        .bind(sender_id)
        .bind(content)
        .bind(client_message_id)
        .fetch_one(&self.pool)
        .await?;

        let (sender_display_name,): (String,) =
            sqlx::query_as("SELECT display_name FROM users WHERE id = $1")
                .bind(sender_id)
                .fetch_one(&self.pool)
                .await?;

        Ok(PublicMessageRow {
            id: stored_id,
            group_id,
            sender_id,
            sender_display_name,
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
        Ok(sqlx::query_as::<_, PublicMessageRow>(
            "SELECT pm.id, pm.group_id, pm.sender_id, u.display_name AS sender_display_name,
                    pm.content, pm.seq, pm.created_at
             FROM public_messages pm
             JOIN users u ON u.id = pm.sender_id
             WHERE pm.group_id = $1 AND pm.seq < $2
             ORDER BY pm.seq DESC
             LIMIT $3",
        )
        .bind(group_id)
        .bind(upper)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
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
                "INSERT INTO key_packages (id, user_id, device_id, key_package)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(device_id)
            .bind(pkg)
            .execute(&self.pool)
            .await?;
            count += 1;
        }
        // Cap unconsumed packages per device (clients re-publish on every login,
        // so without a bound the table grows forever). Keep the newest.
        sqlx::query(
            "DELETE FROM key_packages WHERE device_id = $1 AND consumed = FALSE AND id NOT IN \
             (SELECT id FROM key_packages WHERE device_id = $1 AND consumed = FALSE \
              ORDER BY id DESC LIMIT $2)",
        )
        .bind(device_id)
        .bind(MAX_UNCONSUMED_KEY_PACKAGES)
        .execute(&self.pool)
        .await?;
        Ok(count)
    }

    async fn claim_key_packages_for_user(
        &self,
        user_id: Uuid,
    ) -> ServerResult<Vec<ClaimedKeyPackage>> {
        let device_ids: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT device_id FROM key_packages WHERE user_id = $1 AND consumed = FALSE",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut claimed = Vec::new();
        for (device_id,) in device_ids {
            let row: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
                "UPDATE key_packages SET consumed = TRUE
                 WHERE id = (
                     SELECT id FROM key_packages
                     WHERE device_id = $1 AND consumed = FALSE
                     ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED
                 )
                 RETURNING id, key_package",
            )
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await?;
            if let Some((_id, key_package)) = row {
                claimed.push(ClaimedKeyPackage {
                    device_id,
                    key_package,
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
        let (stored_id, seq, created_at) = sqlx::query_as::<_, (Uuid, i64, DateTime<Utc>)>(
            "INSERT INTO private_messages
                 (id, group_id, sender_id, sender_device_id, ciphertext, epoch, client_message_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (group_id, client_message_id)
               DO UPDATE SET ciphertext = private_messages.ciphertext
             RETURNING id, seq, created_at",
        )
        .bind(id)
        .bind(group_id)
        .bind(sender_id)
        .bind(sender_device_id)
        .bind(ciphertext)
        .bind(epoch)
        .bind(client_message_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(PrivateMessageRow {
            id: stored_id,
            group_id,
            sender_id,
            sender_device_id,
            ciphertext: ciphertext.to_vec(),
            epoch,
            seq,
            created_at,
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
        Ok(sqlx::query_as::<_, PrivateMessageRow>(
            "SELECT id, group_id, sender_id, sender_device_id, ciphertext, epoch, seq, created_at
             FROM private_messages
             WHERE group_id = $1 AND seq < $2
             ORDER BY seq DESC LIMIT $3",
        )
        .bind(group_id)
        .bind(upper)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn enqueue_inbox(
        &self,
        device_id: Uuid,
        kind: &str,
        group_id: Uuid,
        payload: &[u8],
    ) -> ServerResult<()> {
        sqlx::query(
            "INSERT INTO mls_inbox (id, device_id, kind, group_id, payload)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(device_id)
        .bind(kind)
        .bind(group_id)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn drain_inbox(&self, device_id: Uuid) -> ServerResult<Vec<InboxRow>> {
        Ok(sqlx::query_as::<_, InboxRow>(
            "DELETE FROM mls_inbox WHERE device_id = $1 RETURNING kind, group_id, payload",
        )
        .bind(device_id)
        .fetch_all(&self.pool)
        .await?)
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
             (id, recipient_user_id, sender_identity, kind, contact_code) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (recipient_user_id, sender_identity, kind) \
             DO UPDATE SET contact_code = EXCLUDED.contact_code",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(recipient)
        .bind(sender_identity)
        .bind(kind)
        .bind(contact_code)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_friend_requests(&self, recipient: Uuid) -> ServerResult<Vec<FriendRequestRow>> {
        let rows: Vec<(String, String, String, chrono::DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, kind, contact_code, created_at FROM friend_requests \
             WHERE recipient_user_id = $1 ORDER BY created_at",
        )
        .bind(recipient)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, kind, contact_code, created_at)| FriendRequestRow {
                id,
                kind,
                contact_code,
                created_at_ms: created_at.timestamp_millis(),
            })
            .collect())
    }

    async fn delete_friend_request(&self, recipient: Uuid, id: &str) -> ServerResult<()> {
        sqlx::query("DELETE FROM friend_requests WHERE id = $1 AND recipient_user_id = $2")
            .bind(id)
            .bind(recipient)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn count_friend_requests(&self, recipient: Uuid) -> ServerResult<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM friend_requests WHERE recipient_user_id = $1")
                .bind(recipient)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }
}
