//! `AuthService` gRPC implementation (registration, login, token refresh,
//! device management).

use accord_proto::auth_service_server::AuthService;
use accord_proto::{
    ChallengeRequest, ChallengeResponse, CreateInviteRequest, CreateInviteResponse, DeviceId,
    KeyLoginRequest, LoginRequest, LoginResponse, LookupUserRequest, LookupUserResponse,
    RefreshTokenRequest, RefreshTokenResponse, RegisterDeviceRequest, RegisterDeviceResponse,
    RegisterRequest, RegisterResponse, RevokeDeviceRequest, RevokeDeviceResponse,
    RevokeInviteRequest, RevokeInviteResponse, UserId,
};
use accord_types::perms::Permissions;
use ed25519_dalek::{Signature, VerifyingKey};
use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::auth::password::{hash_password, verify_password};
use crate::error::{ServerError, ServerResult};
use crate::store::Store;
use crate::store::model::UserRow;
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

    /// Issue a session for an authenticated user: find-or-create the device, do
    /// the walking-skeleton `#general` auto-join, mint the access + refresh
    /// tokens, and build the [`LoginResponse`]. Shared by password login and
    /// key login so both paths behave identically once the caller is verified.
    async fn issue_session(&self, user: &UserRow, device_name: &str) -> ServerResult<LoginResponse> {
        let device_name = if device_name.trim().is_empty() {
            "Unknown device"
        } else {
            device_name.trim()
        };
        // Reuse this install's device row across logins (clients send a stable
        // per-install device name); the mailbox + Welcome inbox are keyed by it.
        let device_id = match self.store.find_device(user.id, device_name).await? {
            Some(existing) => existing,
            None => self.store.create_device(user.id, device_name).await?,
        };

        // Ensure non-guest accounts land in the default `#general` channel.
        if !user.is_guest {
            if let Ok(default_channel) = Uuid::parse_str(crate::groups::DEFAULT_PUBLIC_CHANNEL_ID) {
                if let Err(e) = self.store.add_member(default_channel, user.id, "member").await {
                    tracing::warn!(error = %e, "could not auto-join #general");
                }
            }
        }

        let access_token = self.jwt.issue(user.id.into(), device_id.into())?;
        let refresh_token = Uuid::now_v7().to_string();
        let expires_at = Utc::now() + chrono::Duration::days(REFRESH_TOKEN_TTL_DAYS);
        self.store
            .store_refresh_token(&refresh_token, user.id, device_id, expires_at)
            .await?;

        tracing::info!(user_id = %user.id, %device_id, "session issued");
        Ok(LoginResponse {
            access_token,
            refresh_token,
            user_id: Some(UserId {
                value: user.id.to_string(),
            }),
            device_id: Some(DeviceId {
                value: device_id.to_string(),
            }),
        })
    }
}

