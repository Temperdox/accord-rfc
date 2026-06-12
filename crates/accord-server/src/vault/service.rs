//! `VaultService` implementation: store and serve opaque per-account blobs.

use std::sync::Arc;

use accord_proto::vault_service_server::VaultService;
use accord_proto::{
    DeleteBlobRequest, DeleteBlobResponse, GetBlobRequest, GetBlobResponse, ListBlobsRequest,
    ListBlobsResponse, PutBlobRequest, PutBlobResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::error::ServerError;
use crate::store::Store;
use crate::util::authenticate;

/// Maximum blob name length (a guard, not a meaningful limit).
const MAX_NAME_LEN: usize = 256;

/// Implements the `VaultService` RPCs.
#[derive(Debug)]
pub struct VaultSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
}

impl VaultSvc {
    /// Construct the service.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys) -> Self {
        Self { store, jwt }
    }

    fn caller<T>(&self, request: &Request<T>) -> Result<Uuid, ServerError> {
        let claims = authenticate(request, &self.jwt)?;
        Uuid::parse_str(&claims.sub)
            .map_err(|_| ServerError::InvalidArgument("invalid user id in token".into()))
    }
}

fn check_name(name: &str) -> Result<(), ServerError> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(ServerError::InvalidArgument("invalid blob name".into()));
    }
    Ok(())
}

#[tonic::async_trait]
impl VaultService for VaultSvc {
    async fn put_blob(
        &self,
        request: Request<PutBlobRequest>,
    ) -> Result<Response<PutBlobResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        check_name(&req.name)?;
        if req.blob.is_empty() {
            return Err(ServerError::InvalidArgument("empty blob".into()).into());
        }
        self.store
            .put_vault_blob(user_id, &req.name, &req.blob)
            .await?;
        Ok(Response::new(PutBlobResponse {}))
    }

    async fn get_blob(
        &self,
        request: Request<GetBlobRequest>,
    ) -> Result<Response<GetBlobResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        let blob = self
            .store
            .get_vault_blob(user_id, &req.name)
            .await?
            .ok_or(ServerError::NotFound("vault blob".into()))?;
        Ok(Response::new(GetBlobResponse { blob }))
    }

    async fn list_blobs(
        &self,
        request: Request<ListBlobsRequest>,
    ) -> Result<Response<ListBlobsResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        let names = self.store.list_vault_blobs(user_id, &req.prefix).await?;
        Ok(Response::new(ListBlobsResponse { names }))
    }

    async fn delete_blob(
        &self,
        request: Request<DeleteBlobRequest>,
    ) -> Result<Response<DeleteBlobResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        self.store.delete_vault_blob(user_id, &req.name).await?;
        Ok(Response::new(DeleteBlobResponse {}))
    }
}
