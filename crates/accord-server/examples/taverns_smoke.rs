//! Verifies the **taverns** server surface end-to-end (headless):
//! 1. Owner + member register.
//! 2. Owner creates a text channel and a voice channel; `channel_kind` survives
//!    `ListGroups`.
//! 3. `ListMembers` reports the owner with `is_owner = true`.
//! 4. Voice scaffold: both join a voice channel over `MessageStream`; the owner
//!    receives the member's `VoiceParticipant` fan-out.
//! 5. Guardrails bind admins: a non-owner ADMINISTRATOR is throttled on a rapid
//!    burst of destructive deletes (4th delete → RESOURCE_EXHAUSTED).
//! 6. Tavern identity round-trips (`UpdateTavern` -> `GetTavern`).
//! 7. Ban works: a banned account is refused at login.
//!
//! ```text
//! cargo run -p accord-server --example taverns_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::messaging_service_client::MessagingServiceClient;
use accord_proto::role_service_client::RoleServiceClient;
use accord_proto::server_message::Payload as ServerPayload;
use accord_proto::{
    AddMembersRequest, AssignRoleRequest, BanMemberRequest, ClientMessage, CreatePublicGroupRequest,
    CreateRoleRequest, DeleteGroupRequest, GetTavernRequest, GroupId, ListGroupsRequest,
    ListMembersRequest, LoginRequest, RegisterRequest, UpdateTavernRequest, UserId, VoiceStateUpdate,
};
use accord_proto::client_message::Payload as ClientPayload;
use accord_types::perms::Permissions;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50064;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