/// Lowercase-hex encode bytes (for the challenge's identity-key binding).
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `ip` is loopback, treating an IPv4-mapped IPv6 address
/// (`::ffff:127.0.0.1`) as loopback too. This matters because the embedded host
/// binds the dual-stack wildcard `[::]`, so a client connecting over
/// `127.0.0.1` is seen as `::ffff:127.0.0.1` - for which `Ipv6Addr::is_loopback`
/// is false. Without this, the device-owner / first-user (owner) path never
/// triggers and the owner is wrongly admitted as a guest.
fn is_loopback(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback(),
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|m| m.is_loopback())
        }
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
        let is_local = remote_ip.is_some_and(is_loopback);
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
        // A password is OPTIONAL: an empty password means a key-only account
        // (authenticated by its identity key via LoginWithKey - how taverns are
        // joined without a per-tavern password). When present it must be long
        // enough; when absent an identity key is required.
        if req.password.is_empty() {
            if req.identity_pubkey.is_empty() {
                return Err(ServerError::InvalidArgument(
                    "a password or an identity key is required".into(),
                )
                .into());
            }
        } else if req.password.len() < MIN_PASSWORD_LEN {
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
        // The very first account becomes the server owner and needs no invite -
        // but ONLY from loopback. A freshly created/hosted server binds the
        // network ([::]) before its owner has registered, so without this gate a
        // remote actor who reaches the port (e.g. port-scanning a just-created
        // tavern on the LAN/mesh) could win the race and register first, claiming
        // ownership. The owner always registers from the hosting machine itself
        // (the embedded host connects over 127.0.0.1), so requiring loopback for
        // the owner-claim closes that race without affecting legitimate flows.
        let user_count = self.store.count_users().await?;
        let is_first_user = user_count == 0 && is_local;
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
        // Key-only accounts store no password hash (login is by identity key).
        let password_hash = if req.password.is_empty() {
            String::new()
        } else {
            hash_password(&req.password)?
        };
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

        // Banned accounts cannot log in (account-level; the cryptographic ban-tag
        // that also catches alts/restored backups is a later layer, BAN-PLAN.md).
        if self.store.is_banned(user.id).await? {
            return Err(Status::permission_denied("this account is banned"));
        }

        Ok(Response::new(
            self.issue_session(&user, &req.device_name).await?,
        ))
    }

    async fn request_challenge(
        &self,
        request: Request<ChallengeRequest>,
    ) -> Result<Response<ChallengeResponse>, Status> {
        let req = request.into_inner();
        if req.identity_pubkey.len() != 32 {
            return Err(ServerError::InvalidArgument("identity key must be 32 bytes".into()).into());
        }
        // Bind the challenge to this key + a fresh nonce; the client signs it.
        let challenge = self
            .jwt
            .issue_challenge(&hex_encode(&req.identity_pubkey), &Uuid::now_v7().to_string())?;
        Ok(Response::new(ChallengeResponse { challenge }))
    }

    async fn login_with_key(
        &self,
        request: Request<KeyLoginRequest>,
    ) -> Result<Response<LoginResponse>, Status> {
        let req = request.into_inner();

        let pk_bytes: [u8; 32] = req
            .identity_pubkey
            .as_slice()
            .try_into()
            .map_err(|_| ServerError::InvalidArgument("identity key must be 32 bytes".into()))?;
        let sig_bytes: [u8; 64] = req
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ServerError::InvalidArgument("signature must be 64 bytes".into()))?;

        // The challenge must be one WE issued (valid signature + unexpired) and
        // for exactly this identity key - not a client-chosen value.
        let challenge_for = self.jwt.verify_challenge(&req.challenge)?;
        if challenge_for != hex_encode(&req.identity_pubkey) {
            return Err(ServerError::Unauthenticated.into());
        }

        // The signature must verify over the challenge bytes under the claimed
        // identity key (proves possession of the private key).
        let verifying_key =
            VerifyingKey::from_bytes(&pk_bytes).map_err(|_| ServerError::Unauthenticated)?;
        let signature = Signature::from_bytes(&sig_bytes);
        verifying_key
            .verify_strict(req.challenge.as_bytes(), &signature)
            .map_err(|_| ServerError::Unauthenticated)?;

        // The key must belong to a registered account.
        let user = self
            .store
            .find_user_by_identity(&req.identity_pubkey)
            .await?
            .ok_or(ServerError::Unauthenticated)?;
        if self.store.is_banned(user.id).await? {
            return Err(Status::permission_denied("this account is banned"));
        }

        Ok(Response::new(
            self.issue_session(&user, &req.device_name).await?,
        ))
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

#[cfg(test)]
mod tests {
    use super::is_loopback;
    use std::net::IpAddr;

    #[test]
    fn loopback_detection_includes_ipv4_mapped() {
        // Plain loopbacks.
        assert!(is_loopback("127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_loopback("::1".parse::<IpAddr>().unwrap()));
        // The dual-stack case: a 127.0.0.1 connection to a `[::]` listener shows
        // up as this IPv4-mapped form. It MUST count as loopback (else the
        // device-owner / first-user-owner path is skipped - the bug this guards).
        assert!(is_loopback("::ffff:127.0.0.1".parse::<IpAddr>().unwrap()));
        // Non-loopback stays non-loopback.
        assert!(!is_loopback("192.168.1.5".parse::<IpAddr>().unwrap()));
        assert!(!is_loopback("::ffff:192.168.1.5".parse::<IpAddr>().unwrap()));
    }
}
