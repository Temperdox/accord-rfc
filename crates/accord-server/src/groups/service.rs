//! `GroupService` implementation.
//!
//! For the walking skeleton, only **public** channels are fully functional:
//! create, list, info, and open self-join via `AddMembers`. Private (MLS) group
//! creation and membership-by-Commit arrive in the private-chat phase.

use std::sync::Arc;

use accord_proto::group_service_server::GroupService;
use accord_proto::{
    AddMembersRequest, AddMembersResponse, ChatKind, CreateGroupResponse,
    CreatePrivateGroupRequest, CreatePublicGroupRequest, GetGroupInfoRequest, GetGroupInfoResponse,
    GroupId, GroupSummary, ListGroupsRequest, ListGroupsResponse, RemoveMembersRequest,
    RemoveMembersResponse, UserId,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::error::ServerError;
use crate::messaging::Hub;
use crate::store::Store;
use crate::store::model::GroupSummaryRow;
use crate::util::authenticate;

/// Implements the `GroupService` RPCs.
#[derive(Debug)]
pub struct GroupSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
    hub: Arc<Hub>,
}

impl GroupSvc {
    /// Construct the service. The [`Hub`] is used to relay MLS Welcomes when a
    /// private group is created.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys, hub: Arc<Hub>) -> Self {
        Self { store, jwt, hub }
    }
}

#[tonic::async_trait]
impl GroupService for GroupSvc {
    async fn create_public_group(
        &self,
        request: Request<CreatePublicGroupRequest>,
    ) -> Result<Response<CreateGroupResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;
        let req = request.into_inner();

        let name = req.name.trim();
        if name.is_empty() {
            return Err(ServerError::InvalidArgument("group name is required".into()).into());
        }

        let group_id = self
            .store
            .create_public_group(name, req.description.trim())
            .await?;
        // The creator owns the channel.
        self.store.add_member(group_id, user_id, "owner").await?;

        tracing::info!(%group_id, name, "created public group");
        Ok(Response::new(CreateGroupResponse {
            group_id: Some(GroupId {
                value: group_id.to_string(),
            }),
        }))
    }

    async fn create_private_group(
        &self,
        request: Request<CreatePrivateGroupRequest>,
    ) -> Result<Response<CreateGroupResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let creator = parse_uuid(&claims.sub)?;
        let creator_device = parse_uuid(&claims.device_id)?;
        let req = request.into_inner();

        // The client supplies the group id (it created the MLS group locally to
        // produce the Welcomes). The server adopts it verbatim.
        let group_id = require_group_id(&req.group_id)?;
        let name = if req.name.trim().is_empty() {
            "Direct message"
        } else {
            req.name.trim()
        };

        self.store
            .create_private_group_with_id(group_id, name)
            .await?;
        self.store.add_member(group_id, creator, "owner").await?;
        for member in &req.member_ids {
            let uid = Uuid::parse_str(&member.value).map_err(|_| {
                ServerError::InvalidArgument("member id is not a valid UUID".into())
            })?;
            self.store.add_member(group_id, uid, "member").await?;
        }

        // Relay each Welcome to its target device (queue for durability + push
        // live; the client ignores Welcomes for groups it already has).
        for welcome in &req.welcomes {
            let device_id = welcome.device_id.as_ref().ok_or_else(|| {
                ServerError::InvalidArgument("welcome target needs device_id".into())
            })?;
            let device = Uuid::parse_str(&device_id.value)
                .map_err(|_| ServerError::InvalidArgument("bad device id".into()))?;
            self.store
                .enqueue_inbox(device, "welcome", group_id, &welcome.welcome)
                .await?;
            self.hub
                .publish_welcome(device, group_id, welcome.welcome.clone())
                .await
                .map_err(ServerError::from)?;
            // Subscribe the newly-welcomed device so it gets live messages.
            self.hub.subscribe(device, group_id);
        }

        // Subscribe the creator's device too (it created the group post-connect).
        self.hub.subscribe(creator_device, group_id);

        tracing::info!(%group_id, "created private group");
        Ok(Response::new(CreateGroupResponse {
            group_id: Some(GroupId {
                value: group_id.to_string(),
            }),
        }))
    }

    async fn add_members(
        &self,
        request: Request<AddMembersRequest>,
    ) -> Result<Response<AddMembersResponse>, Status> {
        // Public channels are open-join for members: any authenticated member may
        // add members (including themselves) to a public group. Guests (open_dms
        // DM-only accounts) may not - otherwise "open a DM" would quietly grant
        // channel access on a private server. Private-group membership is driven
        // by MLS Commits in a later phase.
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        if self.store.is_user_guest(caller).await? {
            return Err(ServerError::PermissionDenied.into());
        }
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        let group = self.store.get_group(group_id).await?;
        if group.kind != "public" {
            return Err(Status::unimplemented(
                "adding members to private groups requires an MLS Commit",
            ));
        }

        // A guest cannot be added to channels either (by themselves or others).
        for member in &req.member_ids {
            let uid = Uuid::parse_str(&member.value).map_err(|_| {
                ServerError::InvalidArgument("member id is not a valid UUID".into())
            })?;
            if self.store.is_user_guest(uid).await? {
                return Err(ServerError::PermissionDenied.into());
            }
            self.store.add_member(group_id, uid, "member").await?;
        }
        Ok(Response::new(AddMembersResponse {}))
    }

    async fn remove_members(
        &self,
        _request: Request<RemoveMembersRequest>,
    ) -> Result<Response<RemoveMembersResponse>, Status> {
        Err(Status::unimplemented(
            "member removal arrives with the moderation/MLS phase",
        ))
    }

    async fn list_groups(
        &self,
        request: Request<ListGroupsRequest>,
    ) -> Result<Response<ListGroupsResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;

        let rows = self.store.list_groups_for_user(user_id).await?;
        let groups = rows.into_iter().map(summary_to_proto).collect();
        Ok(Response::new(ListGroupsResponse { groups }))
    }

    async fn get_group_info(
        &self,
        request: Request<GetGroupInfoRequest>,
    ) -> Result<Response<GetGroupInfoResponse>, Status> {
        let _claims = authenticate(&request, &self.jwt)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        let row = self.store.get_group(group_id).await?;
        let description = row.description.clone();
        let member_ids = self
            .store
            .member_ids(group_id)
            .await?
            .into_iter()
            .map(|id| UserId {
                value: id.to_string(),
            })
            .collect();

        Ok(Response::new(GetGroupInfoResponse {
            summary: Some(summary_to_proto(row)),
            description,
            member_ids,
        }))
    }
}

// --- helpers ----------------------------------------------------------------

fn summary_to_proto(row: GroupSummaryRow) -> GroupSummary {
    let kind = match row.kind.as_str() {
        "public" => ChatKind::Public,
        "private" => ChatKind::Private,
        _ => ChatKind::Unspecified,
    };
    GroupSummary {
        group_id: Some(GroupId {
            value: row.id.to_string(),
        }),
        name: row.name,
        kind: kind as i32,
        member_count: row.member_count.max(0) as u32,
    }
}

fn require_group_id(group_id: &Option<GroupId>) -> Result<Uuid, ServerError> {
    let value = group_id
        .as_ref()
        .ok_or_else(|| ServerError::InvalidArgument("group_id is required".into()))?;
    Uuid::parse_str(&value.value)
        .map_err(|_| ServerError::InvalidArgument("group_id is not a valid UUID".into()))
}

fn parse_uuid(s: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(s).map_err(|_| ServerError::InvalidArgument("invalid id in token".into()))
}