async fn register(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let mut auth = AuthServiceClient::new(ch.clone());
    auth.register(RegisterRequest {
        username: name.into(),
        password: "tavernpw123".into(),
        display_name: name.into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    login(ch, name).await
}

async fn login(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let mut auth = AuthServiceClient::new(ch.clone());
    let r = auth
        .login(LoginRequest {
            username: name.into(),
            password: "tavernpw123".into(),
            device_name: "dev".into(),
        })
        .await?
        .into_inner();
    Ok((r.access_token, r.user_id.unwrap().value))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Fresh DB each run so the smoke is deterministic.
    let _ = std::fs::remove_file("taverns-smoke.db");
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:taverns-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "taverns-smoke-secret".to_owned(),
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

    let (owner_tok, _owner_id) = register(&channel, "owner").await?;
    let (bob_tok, bob_id) = register(&channel, "bob").await?;
    println!("[ok] owner + member registered");

    let mut owner = GroupServiceClient::new(channel.clone());

    // --- channels + channel_kind ---
    owner
        .create_public_group(authed(
            Request::new(CreatePublicGroupRequest {
                name: "general".into(),
                description: "chat".into(),
                channel_kind: "text".into(),
            }),
            &owner_tok,
        ))
        .await?;
    let lounge = owner
        .create_public_group(authed(
            Request::new(CreatePublicGroupRequest {
                name: "Lounge".into(),
                description: String::new(),
                channel_kind: "voice".into(),
            }),
            &owner_tok,
        ))
        .await?
        .into_inner()
        .group_id
        .unwrap()
        .value;

    let groups = owner
        .list_groups(authed(Request::new(ListGroupsRequest {}), &owner_tok))
        .await?
        .into_inner()
        .groups;
    let voice = groups
        .iter()
        .find(|g| g.name == "Lounge")
        .expect("Lounge listed");
    anyhow::ensure!(voice.channel_kind == "voice", "voice channel_kind preserved");
    println!("[ok] text + voice channels created; channel_kind round-trips");

    // --- member list ---
    let members = owner
        .list_members(authed(
            Request::new(ListMembersRequest {
                group_id: Some(GroupId { value: lounge.clone() }),
            }),
            &owner_tok,
        ))
        .await?
        .into_inner()
        .members;
    anyhow::ensure!(
        members.iter().any(|m| m.is_owner),
        "owner present in member list with is_owner"
    );
    println!("[ok] ListMembers reports the owner");

    // --- voice scaffold: member joins, owner sees the participant fan-out ---
    // Bob self-joins the (public, open-join) voice channel first.
    GroupServiceClient::new(channel.clone())
        .add_members(authed(
            Request::new(AddMembersRequest {
                group_id: Some(GroupId { value: lounge.clone() }),
                member_ids: vec![UserId { value: bob_id.clone() }],
            }),
            &bob_tok,
        ))
        .await?;

    // One bidirectional stream per principal (the hub keys subscriptions by
    // device, so a principal uses a single stream for send + receive).
    let (owner_tx, mut owner_inbound) = open_stream(&channel, &owner_tok).await?;
    let (bob_tx, _bob_inbound) = open_stream(&channel, &bob_tok).await?;
    // Owner joins voice, then Bob joins; the owner should observe Bob's join.
    owner_tx.send(voice_join(&lounge)).await.ok();
    bob_tx.send(voice_join(&lounge)).await.ok();

    let saw_participant = wait_for_voice_participant(&mut owner_inbound, &bob_id).await;
    anyhow::ensure!(saw_participant, "owner received a VoiceParticipant for the joiner");
    println!("[ok] voice participant fan-out delivered over MessageStream");

    // --- guardrails bind admins: throttle a destructive burst ---
    // Make Bob a (non-owner) ADMINISTRATOR.
    let admin_role = RoleServiceClient::new(channel.clone())
        .create_role(authed(
            Request::new(CreateRoleRequest {
                name: "Admins".into(),
                permissions: Permissions::ADMINISTRATOR.bits().to_string(),
            }),
            &owner_tok,
        ))
        .await?
        .into_inner();
    RoleServiceClient::new(channel.clone())
        .assign_role(authed(
            Request::new(AssignRoleRequest {
                user_id: Some(UserId { value: bob_id.clone() }),
                role_id: admin_role.id,
            }),
            &owner_tok,
        ))
        .await?;

    let mut bob = GroupServiceClient::new(channel.clone());
    // Bob (admin) creates 4 channels (additive budget is generous).
    let mut ids = Vec::new();
    for i in 0..4 {
        let id = bob
            .create_public_group(authed(
                Request::new(CreatePublicGroupRequest {
                    name: format!("temp-{i}"),
                    description: String::new(),
                    channel_kind: "text".into(),
                }),
                &bob_tok,
            ))
            .await?
            .into_inner()
            .group_id
            .unwrap()
            .value;
        ids.push(id);
    }
    // Destructive budget is 3 in a burst; the 4th rapid delete must be throttled,
    // proving guardrails apply even to an ADMINISTRATOR.
    let mut throttled = false;
    for id in &ids {
        let res = bob
            .delete_group(authed(
                Request::new(DeleteGroupRequest {
                    group_id: Some(GroupId { value: id.clone() }),
                }),
                &bob_tok,
            ))
            .await;
        if let Err(s) = res {
            anyhow::ensure!(
                s.code() == Code::ResourceExhausted,
                "expected throttle, got {s:?}"
            );
            throttled = true;
            break;
        }
    }
    anyhow::ensure!(throttled, "admin was throttled on a destructive burst");
    println!("[ok] guardrails throttled a non-owner ADMINISTRATOR's destructive burst");

    // --- tavern identity ---
    owner
        .update_tavern(authed(
            Request::new(UpdateTavernRequest {
                name: "The Rusty Tankard".into(),
                icon_url: String::new(),
                description: "a cozy place".into(),
            }),
            &owner_tok,
        ))
        .await?;
    let tavern = owner
        .get_tavern(authed(Request::new(GetTavernRequest {}), &owner_tok))
        .await?
        .into_inner();
    anyhow::ensure!(tavern.name == "The Rusty Tankard", "tavern name round-trips");
    println!("[ok] tavern identity update/get round-trips");

    // --- ban: a banned account is refused at login ---
    owner
        .ban_member(authed(
            Request::new(BanMemberRequest {
                user_id: Some(UserId { value: bob_id.clone() }),
                reason: "smoke test".into(),
            }),
            &owner_tok,
        ))
        .await?;
    match login(&channel, "bob").await {
        Err(_) => println!("[ok] banned account refused at login"),
        Ok(_) => anyhow::bail!("banned account was allowed to log in"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nTAVERNS (channels + members + voice scaffold + guardrails + bans) WORK");
    Ok(())
}

fn voice_join(group_id: &str) -> ClientMessage {
    ClientMessage {
        payload: Some(ClientPayload::VoiceState(VoiceStateUpdate {
            group_id: Some(GroupId { value: group_id.to_owned() }),
            joined: true,
            muted: false,
            camera_on: false,
            screen_on: false,
        })),
    }
}

/// Open a `MessageStream`, returning both the outbound sender and the inbound
/// (server->client) stream. The caller holds `tx` to keep the stream alive.
async fn open_stream(
    ch: &Channel,
    token: &str,
) -> anyhow::Result<(
    mpsc::Sender<ClientMessage>,
    tonic::Streaming<accord_proto::ServerMessage>,
)> {
    let (tx, rx) = mpsc::channel::<ClientMessage>(8);
    let resp = MessagingServiceClient::new(ch.clone())
        .message_stream(authed(Request::new(ReceiverStream::new(rx)), token))
        .await?;
    Ok((tx, resp.into_inner()))
}

/// Wait (briefly) for a `VoiceParticipant` event for `user_id`.
async fn wait_for_voice_participant(
    inbound: &mut tonic::Streaming<accord_proto::ServerMessage>,
    user_id: &str,
) -> bool {
    let deadline = tokio::time::sleep(Duration::from_secs(4));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return false,
            msg = inbound.message() => match msg {
                Ok(Some(m)) => {
                    if let Some(ServerPayload::VoiceParticipant(p)) = m.payload {
                        if p.user_id.map(|u| u.value).as_deref() == Some(user_id) && p.joined {
                            return true;
                        }
                    }
                }
                _ => return false,
            }
        }
    }
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
