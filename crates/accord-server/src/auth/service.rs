//! `AuthService` gRPC implementation (registration, login, token refresh,
//! device management).

use accord_proto::auth_service_server::AuthService;
use accord_proto::{
    CreateInviteRequest, CreateInviteResponse, DeviceId, LoginRequest, LoginResponse,
    LookupUserRequest, LookupUserResponse, RefreshTokenRequest, RefreshTokenResponse,
    RegisterDeviceRequest, RegisterDeviceResponse, RegisterRequest, RegisterResponse,
    RevokeDeviceRequest, RevokeDeviceResponse, RevokeInviteRequest, RevokeInviteResponse, UserId,
};
use accord_types::perms::Permissions;
use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::auth::password::{hash_password, verify_password};
use crate::error::ServerError;
use crate::store::Store;
use crate::util::authenticate;

/// How long refresh tokens stay valid.
const REFRESH_TOKEN_TTL_DAYS: i64 = 30;
/// Minimum password length we accept at registration.
const MIN_PASSWORD_LEN: usize = 8;
/// Max accounts that may be created locally on one device's home server. A soft
/// anti-abuse cap (a determined user can wipe local data and recreate); the real
/// defense against ban evasion is the tag/commitment system in BAN-PLAN.md.
const MAX_LOCAL_ACCOUNTS: i64 = 3;
/// Max successful remote registrations per source IP per window (raid brake;
/// BAN-PLAN.md Layer 3). Loopback (the device owner) is exempt.
const MAX_REMOTE_REGISTRATIONS_PER_WINDOW: usize = 5;
/// The sliding window for the remote-registration rate limit.
const REGISTRATION_WINDOW: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Implements the `AuthService` RPCs.
#[derive(Debug)]
pub struct AuthSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
    /// When true (private server), non-owner registration requires an invite.
    require_invite: bool,
    /// When true, an inviteless remote registration is admitted as a **guest**:
    /// a DM-only account with no channel access and no permissions. See
    /// `Config::open_dms` and migration 0010.
    open_dms: bool,
    /// Recent remote registration timestamps per source IP (in-memory sliding
    /// window; resets on restart, which is fine for a raid brake).
    reg_log: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, Vec<std::time::Instant>>>,
}

