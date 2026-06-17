//! `GroupService` implementation.
//!
//! Public channels: create/list/info/open-self-join, plus tavern administration
//! (delete, member list, identity) and moderation (kick/ban). Private (MLS) group
//! creation carries the initial Commit + Welcomes (the server stays an opaque
//! relay). Privileged mutations are gated by RBAC ([`crate::authz`]) AND the
//! guardrail/auto-mod layer ([`crate::guardrails`]) - the latter rate-limits and
//! flags hostile actions even for admins, audits them, and alerts owner/admins.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use accord_proto::group_service_server::GroupService;
use accord_proto::{
    AddMembersRequest, AddMembersResponse, AuditEntry, BanInfo, BanMemberRequest, BanMemberResponse,
    ChatKind, CreateGroupResponse, CreatePrivateGroupRequest, CreatePublicGroupRequest,
    DeleteGroupRequest, DeleteGroupResponse, GetGroupInfoRequest, GetGroupInfoResponse,
    GetTavernRequest, GroupId, GroupSummary, KickMemberRequest, KickMemberResponse,
    ListAuditRequest, ListAuditResponse, ListBansRequest, ListBansResponse, ListGroupsRequest,
    ListGroupsResponse, ListMembersRequest, ListMembersResponse, MemberInfo, ModAlert,
    RemoveMembersRequest, RemoveMembersResponse, Severity, TavernInfo, UnbanMemberRequest,
    UnbanMemberResponse, UpdateTavernRequest, UserId,
};
use accord_types::perms::Permissions;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::authz;
use crate::error::ServerError;
use crate::guardrails::{ActionClass, ActionContext, GuardrailDecision, Guardrails};
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
    guardrails: Arc<Guardrails>,
}

