//! `BackupService` implementation: store and serve opaque encrypted key backups.

use std::sync::Arc;

use accord_proto::backup_service_server::BackupService;
use accord_proto::{
    DeleteKeyBackupRequest, DeleteKeyBackupResponse, DownloadKeyBackupRequest,
    DownloadKeyBackupResponse, UpdateKeyBackupRequest, UpdateKeyBackupResponse,
    UploadKeyBackupRequest, UploadKeyBackupResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::error::ServerError;
use crate::store::Store;
use crate::util::authenticate;

/// Implements the `BackupService` RPCs.
#[derive(Debug)]
pub struct BackupSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
}

impl BackupSvc {
    /// Construct the service.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys) -> Self {
        Self { store, jwt }
    }

    /// Authenticate and return the caller's user id.
    fn caller<T>(&self, request: &Request<T>) -> Result<Uuid, ServerError> {
        let claims = authenticate(request, &self.jwt)?;
        Uuid::parse_str(&claims.sub)
            .map_err(|_| ServerError::InvalidArgument("invalid user id in token".into()))
    }

    /// Shared insert-or-replace path for upload and update.
    async fn store_backup(
        &self,
        user_id: Uuid,
        blob: &[u8],
        salt: &[u8],
        params: &[u8],
        version: u32,
    ) -> Result<(), ServerError> {
        if blob.is_empty() || salt.is_empty() || params.is_empty() {
            return Err(ServerError::InvalidArgument("incomplete backup".into()));
        }
        self.store
            .upsert_backup(user_id, blob, salt, params, version as i32)
            .await
    }
}

#[tonic::async_trait]
impl BackupService for BackupSvc {
    async fn upload_key_backup(
        &self,
        request: Request<UploadKeyBackupRequest>,
    ) -> Result<Response<UploadKeyBackupResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        self.store_backup(
            user_id,
            &req.encrypted_blob,
            &req.salt,
            &req.argon2_params,
            req.version,
        )
        .await?;
        Ok(Response::new(UploadKeyBackupResponse {}))
    }

    async fn download_key_backup(
        &self,
        request: Request<DownloadKeyBackupRequest>,
    ) -> Result<Response<DownloadKeyBackupResponse>, Status> {
        let user_id = self.caller(&request)?;
        let row = self
            .store
            .get_backup(user_id)
            .await?
            .ok_or(ServerError::NotFound("key backup".into()))?;
        Ok(Response::new(DownloadKeyBackupResponse {
            encrypted_blob: row.encrypted_blob,
            salt: row.salt,
            argon2_params: row.argon2_params,
            version: row.version as u32,
        }))
    }

    async fn update_key_backup(
        &self,
        request: Request<UpdateKeyBackupRequest>,
    ) -> Result<Response<UpdateKeyBackupResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        self.store_backup(
            user_id,
            &req.encrypted_blob,
            &req.salt,
            &req.argon2_params,
            req.version,
        )
        .await?;
        Ok(Response::new(UpdateKeyBackupResponse {}))
    }

    async fn delete_key_backup(
        &self,
        request: Request<DeleteKeyBackupRequest>,
    ) -> Result<Response<DeleteKeyBackupResponse>, Status> {
        let user_id = self.caller(&request)?;
        self.store.delete_backup(user_id).await?;
        Ok(Response::new(DeleteKeyBackupResponse {}))
    }
}
