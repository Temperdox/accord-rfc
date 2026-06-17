//! Verifies the **roles & permissions (RBAC)** API end-to-end (headless):
//! 1. Owner (first account) + a normal member B register.
//! 2. B (only `@everyone`) is DENIED creating a role (no MANAGE_ROLES).
//! 3. Owner creates a "Moderator" role (MANAGE_ROLES) and assigns it to B.
//! 4. B can now create roles (permission took effect via assignment).
//! 5. Anti-escalation: B (MANAGE_ROLES but not admin) is DENIED creating an
//! ADMINISTRATOR role.
//! 6. The owner is allowed to (owner overrides everything).
//!
//! ```text
//! cargo run -p accord-server --example roles_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::role_service_client::RoleServiceClient;
use accord_proto::{
    AssignRoleRequest, CreateRoleRequest, GetMyPermissionsRequest, LoginRequest, RegisterRequest,
    UserId,
};
use accord_types::perms::Permissions;
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50063;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

/// Register + login; returns (token, user_id).
async fn account(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let mut auth = AuthServiceClient::new(ch.clone());
    auth.register(RegisterRequest {
        username: name.into(),
        password: "rolespw123".into(),
        display_name: name.into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let r = auth
        .login(LoginRequest {
            username: name.into(),
            password: "rolespw123".into(),
            device_name: "dev".into(),
        })
        .await?
        .into_inner();
    Ok((r.access_token, r.user_id.unwrap().value))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:roles-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "roles-smoke-secret".to_owned(),
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: false,
        open_dms: true,
        tls_cert_pem: None,
        tls_key_pem: None,
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        accord_server::run_with_shutdown(config, shutdown_rx)
            .await
            .expect("server ran");
    });

    let endpoint = format!("http://127.0.0.1:{PORT}");
    let channel = connect_with_retry(&endpoint).await?;
    println!("[ok] server up on {endpoint}");

    let (owner_tok, _owner_id) = account(&channel, "owner").await?;
    let (b_tok, b_id) = account(&channel, "bob").await?;
    println!("[ok] owner + member registered");

    let mut owner = RoleServiceClient::new(channel.clone());
    let mut bob = RoleServiceClient::new(channel.clone());

    // B has only @everyone - no MANAGE_ROLES.
    let denied = bob
        .create_role(authed(
            Request::new(CreateRoleRequest {
                name: "Nope".into(),
                permissions: Permissions::MANAGE_ROLES.bits().to_string(),
                ..Default::default()
            }),
            &b_tok,
        ))
        .await;
    match denied {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] member without MANAGE_ROLES denied creating roles");
        }
        other => anyhow::bail!("expected PermissionDenied, got {other:?}"),
    }

    // Owner creates a Moderator role and assigns it to B.
    let mod_role = owner
        .create_role(authed(
            Request::new(CreateRoleRequest {
                name: "Moderator".into(),
                permissions: Permissions::MANAGE_ROLES.bits().to_string(),
                ..Default::default()
            }),
            &owner_tok,
        ))
        .await?
        .into_inner();
    owner
        .assign_role(authed(
            Request::new(AssignRoleRequest {
                user_id: Some(UserId {
                    value: b_id.clone(),
                }),
                role_id: mod_role.id.clone(),
            }),
            &owner_tok,
        ))
        .await?;
    println!("[ok] owner created 'Moderator' and assigned it to the member");

    // B can now create roles.
    bob.create_role(authed(
        Request::new(CreateRoleRequest {
            name: "Helper".into(),
            permissions: Permissions::SEND_MESSAGES.bits().to_string(),
            ..Default::default()
        }),
        &b_tok,
    ))
    .await?;
    println!("[ok] member can now create roles (permission took effect)");

    // Anti-escalation: B (not admin) cannot create an ADMINISTRATOR role.
    let escalate = bob
        .create_role(authed(
            Request::new(CreateRoleRequest {
                name: "Sneaky Admin".into(),
                permissions: Permissions::ADMINISTRATOR.bits().to_string(),
                ..Default::default()
            }),
            &b_tok,
        ))
        .await;
    match escalate {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] non-admin blocked from creating an ADMINISTRATOR role");
        }
        other => anyhow::bail!("expected PermissionDenied on escalation, got {other:?}"),
    }

    // Owner (admin) is allowed to.
    owner
        .create_role(authed(
            Request::new(CreateRoleRequest {
                name: "Admins".into(),
                permissions: Permissions::ADMINISTRATOR.bits().to_string(),
                ..Default::default()
            }),
            &owner_tok,
        ))
        .await?;
    println!("[ok] owner can create an ADMINISTRATOR role");

    // GetMyPermissions reflects owner status.
    let owner_perms = owner
        .get_my_permissions(authed(Request::new(GetMyPermissionsRequest {}), &owner_tok))
        .await?
        .into_inner();
    assert!(owner_perms.is_owner, "owner should be flagged as owner");
    println!("[ok] GetMyPermissions: owner is_owner=true");

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nROLES & PERMISSIONS (RBAC) WORK ");
    Ok(())
}

async fn connect_with_retry(endpoint: &str) -> anyhow::Result<Channel> {
    for _ in 0..50 {
        if let Ok(ch) = Channel::from_shared(endpoint.to_owned())?.connect().await {
            return Ok(ch);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("server did not start in time")
}
