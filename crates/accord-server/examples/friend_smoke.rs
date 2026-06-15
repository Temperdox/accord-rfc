//! Smoke test for the **friend-request parking** flow (friends.proto):
//! 1. Alice parks a request for Bob (carrying her fr code).
//! 2. Re-sending dedupes (upsert), not duplicates.
//! 3. Bob lists it, then accepts: deletes it and parks an `accept` for Alice.
//! 4. Alice lists her side and finds the acceptance (with Bob's code).
//! 5. Cleanup deletes are scoped to the recipient.
//!
//! ```text
//! cargo run -p accord-server --example friend_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::friend_service_client::FriendServiceClient;
use accord_proto::{
    DeleteFriendRequestRequest, GetPublicProfileRequest, ListFriendRequestsRequest, LoginRequest,
    RegisterRequest, SendFriendRequestRequest, UserId,
};
use accord_types::contact::ContactCode;
use tokio::sync::oneshot;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

const PORT: u16 = 50072;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().expect("token");
    req.metadata_mut().insert("authorization", v);
    req
}

async fn make_user(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let mut auth = AuthServiceClient::new(ch.clone());
    auth.register(RegisterRequest {
        username: name.into(),
        password: "friendpw123".into(),
        display_name: name.into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let r = auth
        .login(LoginRequest {
            username: name.into(),
            password: "friendpw123".into(),
            device_name: format!("{name}-dev"),
        })
        .await?
        .into_inner();
    Ok((r.access_token, r.user_id.unwrap().value))
}

fn code_for(name: &str, key_byte: u8) -> String {
    ContactCode::new(vec![key_byte; 32])
        .with_name(Some(name.to_owned()))
        .with_addresses(vec!["https://198.51.100.7:50051".to_owned()])
        .with_host_user_id(Some("00000000-0000-7000-8000-000000000042".to_owned()))
        .encode()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:friend-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "friend-smoke-secret".to_owned(),
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
    let ch = connect_with_retry(&format!("http://127.0.0.1:{PORT}")).await?;

    let (alice_tok, alice_uid) = make_user(&ch, "alice").await?;
    let (bob_tok, bob_uid) = make_user(&ch, "bob").await?;

    // 1 + 2. Alice parks a request for Bob, twice (the second upserts).
    let alice_code = code_for("alice", 7);
    for _ in 0..2 {
        FriendServiceClient::new(ch.clone())
            .send_friend_request(authed(
                Request::new(SendFriendRequestRequest {
                    recipient: Some(UserId {
                        value: bob_uid.clone(),
                    }),
                    contact_code: alice_code.clone(),
                    kind: "request".into(),
                }),
                &alice_tok,
            ))
            .await?;
    }
    println!("[ok] Alice parked a friend request for Bob (re-send upserted)");

    // 3. Bob lists: exactly one request, carrying Alice's code.
    let bob_list = FriendServiceClient::new(ch.clone())
        .list_friend_requests(authed(Request::new(ListFriendRequestsRequest {}), &bob_tok))
        .await?
        .into_inner()
        .requests;
    anyhow::ensure!(
        bob_list.len() == 1,
        "expected 1 request, got {}",
        bob_list.len()
    );
    anyhow::ensure!(bob_list[0].kind == "request");
    anyhow::ensure!(bob_list[0].contact_code == alice_code);
    println!("[ok] Bob sees exactly one parked request with Alice's code");

    // Bob accepts: delete his copy, park an accept for Alice with HIS code.
    FriendServiceClient::new(ch.clone())
        .delete_friend_request(authed(
            Request::new(DeleteFriendRequestRequest {
                id: bob_list[0].id.clone(),
            }),
            &bob_tok,
        ))
        .await?;
    let bob_code = code_for("bob", 9);
    FriendServiceClient::new(ch.clone())
        .send_friend_request(authed(
            Request::new(SendFriendRequestRequest {
                recipient: Some(UserId {
                    value: alice_uid.clone(),
                }),
                contact_code: bob_code.clone(),
                kind: "accept".into(),
            }),
            &bob_tok,
        ))
        .await?;
    println!("[ok] Bob accepted (cleared his copy, parked an accept for Alice)");

    // 4. Alice finds the acceptance with Bob's code.
    let alice_list = FriendServiceClient::new(ch.clone())
        .list_friend_requests(authed(
            Request::new(ListFriendRequestsRequest {}),
            &alice_tok,
        ))
        .await?
        .into_inner()
        .requests;
    anyhow::ensure!(alice_list.len() == 1 && alice_list[0].kind == "accept");
    anyhow::ensure!(alice_list[0].contact_code == bob_code);
    println!("[ok] Alice received the acceptance with Bob's code");

    // 5. Deleting with the wrong recipient is a no-op; the right one clears it.
    FriendServiceClient::new(ch.clone())
        .delete_friend_request(authed(
            Request::new(DeleteFriendRequestRequest {
                id: alice_list[0].id.clone(),
            }),
            &bob_tok,
        ))
        .await?;
    let still_there = FriendServiceClient::new(ch.clone())
        .list_friend_requests(authed(
            Request::new(ListFriendRequestsRequest {}),
            &alice_tok,
        ))
        .await?
        .into_inner()
        .requests;
    anyhow::ensure!(still_there.len() == 1, "delete must be recipient-scoped");
    FriendServiceClient::new(ch.clone())
        .delete_friend_request(authed(
            Request::new(DeleteFriendRequestRequest {
                id: alice_list[0].id.clone(),
            }),
            &alice_tok,
        ))
        .await?;
    println!("[ok] deletes are scoped to the recipient");

    // 7. The public-profile fetch a requester does after delivery: live
    // username + display name (instead of the snapshot in the code).
    let profile = FriendServiceClient::new(ch.clone())
        .get_public_profile(authed(
            Request::new(GetPublicProfileRequest {
                user_id: Some(UserId {
                    value: bob_uid.clone(),
                }),
            }),
            &alice_tok,
        ))
        .await?
        .into_inner();
    anyhow::ensure!(profile.username == "bob", "profile username");
    anyhow::ensure!(profile.display_name == "bob", "profile display name");
    let missing = FriendServiceClient::new(ch.clone())
        .get_public_profile(authed(
            Request::new(GetPublicProfileRequest {
                user_id: Some(UserId {
                    value: "00000000-0000-7000-8000-00000000dead".to_owned(),
                }),
            }),
            &alice_tok,
        ))
        .await;
    anyhow::ensure!(missing.is_err(), "unknown user must be NotFound");
    println!("[ok] public profile fetch works (and 404s on unknown users)");

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nFRIEND REQUEST PARKING WORKS");
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
