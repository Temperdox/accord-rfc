//! `RoleService` implementation - permission-scoped role management.

use std::sync::Arc;

use accord_proto::role_service_server::RoleService;
use accord_proto::{
    AssignRoleRequest, AssignRoleResponse, CreateRoleRequest, DeleteRoleRequest,
    DeleteRoleResponse, GetMyPermissionsRequest, GetMyPermissionsResponse, ListRolesRequest,
    ListRolesResponse, ReorderRolesRequest, ReorderRolesResponse, Role, UnassignRoleRequest,
    UnassignRoleResponse, UpdateRoleRequest,
};
use accord_types::perms::Permissions;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::authz;
use crate::error::ServerError;
use crate::store::Store;
use crate::store::model::{RoleRow, RoleWrite};
use crate::util::authenticate;

/// Implements the `RoleService` RPCs.
#[derive(Debug)]
pub struct RoleSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
}

impl RoleSvc {
    /// Construct the service.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys) -> Self {
        Self { store, jwt }
    }

    /// Authenticate the request and return the caller's user id.
    fn caller<T>(&self, request: &Request<T>) -> Result<Uuid, ServerError> {
        let claims = authenticate(request, &self.jwt)?;
        Uuid::parse_str(&claims.sub)
            .map_err(|_| ServerError::InvalidArgument("invalid user id in token".into()))
    }
}

#[tonic::async_trait]
impl RoleService for RoleSvc {
    async fn list_roles(
        &self,
        request: Request<ListRolesRequest>,
    ) -> Result<Response<ListRolesResponse>, Status> {
        // Any authenticated member can see the role list.
        let _ = self.caller(&request)?;
        let roles = self
            .store
            .list_roles()
            .await?
            .into_iter()
            .map(role_to_proto)
            .collect();
        Ok(Response::new(ListRolesResponse { roles }))
    }

    async fn create_role(
        &self,
        request: Request<CreateRoleRequest>,
    ) -> Result<Response<Role>, Status> {
        let caller = self.caller(&request)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_ROLES).await?;
        let req = request.into_inner();

        let perms = parse_perms(&req.permissions)?;
        self.guard_admin_escalation(caller, perms).await?;