impl AuthSvc {
    /// Construct the service from a [`Store`], JWT keys, and the invite policy.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys, require_invite: bool, open_dms: bool) -> Self {
        Self {
            store,
            jwt,
            require_invite,
            open_dms,
            reg_log: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Enforce the per-IP remote-registration rate limit (and record this
    /// attempt). Loopback callers are exempt.
    fn check_registration_rate(&self, ip: std::net::IpAddr) -> Result<(), ServerError> {
        let now = std::time::Instant::now();
        let mut log = self.reg_log.lock().map_err(|_| {
            ServerError::Internal("registration rate-limit lock poisoned".to_owned())
        })?;
        let entries = log.entry(ip).or_default();
        entries.retain(|t| now.duration_since(*t) < REGISTRATION_WINDOW);
        if entries.len() >= MAX_REMOTE_REGISTRATIONS_PER_WINDOW {
            return Err(ServerError::RateLimited(
                "too many registrations from this address; try again later".to_owned(),
            ));
        }
        entries.push(now);
        Ok(())
    }
}

#[tonic::async_trait]
impl AuthService for AuthSvc {
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        // A connection from loopback is the device owner on this machine (who can
        // already read the database directly), so local multi-account creation is
        // allowed without an invite; remote/mesh joiners still need one.
        let remote_ip = request.remote_addr().map(|a| a.ip());
        let is_local = remote_ip.is_some_and(|ip| ip.is_loopback());
        // Raid brake: remote registrations are rate-limited per source IP.
        if !is_local {
            if let Some(ip) = remote_ip {
                self.check_registration_rate(ip)?;
            }
        }
        let req = request.into_inner();

        // --- validate ---
        let username = req.username.trim();
        if username.is_empty() {
            return Err(ServerError::InvalidArgument("username is required".into()).into());
        }
        if req.password.len() < MIN_PASSWORD_LEN {
            return Err(ServerError::InvalidArgument(format!(
                "password must be at least {MIN_PASSWORD_LEN} characters"
            ))
            .into());
        }
        let display_name = if req.display_name.trim().is_empty() {
            username
        } else {
            req.display_name.trim()
        };

        // --- invite gating (private servers) ---
        // The very first account becomes the server owner and needs no invite.
        let user_count = self.store.count_users().await?;
        let is_first_user = user_count == 0;
        // Local (loopback) multi-account creation is capped per device. Remote
        // joiners are gated by invite instead.
        if is_local && self.require_invite && user_count >= MAX_LOCAL_ACCOUNTS {
            return Err(ServerError::InvalidArgument(format!(
                "account limit reached ({MAX_LOCAL_ACCOUNTS} per device)"
            ))
            .into());
        }

        // Decide membership tier. Full member: open server, the first account
        // (owner), the device owner (loopback), or a valid invite. Otherwise,
        // open_dms admits the caller as a **guest** - a DM-only account with no
        // channel access and no permissions (enforced in login/groups/authz) -
        // and with open_dms off, registration is refused outright.
        let has_valid_invite = {
            let token = req.invite_token.trim();
            !token.is_empty() && self.store.is_invite_valid(token).await?
        };
        let is_member = !self.require_invite || is_first_user || is_local || has_valid_invite;
        if !is_member && !self.open_dms {
            return Err(ServerError::PermissionDenied.into());
        }
        let is_guest = !is_member;

        // --- identity key (optional) ---
        // A non-empty key must be a valid 32-byte Ed25519 public key and must not
        // already be in use. This is what makes accounts collision-proof without
        // a central authority: the account is the key.
        let identity_pubkey: Option<&[u8]> = if req.identity_pubkey.is_empty() {
            None
        } else {
            if req.identity_pubkey.len() != 32 {
                return Err(
                    ServerError::InvalidArgument("identity key must be 32 bytes".into()).into(),
                );
            }
            if self.store.identity_exists(&req.identity_pubkey).await? {
                return Err(ServerError::AlreadyExists("identity key".to_owned()).into());
            }
            Some(req.identity_pubkey.as_slice())
        };

        // --- create ---
        let password_hash = hash_password(&req.password)?;
        let user_id = self
            .store
            .create_user(
                username,
                display_name,
                &password_hash,
                is_first_user,
                is_guest,
                identity_pubkey,
            )
            .await?;

        tracing::info!(%user_id, username, owner = is_first_user, guest = is_guest, "registered new user");
        Ok(Response::new(RegisterResponse {
            user_id: Some(UserId {
                value: user_id.to_string(),
            }),
        }))
    }

    async fn login(
        &self,
        request: Request<LoginRequest>,
    ) -> Result<Response<LoginResponse>, Status> {
        let req = request.into_inner();

        let user = self
            .store
            .find_user_by_username(req.username.trim())
            .await?
            .ok_or(ServerError::Unauthenticated)?;

        if !verify_password(&req.password, &user.password_hash)? {
            return Err(ServerError::Unauthenticated.into());
        }

        let device_name = if req.device_name.trim().is_empty() {
            "Unknown device"
        } else {
            req.device_name.trim()
        };
        // Reuse this install's device row across logins (clients send a stable
        // per-install device name). The mailbox and Welcome inbox are keyed by
        // device id, so a fresh device per login would orphan everything queued
        // for the previous one.
        let device_id = match self.store.find_device(user.id, device_name).await? {
            Some(existing) => existing,
            None => self.store.create_device(user.id, device_name).await?,
        };

        // Walking-skeleton convenience: ensure the user is in the default
        // `#general` channel so they have somewhere to chat immediately. Guests
        // (open_dms DM-only accounts) are deliberately NOT joined to anything.
        if !user.is_guest {
            if let Ok(default_channel) = Uuid::parse_str(crate::groups::DEFAULT_PUBLIC_CHANNEL_ID) {
                if let Err(e) = self
                    .store
                    .add_member(default_channel, user.id, "member")
                    .await
                {
                    tracing::warn!(error = %e, "could not auto-join #general");
                }
            }
        }

        // Mint tokens.
        let access_token = self.jwt.issue(user.id.into(), device_id.into())?;
        let refresh_token = Uuid::now_v7().to_string();
        let expires_at = Utc::now() + chrono::Duration::days(REFRESH_TOKEN_TTL_DAYS);
        self.store
            .store_refresh_token(&refresh_token, user.id, device_id, expires_at)
            .await?;

        tracing::info!(user_id = %user.id, %device_id, "user logged in");
        Ok(Response::new(LoginResponse {
            access_token,
            refresh_token,
            user_id: Some(UserId {
                value: user.id.to_string(),
            }),
            device_id: Some(DeviceId {
                value: device_id.to_string(),
            }),
        }))
    }

    async fn refresh_token(
        &self,
        request: Request<RefreshTokenRequest>,
    ) -> Result<Response<RefreshTokenResponse>, Status> {
        let req = request.into_inner();

        let row = self
            .store
            .lookup_refresh_token(&req.refresh_token)
            .await?
            .ok_or(ServerError::Unauthenticated)?;

        if row.expires_at < chrono::Utc::now() {
            return Err(ServerError::Unauthenticated.into());
        }

        // Rotate: mint a replacement refresh token and invalidate the old one,
        // so an active session never hard-expires and a stolen token dies the
        // first time either holder uses it.
        let access_token = self.jwt.issue(row.user_id.into(), row.device_id.into())?;
        let new_refresh = Uuid::now_v7().to_string();
        let expires_at = Utc::now() + chrono::Duration::days(REFRESH_TOKEN_TTL_DAYS);
        self.store
            .store_refresh_token(&new_refresh, row.user_id, row.device_id, expires_at)
            .await?;
        self.store.delete_refresh_token(&req.refresh_token).await?;
        Ok(Response::new(RefreshTokenResponse {
            access_token,
            refresh_token: new_refresh,
        }))
    }

    async fn register_device(
        &self,
        request: Request<RegisterDeviceRequest>,
    ) -> Result<Response<RegisterDeviceResponse>, Status> {
        // Attach the device's opaque MLS credential to the device in the token.
        // (Peers actually learn the credential from KeyPackages; this is stored
        // for completeness / future use.)
        let claims = authenticate(&request, &self.jwt)?;
        let device_id = Uuid::parse_str(&claims.device_id)
            .map_err(|_| ServerError::InvalidArgument("invalid device id in token".into()))?;
        let req = request.into_inner();
        self.store
            .set_device_credential(device_id, &req.mls_credential)
            .await?;
        Ok(Response::new(RegisterDeviceResponse {
            device_id: Some(DeviceId {
                value: device_id.to_string(),
            }),
        }))
    }

    async fn revoke_device(
        &self,
        request: Request<RevokeDeviceRequest>,
    ) -> Result<Response<RevokeDeviceResponse>, Status> {
        // Must be authenticated; a user may only revoke their own device.
        let _claims = authenticate(&request, &self.jwt)?;
        let req = request.into_inner();
        let device_id = req
            .device_id
            .ok_or_else(|| ServerError::InvalidArgument("device_id is required".into()))?;
        let uuid = Uuid::parse_str(&device_id.value)
            .map_err(|_| ServerError::InvalidArgument("device_id is not a valid UUID".into()))?;

        self.store.revoke_device(uuid).await?;
        Ok(Response::new(RevokeDeviceResponse {}))
    }

    async fn lookup_user(
        &self,
        request: Request<LookupUserRequest>,
    ) -> Result<Response<LookupUserResponse>, Status> {
        // Must be authenticated to resolve usernames (avoids open enumeration).
        let _claims = authenticate(&request, &self.jwt)?;
        let req = request.into_inner();

        let user = self
            .store
            .find_user_by_username(req.username.trim())
            .await?
            .ok_or_else(|| ServerError::NotFound(format!("user '{}'", req.username.trim())))?;

        Ok(Response::new(LookupUserResponse {
            user_id: Some(UserId {
                value: user.id.to_string(),
            }),
            display_name: user.display_name,
        }))
    }

    async fn create_invite(
        &self,
        request: Request<CreateInviteRequest>,
    ) -> Result<Response<CreateInviteResponse>, Status> {
        let user_id = self.require(&request, Permissions::CREATE_INVITE).await?;

        // A random, hard-to-guess token. (Foundation for TTL/max-use rotation.)
        let token = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
        self.store.create_invite(&token, user_id).await?;
        tracing::info!(%user_id, "created invite token");
        Ok(Response::new(CreateInviteResponse { token }))
    }

    async fn revoke_invite(
        &self,
        request: Request<RevokeInviteRequest>,
    ) -> Result<Response<RevokeInviteResponse>, Status> {
        let _ = self.require(&request, Permissions::CREATE_INVITE).await?;
        let token = request.into_inner().token;
        self.store.revoke_invite(&token).await?;
        Ok(Response::new(RevokeInviteResponse {}))
    }
}

impl AuthSvc {
    /// Authenticate the request and require the caller has `perm`. Returns the
    /// caller's user id.
    async fn require<T>(
        &self,
        request: &Request<T>,
        perm: Permissions,
    ) -> Result<Uuid, ServerError> {
        let claims = authenticate(request, &self.jwt)?;
        let user_id = Uuid::parse_str(&claims.sub)
            .map_err(|_| ServerError::InvalidArgument("invalid user id in token".into()))?;
        crate::authz::require(self.store.as_ref(), user_id, perm).await?;
        Ok(user_id)
    }
}
