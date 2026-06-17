//! Headless integration test for the client's taverns plumbing.
//!
//! This is a `#[cfg(test)]` module compiled into the binary (the crate is
//! bin-only, so a `tests/` integration crate can't reach internal modules). It
//! drives the SAME client-side code the Tauri commands use - `grpc::build_channel`
//! / `grpc::authed` and the `dto::GroupDto` mapping - against a real in-process
//! `accord-server`, so it covers the client gRPC layer + DTO conversions that the
//! server-side `taverns_smoke` example cannot.
//!
//! NOTE: the client binary carries a `requireAdministrator` manifest, so the test
//! binary only runs in an ELEVATED shell (otherwise `os error 740`). Run with:
//! `cargo test -p accord-client taverns_it -- --nocapture` from an admin terminal.

#![cfg(test)]

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::{
    CreatePublicGroupRequest, GetTavernRequest, GroupId, ListGroupsRequest, ListMembersRequest,
    LoginRequest, RegisterRequest, UpdateTavernRequest,
};
use tokio::sync::oneshot;
use tonic::Request;

use crate::commands::dto::GroupDto;
use crate::grpc::{authed, build_channel};

const PORT: u16 = 50071;

async fn start_server() -> oneshot::Sender<()> {
    let db = std::env::temp_dir().join(format!("accord-client-it-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse().expect("addr"),
        database_url: format!("sqlite:{}", db.to_string_lossy().replace('\\', "/")),
        redis_url: String::new(),
        jwt_secret: "client-it-secret".to_owned(),
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: false,
        open_dms: true,
        tls_cert_pem: None,
        tls_key_pem: None,
    };
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = accord_server::run_with_shutdown(config, rx).await;
    });
    tx
}

async fn channel() -> tonic::transport::Channel {
    let endpoint = format!("http://127.0.0.1:{PORT}");
    for _ in 0..50 {
        if let Ok(ch) = build_channel(&endpoint, None).await {
            return ch;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("server did not start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_taverns_roundtrip() {
    let shutdown = start_server().await;
    let ch = channel().await;

    // Register + login the owner via the client's gRPC stack.
    let mut auth = AuthServiceClient::new(ch.clone());
    auth.register(RegisterRequest {
        username: "owner".into(),
        password: "clientit123".into(),
        display_name: "Owner".into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await
    .expect("register");
    let token = auth
        .login(LoginRequest {
            username: "owner".into(),
            password: "clientit123".into(),
            device_name: "dev".into(),
        })
        .await
        .expect("login")
        .into_inner()
        .access_token;

    let mut groups = GroupServiceClient::new(ch.clone());

    // Create a text and a voice channel (the commands' create_channel maps to this).
    for (name, kind) in [("general", "text"), ("Lounge", "voice")] {
        groups
            .create_public_group(authed(
                Request::new(CreatePublicGroupRequest {
                    name: name.into(),
                    description: String::new(),
                    channel_kind: kind.into(),
                }),
                &token,
            ).expect("auth"))
            .await
            .expect("create channel");
    }

    // list_groups + the client DTO mapping must preserve channel_kind.
    let listed = groups
        .list_groups(authed(Request::new(ListGroupsRequest {}), &token).expect("auth"))
        .await
        .expect("list")
        .into_inner()
        .groups;
    let dtos: Vec<GroupDto> = listed.into_iter().map(GroupDto::from_summary).collect();
    let lounge = dtos.iter().find(|g| g.name == "Lounge").expect("voice channel listed");
    assert_eq!(lounge.channel_kind, "voice", "voice channel_kind via client DTO");
    let general = dtos.iter().find(|g| g.name == "general").expect("text channel listed");
    assert_eq!(general.channel_kind, "text", "text channel_kind via client DTO");

    // ListMembers shows the owner.
    let members = groups
        .list_members(authed(
            Request::new(ListMembersRequest {
                group_id: Some(GroupId { value: general.id.clone() }),
            }),
            &token,
        ).expect("auth"))
        .await
        .expect("members")
        .into_inner()
        .members;
    assert!(members.iter().any(|m| m.is_owner), "owner in member list");

    // Tavern identity update/get round-trips.
    groups
        .update_tavern(authed(
            Request::new(UpdateTavernRequest {
                name: "Client IT Tavern".into(),
                icon_url: String::new(),
                description: String::new(),
            }),
            &token,
        ).expect("auth"))
        .await
        .expect("update tavern");
    let tavern = groups
        .get_tavern(authed(Request::new(GetTavernRequest {}), &token).expect("auth"))
        .await
        .expect("get tavern")
        .into_inner();
    assert_eq!(tavern.name, "Client IT Tavern");

    let _ = shutdown.send(());
}
