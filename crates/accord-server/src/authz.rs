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