impl GroupSvc {
    /// Construct the service. The [`Hub`] relays MLS Welcomes (private groups) and
    /// `ModAlert`s; [`Guardrails`] gates privileged mutations.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        jwt: JwtKeys,
        hub: Arc<Hub>,
        guardrails: Arc<Guardrails>,
    ) -> Self {
        Self {
            store,
            jwt,
            hub,
            guardrails,
        }
    }

    /// Run a privileged action through the guardrail layer: rate-limit + name
    /// heuristics, audit-log the notable outcomes, and alert owner/admins live.
    /// Returns `Ok` if the action may proceed, else a mapped gRPC `Status`.
    async fn guard(
        &self,
        actor: Uuid,
        action: ActionClass,
        target: &str,
        name: Option<&str>,
        recent_names: &[String],
    ) -> Result<(), Status> {
        let is_owner = self.store.is_owner(actor).await.unwrap_or(false);
        let ctx = ActionContext {
            name,
            recent_names,
            is_owner,
        };
        let decision = self.guardrails.check(actor, action, &ctx);

        if decision.is_notable() {
            let (verdict, reason, severity) = match &decision {
                GuardrailDecision::Throttle { reason, .. } => {
                    ("throttle", reason.clone(), Severity::Hostile)
                }
                GuardrailDecision::Deny { reason } => ("deny", reason.clone(), Severity::Hostile),
                GuardrailDecision::AllowFlagged { reason } => {
                    ("flagged", reason.clone(), Severity::Warn)
                }
                GuardrailDecision::Allow => ("allow", String::new(), Severity::Info),
            };
            // Best-effort: a moderation audit failure must not block the action's
            // own error path.
            let _ = self
                .store
                .record_audit(actor, action.as_str(), target, verdict, &reason)
                .await;
            self.alert_admins(actor, action.as_str(), target, &reason, severity)
                .await;
        }

        match decision {
            GuardrailDecision::Allow | GuardrailDecision::AllowFlagged { .. } => Ok(()),
            GuardrailDecision::Throttle { reason, .. } => Err(Status::resource_exhausted(reason)),
            GuardrailDecision::Deny { reason } => Err(Status::failed_precondition(reason)),
        }
    }

    /// Fan a `ModAlert` to connected owner/admin devices.
    async fn alert_admins(
        &self,
        actor: Uuid,
        action: &str,
        target: &str,
        reason: &str,
        severity: Severity,
    ) {
        let mask =
            (Permissions::ADMINISTRATOR.bits() | Permissions::MANAGE_SERVER.bits()) as i64;
        let Ok(devices) = self.store.admin_device_ids(mask).await else {
            return;
        };
        if devices.is_empty() {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let alert = ModAlert {
            actor_id: Some(UserId {
                value: actor.to_string(),
            }),
            action: action.to_owned(),
            target: target.to_owned(),
            reason: reason.to_owned(),
            severity: severity as i32,
            timestamp: Some(prost_types::Timestamp {
                seconds: now.as_secs() as i64,
                nanos: now.subsec_nanos() as i32,
            }),
        };
        self.hub.send_mod_alert(&devices, alert);
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
        let channel_kind = match req.channel_kind.trim() {
            "voice" => "voice",
            _ => "text",
        };

        // RBAC: only members who can manage channels may create them.
        authz::require(self.store.as_ref(), user_id, Permissions::MANAGE_CHANNELS).await?;

        // Guardrail: rate-limit + spam/random-name heuristics over the existing
        // channel names this user can see.
        let recent_names: Vec<String> = self
            .store
            .list_groups_for_user(user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|g| g.kind == "public")
            .map(|g| g.name)
            .collect();
        self.guard(
            user_id,
            ActionClass::CreateChannel,
            name,
            Some(name),
            &recent_names,
        )
        .await?;

        let group_id = self
            .store
            .create_public_group(name, req.description.trim(), channel_kind)
            .await?;
        // The creator owns the channel.
        self.store.add_member(group_id, user_id, "owner").await?;

        tracing::info!(%group_id, name, channel_kind, "created public group");
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

        for member in &req.member_ids {
            let uid = Uuid::parse_str(&member.value).map_err(|_| {
                ServerError::InvalidArgument("member id is not a valid UUID".into())
            })?;
            // A guest cannot be added to channels either (by themselves or others).
            if self.store.is_user_guest(uid).await? {
                return Err(ServerError::PermissionDenied.into());
            }
            // A banned account cannot (re)join channels.
            if self.store.is_banned(uid).await? {
                return Err(Status::failed_precondition("user is banned from this server"));
            }
            self.store.add_member(group_id, uid, "member").await?;
        }
        Ok(Response::new(AddMembersResponse {}))
    }

    async fn remove_members(
        &self,
        request: Request<RemoveMembersRequest>,
    ) -> Result<Response<RemoveMembersResponse>, Status> {
        // Self-leave for any member; removing others requires KICK_MEMBERS.
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        for member in &req.member_ids {
            let uid = Uuid::parse_str(&member.value).map_err(|_| {
                ServerError::InvalidArgument("member id is not a valid UUID".into())
            })?;
            if uid != caller {
                authz::require(self.store.as_ref(), caller, Permissions::KICK_MEMBERS).await?;
                self.guard(caller, ActionClass::KickMember, &uid.to_string(), None, &[])
                    .await?;
            }
            self.store.remove_member(group_id, uid).await?;
        }
        Ok(Response::new(RemoveMembersResponse {}))
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

    async fn delete_group(
        &self,
        request: Request<DeleteGroupRequest>,
    ) -> Result<Response<DeleteGroupResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        let group = self.store.get_group(group_id).await?;
        if group.kind != "public" {
            return Err(Status::failed_precondition(
                "private groups are not deleted through this RPC",
            ));
        }
        authz::require(self.store.as_ref(), user_id, Permissions::MANAGE_CHANNELS).await?;
        self.guard(user_id, ActionClass::DeleteChannel, &group.name, None, &[])
            .await?;

        self.store.delete_group(group_id).await?;
        tracing::info!(%group_id, "deleted public group");
        Ok(Response::new(DeleteGroupResponse {}))
    }

    async fn list_members(
        &self,
        request: Request<ListMembersRequest>,
    ) -> Result<Response<ListMembersResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;

        if !self.store.is_member(group_id, user_id).await? {
            return Err(ServerError::PermissionDenied.into());
        }

        let rows = self.store.list_members(group_id).await?;
        let mut members = Vec::with_capacity(rows.len());
        for m in rows {
            let role_ids = self
                .store
                .roles_for_user(m.user_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.id.to_string())
                .collect();
            // Online = any of the member's devices has a live stream here
            // (single-instance liveness, like the hub's other presence checks).
            let online = self
                .store
                .device_ids_for_user(m.user_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .any(|d| self.hub.is_connected(d));
            members.push(MemberInfo {
                user_id: Some(UserId {
                    value: m.user_id.to_string(),
                }),
                username: m.username,
                display_name: m.display_name,
                is_owner: m.is_owner,
                online,
                role_ids,
            });
        }
        Ok(Response::new(ListMembersResponse { members }))
    }

    async fn get_tavern(
        &self,
        request: Request<GetTavernRequest>,
    ) -> Result<Response<TavernInfo>, Status> {
        let _claims = authenticate(&request, &self.jwt)?;
        let t = self.store.get_tavern().await?;
        Ok(Response::new(TavernInfo {
            name: t.name,
            icon_url: t.icon_url,
            description: t.description,
            linking_enabled: t.linking_enabled,
            banner_url: t.banner_url,
        }))
    }

    async fn update_tavern(
        &self,
        request: Request<UpdateTavernRequest>,
    ) -> Result<Response<TavernInfo>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub)?;
        let req = request.into_inner();

        authz::require(self.store.as_ref(), user_id, Permissions::MANAGE_SERVER).await?;
        self.guard(user_id, ActionClass::UpdateServer, "tavern", None, &[])
            .await?;

        let tavern_id = Uuid::parse_str(super::TAVERN_ID)
            .map_err(|_| Status::internal("bad TAVERN_ID constant"))?;
        self.store
            .upsert_tavern(
                tavern_id,
                req.name.trim(),
                req.icon_url.trim(),
                req.description.trim(),
                req.banner_url.trim(),
            )
            .await?;
        let t = self.store.get_tavern().await?;
        Ok(Response::new(TavernInfo {
            name: t.name,
            icon_url: t.icon_url,
            description: t.description,
            linking_enabled: t.linking_enabled,
            banner_url: t.banner_url,
        }))
    }

    async fn kick_member(
        &self,
        request: Request<KickMemberRequest>,
    ) -> Result<Response<KickMemberResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        let req = request.into_inner();
        let group_id = require_group_id(&req.group_id)?;
        let target = require_user_id(&req.user_id)?;

        if target != caller {
            authz::require(self.store.as_ref(), caller, Permissions::KICK_MEMBERS).await?;
            // Role hierarchy: you can only kick members you outrank.
            authz::require_outranks(self.store.as_ref(), caller, target).await?;
            self.guard(caller, ActionClass::KickMember, &target.to_string(), None, &[])
                .await?;
        }
        self.store.remove_member(group_id, target).await?;
        tracing::info!(%group_id, %target, "kicked member");
        Ok(Response::new(KickMemberResponse {}))
    }

    async fn ban_member(
        &self,
        request: Request<BanMemberRequest>,
    ) -> Result<Response<BanMemberResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        let req = request.into_inner();
        let target = require_user_id(&req.user_id)?;

        authz::require(self.store.as_ref(), caller, Permissions::BAN_MEMBERS).await?;
        if self.store.is_owner(target).await? {
            return Err(Status::failed_precondition("cannot ban the server owner"));
        }
        // Role hierarchy: you can only ban members you outrank.
        authz::require_outranks(self.store.as_ref(), caller, target).await?;
        self.guard(caller, ActionClass::BanMember, &target.to_string(), None, &[])
            .await?;

        // Remove the banned account from every channel, then record the ban. The
        // cryptographic ban-tag (BAN-PLAN.md Layer 2) layers on later; this is the
        // account-level subset.
        for gid in self.store.group_ids_for_user(target).await? {
            self.store.remove_member(gid, target).await?;
        }
        self.store
            .ban_user(target, caller, req.reason.trim())
            .await?;
        // Best-effort: drop the banned user's devices from any live voice channels
        // and revoke nothing here (token revocation is a later hardening step).
        tracing::info!(%target, "banned member");
        Ok(Response::new(BanMemberResponse {}))
    }

    async fn unban_member(
        &self,
        request: Request<UnbanMemberRequest>,
    ) -> Result<Response<UnbanMemberResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        let req = request.into_inner();
        let target = require_user_id(&req.user_id)?;

        authz::require(self.store.as_ref(), caller, Permissions::BAN_MEMBERS).await?;
        self.store.unban_user(target).await?;
        Ok(Response::new(UnbanMemberResponse {}))
    }

    async fn list_bans(
        &self,
        request: Request<ListBansRequest>,
    ) -> Result<Response<ListBansResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        authz::require(self.store.as_ref(), caller, Permissions::BAN_MEMBERS).await?;

        let bans = self
            .store
            .list_bans()
            .await?
            .into_iter()
            .map(|b| BanInfo {
                user_id: Some(UserId {
                    value: b.user_id.to_string(),
                }),
                reason: b.reason,
                banned_by: Some(UserId {
                    value: b.banned_by.to_string(),
                }),
                created_at_ms: b.created_at_ms,
            })
            .collect();
        Ok(Response::new(ListBansResponse { bans }))
    }

    async fn list_audit(
        &self,
        request: Request<ListAuditRequest>,
    ) -> Result<Response<ListAuditResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let caller = parse_uuid(&claims.sub)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_SERVER).await?;

        let limit = request.into_inner().limit.clamp(1, 500) as i64;
        let entries = self
            .store
            .list_audit(limit)
            .await?
            .into_iter()
            .map(|a| AuditEntry {
                actor_id: Some(UserId {
                    value: a.actor_id.to_string(),
                }),
                action: a.action,
                target: a.target,
                verdict: a.verdict,
                reason: a.reason,
                created_at_ms: a.created_at_ms,
            })
            .collect();
        Ok(Response::new(ListAuditResponse { entries }))
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
        channel_kind: row.channel_kind,
    }
}

fn require_group_id(group_id: &Option<GroupId>) -> Result<Uuid, ServerError> {
    let value = group_id
        .as_ref()
        .ok_or_else(|| ServerError::InvalidArgument("group_id is required".into()))?;
    Uuid::parse_str(&value.value)
        .map_err(|_| ServerError::InvalidArgument("group_id is not a valid UUID".into()))
}

fn require_user_id(user_id: &Option<UserId>) -> Result<Uuid, ServerError> {
    let value = user_id
        .as_ref()
        .ok_or_else(|| ServerError::InvalidArgument("user_id is required".into()))?;
    Uuid::parse_str(&value.value)
        .map_err(|_| ServerError::InvalidArgument("user_id is not a valid UUID".into()))
}

fn parse_uuid(s: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(s).map_err(|_| ServerError::InvalidArgument("invalid id in token".into()))
}
