//! Verifies **guest isolation** (open_dms accounts are DM-only) end-to-end:
//! 1. Private server (`require_invite`, `open_dms`); first account = owner.
//! 2. A remote, inviteless registration succeeds - but as a **guest**.
//! 3. The guest is NOT auto-joined to `#general` (empty group list).
//! 4. The guest cannot add themselves to `#general` (add_members denied).
//! 5. The guest cannot read `#general` history (not a member).
//! 6. The guest cannot mint an invite (zero permissions).
//! 7. A user registering WITH an invite is a full member (sees `#general`).
//! 8. The per-IP remote registration rate limit kicks in.
//!
//! Connects over the LAN address (loopback would be the device-owner bypass).
//!
//! ```text
//! cargo run -p accord-server --example guest_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::messaging_service_client::MessagingServiceClient;
use accord_proto::{
    AddMembersRequest, CreateInviteRequest, FetchHistoryRequest, GroupId, ListGroupsRequest,
    LoginRequest, RegisterRequest, UserId,
};
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50071;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().expect("token");
    req.metadata_mut().insert("authorization", v);
    req
}

async fn register(ch: &Channel, name: &str, invite: &str) -> Result<(), tonic::Status> {
    AuthServiceClient::new(ch.clone())
        .register(RegisterRequest {
            username: name.into(),
            password: "guestpw123".into(),
            display_name: name.into(),
            invite_token: invite.into(),
            identity_pubkey: Vec::new(),
        })
        .await
        .map(|_| ())
}

async fn login(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let r = AuthServiceClient::new(ch.clone())
        .login(LoginRequest {
            username: name.into(),
            password: "guestpw123".into(),
            device_name: format!("{name}-dev"),
        })
        .await?
        .into_inner();
    Ok((r.access_token, r.user_id.unwrap().value))
}

async fn list_group_names(ch: &Channel, token: &str) -> anyhow::Result<Vec<(String, String)>> {
    let groups = GroupServiceClient::new(ch.clone())
        .list_groups(authed(Request::new(ListGroupsRequest {}), token))
        .await?
        .into_inner()
        .groups;
    Ok(groups
        .into_iter()
        .map(|g| (g.group_id.map(|x| x.value).unwrap_or_default(), g.name))
        .collect())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("0.0.0.0:{PORT}").parse()?,
        database_url: "sqlite:guest-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "guest-smoke-secret".to_owned(),
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: true,
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

    let lan_ip = local_ip_address::local_ip()
        .map_err(|e| anyhow::anyhow!("need a non-loopback LAN address for this test: {e}"))?;
    let ch = connect_with_retry(&format!("http://{lan_ip}:{PORT}")).await?;

    // 1-2. Owner, then an inviteless remote registration (becomes a guest).
    register(&ch, "owner", "").await?;
    let (owner_tok, _) = login(&ch, "owner").await?;
    register(&ch, "guest", "")
        .await
        .map_err(|s| anyhow::anyhow!("open_dms should admit a guest: {s}"))?;
    let (guest_tok, guest_uid) = login(&ch, "guest").await?;
    println!("[ok] owner registered; inviteless remote registration admitted as guest");

    // The owner sees #general; grab its id for the guest's attempts below.
    let owner_groups = list_group_names(&ch, &owner_tok).await?;
    let general_id = owner_groups
        .iter()
        .find(|(_, name)| name == "general")
        .map(|(id, _)| id.clone())
        .ok_or_else(|| anyhow::anyhow!("owner should see #general"))?;

    // 3. The guest is in NO groups (no #general auto-join).
    let guest_groups = list_group_names(&ch, &guest_tok).await?;
    anyhow::ensure!(
        guest_groups.is_empty(),
        "guest should see no groups, saw {guest_groups:?}"
    );
    println!("[ok] guest sees no channels (no #general auto-join)");

    // 4. The guest cannot add themselves to #general.
    match GroupServiceClient::new(ch.clone())
        .add_members(authed(
            Request::new(AddMembersRequest {
                group_id: Some(GroupId {
                    value: general_id.clone(),
                }),
                member_ids: vec![UserId {
                    value: guest_uid.clone(),
                }],
            }),
            &guest_tok,
        ))
        .await
    {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] guest self-join to #general rejected");
        }
        other => anyhow::bail!("expected PermissionDenied on self-join, got {other:?}"),
    }

    // 5. The guest cannot read #general history.
    match MessagingServiceClient::new(ch.clone())
        .fetch_public_history(authed(
            Request::new(FetchHistoryRequest {
                group_id: Some(GroupId {
                    value: general_id.clone(),
                }),
                before_sequence: 0,
                limit: 10,
            }),
            &guest_tok,
        ))
        .await
    {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] guest cannot read #general history");
        }
        other => anyhow::bail!("expected PermissionDenied on history, got {other:?}"),
    }

    // 6. The guest cannot mint an invite (no permissions at all).
    match AuthServiceClient::new(ch.clone())
        .create_invite(authed(Request::new(CreateInviteRequest {}), &guest_tok))
        .await
    {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] guest cannot mint an invite (no permission escalation)");
        }
        other => anyhow::bail!("expected PermissionDenied on create_invite, got {other:?}"),
    }

    // 7. An invited registration is a full member and sees #general.
    let invite = AuthServiceClient::new(ch.clone())
        .create_invite(authed(Request::new(CreateInviteRequest {}), &owner_tok))
        .await?
        .into_inner()
        .token;
    register(&ch, "member", &invite).await?;
    let (member_tok, _) = login(&ch, "member").await?;
    let member_groups = list_group_names(&ch, &member_tok).await?;
    anyhow::ensure!(
        member_groups.iter().any(|(_, n)| n == "general"),
        "invited member should see #general"
    );
    println!("[ok] invited member is a full member (sees #general)");

    // 8. Rate limit: registrations 1-3 used the window; two fillers reach the
    //    cap of 5, then the next remote attempt is rejected.
    register(&ch, "filler-a", "").await?;
    register(&ch, "filler-b", "").await?;
    match register(&ch, "one-too-many", "").await {
        Err(s) if s.code() == Code::ResourceExhausted => {
            println!("[ok] remote registration rate limit enforced");
        }
        other => anyhow::bail!("expected ResourceExhausted, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nGUEST ISOLATION WORKS");
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
