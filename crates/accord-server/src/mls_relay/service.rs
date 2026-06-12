//! `MlsService` implementation - the opaque relay.

use std::collections::HashMap;
use std::sync::Arc;

use accord_proto::mls_service_server::MlsService;
use accord_proto::{
    DeviceId, DeviceKeyPackage, FetchKeyPackagesRequest, FetchKeyPackagesResponse,
    KeyPackageBundle, SendCommitRequest, SendCommitResponse, SendWelcomeRequest,
    SendWelcomeResponse, UploadKeyPackagesRequest, UploadKeyPackagesResponse, WelcomeTarget,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::error::ServerError;
use crate::messaging::Hub;
use crate::store::Store;
use crate::util::authenticate;

/// Implements the `MlsService` RPCs. Holds the [`Store`] (for KeyPackage storage
/// + the offline inbox) and the [`Hub`] (for live relay).
#[derive(Debug)]
pub struct MlsRelaySvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
    hub: Arc<Hub>,
}

impl MlsRelaySvc {
    /// Construct the service.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys, hub: Arc<Hub>) -> Self {
        Self { store, jwt, hub }
    }

    /// Deliver a Welcome to a device: queue it for durability AND push it live.
    /// The client is idempotent (ignores a Welcome for a group it already has),
    /// so the belt-and-suspenders double path is safe.
    async fn relay_welcome(
        &self,
        target: &WelcomeTarget,
        group_id: Uuid,
    ) -> Result<(), ServerError> {
        let device_id = target
            .device_id
            .as_ref()
            .ok_or_else(|| ServerError::InvalidArgument("welcome target needs device_id".into()))?;
        let device = Uuid::parse_str(&device_id.value)
            .map_err(|_| ServerError::InvalidArgument("bad device id".into()))?;

        self.store
            .enqueue_inbox(device, "welcome", group_id, &target.welcome)
            .await?;
        self.hub
            .publish_welcome(device, group_id, target.welcome.clone())
            .await?;
        // Subscribe the welcomed device so it receives live group traffic.
        self.hub.subscribe(device, group_id);
        Ok(())
    }
}

#[tonic::async_trait]
impl MlsService for MlsRelaySvc {
    async fn upload_key_packages(
        &self,
        request: Request<UploadKeyPackagesRequest>,
    ) -> Result<Response<UploadKeyPackagesResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;
        let device_id = parse_uuid(&claims.device_id)?;
        let req = request.into_inner();

        let stored = self
            .store
            .store_key_packages(user_id, device_id, &req.key_packages)
            .await?;
        Ok(Response::new(UploadKeyPackagesResponse {
            stored_count: stored,
        }))
    }

    async fn fetch_key_packages(
        &self,
        request: Request<FetchKeyPackagesRequest>,
    ) -> Result<Response<FetchKeyPackagesResponse>, Status> {
        let _claims = authenticate(&request, &self.jwt)?;
        let req = request.into_inner();

        let mut packages: HashMap<String, KeyPackageBundle> = HashMap::new();
        for user in &req.user_ids {
            let uid = Uuid::parse_str(&user.value)
                .map_err(|_| ServerError::InvalidArgument("bad user id".into()))?;
            let claimed = self.store.claim_key_packages_for_user(uid).await?;
            if claimed.is_empty() {
                continue;
            }
            let device_packages = claimed
                .into_iter()
                .map(|c| DeviceKeyPackage {
                    device_id: Some(DeviceId {
                        value: c.device_id.to_string(),
                    }),
                    key_package: c.key_package,
                })
                .collect();
            packages.insert(user.value.clone(), KeyPackageBundle { device_packages });
        }
        Ok(Response::new(FetchKeyPackagesResponse { packages }))
    }

    async fn send_commit(
        &self,
        request: Request<SendCommitRequest>,
    ) -> Result<Response<SendCommitResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;
        let device_id = parse_uuid(&claims.device_id)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        if !self.store.is_member(group_id, user_id).await? {
            return Err(ServerError::PermissionDenied.into());
        }

        // Relay the Commit to existing members (epoch is informational here).
        self.hub
            .publish_commit(group_id, req.commit, 0, device_id)
            .await
            .map_err(ServerError::from)?;

        // Deliver any accompanying Welcomes to the newly-added devices.
        for welcome in &req.welcomes {
            self.relay_welcome(welcome, group_id).await?;
        }

        Ok(Response::new(SendCommitResponse { epoch: 0 }))
    }

    async fn send_welcome(
        &self,
        request: Request<SendWelcomeRequest>,
    ) -> Result<Response<SendWelcomeResponse>, Status> {
        let _claims = authenticate(&request, &self.jwt)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        for welcome in &req.welcomes {
            self.relay_welcome(welcome, group_id).await?;
        }
        Ok(Response::new(SendWelcomeResponse {}))
    }
}

fn require_group_id(group_id: &Option<accord_proto::GroupId>) -> Result<Uuid, ServerError> {
    let value = group_id
        .as_ref()
        .ok_or_else(|| ServerError::InvalidArgument("group_id is required".into()))?;
    Uuid::parse_str(&value.value)
        .map_err(|_| ServerError::InvalidArgument("group_id is not a valid UUID".into()))
}

fn parse_uuid(s: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(s).map_err(|_| ServerError::InvalidArgument("invalid id in token".into()))
}