        let name = req.name.trim();
        if name.is_empty() {
            return Err(ServerError::InvalidArgument("role name is required".into()).into());
        }
        let write = RoleWrite {
            name: name.to_string(),
            permissions: perms.bits() as i64,
            color: req.color,
            icon: req.icon,
            hoist: req.hoist,
            mentionable: req.mentionable,
        };
        let id = self.store.create_role(&write).await?;
        let role = self
            .store
            .get_role(id)
            .await?
            .ok_or(ServerError::NotFound("role".into()))?;
        Ok(Response::new(role_to_proto(role)))
    }

    async fn update_role(
        &self,
        request: Request<UpdateRoleRequest>,
    ) -> Result<Response<Role>, Status> {
        let caller = self.caller(&request)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_ROLES).await?;
        let req = request.into_inner();
        let id = parse_uuid(&req.id, "role id")?;

        let perms = parse_perms(&req.permissions)?;
        self.guard_admin_escalation(caller, perms).await?;

        // Hierarchy: a non-owner can only edit roles strictly below their rank.
        let existing = self
            .store
            .get_role(id)
            .await?
            .ok_or(ServerError::NotFound("role".into()))?;
        authz::require_role_below(self.store.as_ref(), caller, existing.position).await?;

        let write = RoleWrite {
            name: req.name.trim().to_string(),
            permissions: perms.bits() as i64,
            color: req.color,
            icon: req.icon,
            hoist: req.hoist,
            mentionable: req.mentionable,
        };
        self.store.update_role(id, &write).await?;
        let role = self
            .store
            .get_role(id)
            .await?
            .ok_or(ServerError::NotFound("role".into()))?;
        Ok(Response::new(role_to_proto(role)))
    }

    async fn delete_role(
        &self,
        request: Request<DeleteRoleRequest>,
    ) -> Result<Response<DeleteRoleResponse>, Status> {
        let caller = self.caller(&request)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_ROLES).await?;
        let id = parse_uuid(&request.into_inner().id, "role id")?;

        let role = self
            .store
            .get_role(id)
            .await?
            .ok_or(ServerError::NotFound("role".into()))?;
        if role.is_default {
            return Err(
                ServerError::InvalidArgument("cannot delete the @everyone role".into()).into(),
            );
        }
        authz::require_role_below(self.store.as_ref(), caller, role.position).await?;
        self.store.delete_role(id).await?;
        Ok(Response::new(DeleteRoleResponse {}))
    }

    async fn reorder_roles(
        &self,
        request: Request<ReorderRolesRequest>,
    ) -> Result<Response<ReorderRolesResponse>, Status> {
        let caller = self.caller(&request)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_ROLES).await?;
        let req = request.into_inner();

        let ids: Vec<Uuid> = req
            .role_ids
            .iter()
            .map(|s| parse_uuid(s, "role id"))
            .collect::<Result<_, _>>()?;

        // Hierarchy: a non-owner may neither touch a role at/above their rank nor
        // push any role to a position at/above their rank. The new positions are
        // n..1 (first id highest); validate both old and new against the rank.
        let rank = authz::rank(self.store.as_ref(), caller).await?;
        let n = ids.len() as i32;
        for (i, id) in ids.iter().enumerate() {
            let role = self
                .store
                .get_role(*id)
                .await?
                .ok_or(ServerError::NotFound("role".into()))?;
            if role.is_default {
                return Err(ServerError::InvalidArgument(
                    "the @everyone role cannot be reordered".into(),
                )
                .into());
            }
            let new_pos = n - i as i32;
            if rank <= role.position || rank <= new_pos {
                return Err(ServerError::PermissionDenied.into());
            }
        }
        self.store.reorder_roles(&ids).await?;
        Ok(Response::new(ReorderRolesResponse {}))
    }

    async fn assign_role(
        &self,
        request: Request<AssignRoleRequest>,
    ) -> Result<Response<AssignRoleResponse>, Status> {
        let caller = self.caller(&request)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_ROLES).await?;
        let req = request.into_inner();
        let user_id = require_user_id(&req.user_id)?;
        let role_id = parse_uuid(&req.role_id, "role id")?;

        // Anti-escalation: can't hand out an ADMINISTRATOR role unless you are one.
        let role = self
            .store
            .get_role(role_id)
            .await?
            .ok_or(ServerError::NotFound("role".into()))?;
        self.guard_admin_escalation(caller, Permissions::from_bits(role.permissions as u64))
            .await?;
        // Hierarchy: can only grant roles below your rank, and only to members
        // you outrank.
        authz::require_role_below(self.store.as_ref(), caller, role.position).await?;
        authz::require_outranks(self.store.as_ref(), caller, user_id).await?;

        self.store.assign_role(user_id, role_id).await?;
        Ok(Response::new(AssignRoleResponse {}))
    }

    async fn unassign_role(
        &self,
        request: Request<UnassignRoleRequest>,
    ) -> Result<Response<UnassignRoleResponse>, Status> {
        let caller = self.caller(&request)?;
        authz::require(self.store.as_ref(), caller, Permissions::MANAGE_ROLES).await?;
        let req = request.into_inner();
        let user_id = require_user_id(&req.user_id)?;
        let role_id = parse_uuid(&req.role_id, "role id")?;

        let role = self
            .store
            .get_role(role_id)
            .await?
            .ok_or(ServerError::NotFound("role".into()))?;
        authz::require_role_below(self.store.as_ref(), caller, role.position).await?;
        authz::require_outranks(self.store.as_ref(), caller, user_id).await?;

        self.store.unassign_role(user_id, role_id).await?;
        Ok(Response::new(UnassignRoleResponse {}))
    }

    async fn get_my_permissions(
        &self,
        request: Request<GetMyPermissionsRequest>,
    ) -> Result<Response<GetMyPermissionsResponse>, Status> {
        let caller = self.caller(&request)?;
        let perms = authz::effective(self.store.as_ref(), caller).await?;
        let is_owner = self.store.is_owner(caller).await?;
        Ok(Response::new(GetMyPermissionsResponse {
            permissions: perms.bits().to_string(),
            is_owner,
        }))
    }
}

impl RoleSvc {
    /// Block setting/granting ADMINISTRATOR unless the caller is an admin/owner.
    async fn guard_admin_escalation(
        &self,
        caller: Uuid,
        target: Permissions,
    ) -> Result<(), ServerError> {
        if target.is_admin()
            && !authz::effective(self.store.as_ref(), caller)
                .await?
                .is_admin()
        {
            return Err(ServerError::PermissionDenied);
        }
        Ok(())
    }
}

// --- helpers ----------------------------------------------------------------

fn role_to_proto(r: RoleRow) -> Role {
    Role {
        id: r.id.to_string(),
        name: r.name,
        permissions: (r.permissions as u64).to_string(),
        position: r.position,
        is_default: r.is_default,
        color: r.color,
        icon: r.icon,
        hoist: r.hoist,
        mentionable: r.mentionable,
    }
}

fn parse_perms(s: &str) -> Result<Permissions, ServerError> {
    if s.trim().is_empty() {
        return Ok(Permissions::empty());
    }
    s.trim()
        .parse::<Permissions>()
        .map_err(|_| ServerError::InvalidArgument("bad permissions".into()))
}

fn parse_uuid(s: &str, what: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(s).map_err(|_| ServerError::InvalidArgument(format!("invalid {what}")))
}

fn require_user_id(id: &Option<accord_proto::UserId>) -> Result<Uuid, ServerError> {
    let value = id
        .as_ref()
        .ok_or_else(|| ServerError::InvalidArgument("user_id is required".into()))?;
    parse_uuid(&value.value, "user id")
}
