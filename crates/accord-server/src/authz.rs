//! Permission-scoped authorization.
//!
//! Central place where the API enforces RBAC. A member's effective permissions
//! are: the server **owner** -> everything; otherwise the `@everyone` default
//! role OR'd with their assigned roles, with **ADMINISTRATOR** overriding all.
//!
//! Services call [`require`] before privileged actions and [`effective`] to
//! report a member's permissions.

use accord_types::perms::Permissions;
use uuid::Uuid;

use crate::error::{ServerError, ServerResult};
use crate::store::Store;

/// Compute a user's effective permissions.
///
/// # Errors
/// Returns [`ServerError`] on a store failure.
pub async fn effective(store: &dyn Store, user_id: Uuid) -> ServerResult<Permissions> {
    if store.is_owner(user_id).await? {
        return Ok(Permissions::all());
    }
    // Guests (open_dms DM-only accounts) get NO permissions - not even the
    // @everyone defaults. Otherwise a guest could e.g. mint an invite
    // (CREATE_INVITE is in default_everyone) and upgrade themselves.
    if store.is_user_guest(user_id).await? {
        return Ok(Permissions::from_bits(0));
    }
    let bits = store.member_permissions(user_id).await? as u64;
    Ok(Permissions::from_bits(bits))
}

/// Require that `user_id` has `perm` (ADMINISTRATOR / owner always pass).
///
/// # Errors
/// Returns [`ServerError::PermissionDenied`] if the user lacks `perm`.
pub async fn require(store: &dyn Store, user_id: Uuid, perm: Permissions) -> ServerResult<()> {
    if effective(store, user_id).await?.allows(perm) {
        Ok(())
    } else {
        Err(ServerError::PermissionDenied)
    }
}

/// A user's hierarchy rank = the highest `position` among their assigned roles,
/// with the **owner** sitting above everyone (`i32::MAX`). Higher rank = more
/// power; rank governs *who* you can act on, independent of which permission
/// bits you hold.
///
/// # Errors
/// Returns [`ServerError`] on a store failure.
pub async fn rank(store: &dyn Store, user_id: Uuid) -> ServerResult<i32> {
    if store.is_owner(user_id).await? {
        return Ok(i32::MAX);
    }
    store.highest_role_position(user_id).await
}

/// Whether `actor` outranks `target` and may therefore kick/ban/role-manage
/// them. The owner can be acted on by no one; otherwise the actor's rank must be
/// strictly greater than the target's. (A self-target is always allowed - that
/// is a self-leave, not moderation.)
///
/// # Errors
/// Returns [`ServerError`] on a store failure.
pub async fn can_act_on(store: &dyn Store, actor: Uuid, target: Uuid) -> ServerResult<bool> {
    if actor == target {
        return Ok(true);
    }
    if store.is_owner(target).await? {
        return Ok(false);
    }
    Ok(rank(store, actor).await? > rank(store, target).await?)
}

/// Require that `actor` outranks `target`.
///
/// # Errors
/// Returns [`ServerError::PermissionDenied`] if `actor` does not outrank `target`.
pub async fn require_outranks(store: &dyn Store, actor: Uuid, target: Uuid) -> ServerResult<()> {
    if can_act_on(store, actor, target).await? {
        Ok(())
    } else {
        Err(ServerError::PermissionDenied)
    }
}

/// Require that `actor` may manage a role at `role_position`: a non-owner can
/// only create/edit/assign/reorder roles strictly **below** their own rank, so
/// you can never touch a role at or above your highest.
///
/// # Errors
/// Returns [`ServerError::PermissionDenied`] if the role is not below the actor.
pub async fn require_role_below(
    store: &dyn Store,
    actor: Uuid,
    role_position: i32,
) -> ServerResult<()> {
    if rank(store, actor).await? > role_position {
        Ok(())
    } else {
        Err(ServerError::PermissionDenied)
    }
}
