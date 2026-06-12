//! End-to-end gRPC smoke test for the walking skeleton.
//!
//! Exercises the full public-message pipeline against a *running* server:
//! 1. Register a fresh random user.
//! 2. Log in -> receive access token + auto-join `#general`.
//! 3. List groups -> confirm `#general` is present.
//! 4. Open the bidirectional message stream and send a public message.
//! 5. Receive that message back over the stream (Redis fan-out round-trip).
//! 6. Fetch public history -> confirm the message was persisted.
//!
//! Run with the Docker stack up and the server running:
//! ```text
//! docker compose up -d
//! cargo run -p accord-server # in one terminal
//! cargo run -p accord-server --example smoke # in another
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::messaging_service_client::MessagingServiceClient;
use accord_proto::{
    ClientMessage, FetchHistoryRequest, GroupId, ListGroupsRequest, LoginRequest, MessageId,
    RegisterRequest, SendPublicMessage, client_message::Payload as ClientPayload,
    server_message::Payload as ServerPayload,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

const ENDPOINT: &str = "http://127.0.0.1:50051";

/// Attach `authorization: Bearer <token>` to a request.
fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let value: MetadataValue<_> = format!("Bearer {token}")
        .parse()
        .expect("valid metadata value");
    req.metadata_mut().insert("authorization", value);
    req
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let channel = Channel::from_static(ENDPOINT).connect().await?;

    // --- 1. register ---
    let username = format!("smoke-{}", uuid::Uuid::now_v7());
    let mut auth = AuthServiceClient::new(channel.clone());
    auth.register(RegisterRequest {
        username: username.clone(),
        password: "hunter2pw".into(),
        display_name: "Smoke Tester".into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    println!("[ok] registered {username}");

    // --- 2. login ---
    let login = auth
        .login(LoginRequest {
            username: username.clone(),
            password: "hunter2pw".into(),
            device_name: "smoke-device".into(),
        })
        .await?
        .into_inner();
    let token = login.access_token;
    println!("[ok] logged in; got access token ({} bytes)", token.len());

    // --- 3. list groups -> find #general ---
    let mut groups = GroupServiceClient::new(channel.clone());
    let group_list = groups
        .list_groups(authed(Request::new(ListGroupsRequest {}), &token))
        .await?
        .into_inner();
    let general = group_list
        .groups
        .iter()
        .find(|g| g.name == "general")
        .expect("auto-joined #general should be listed");
    let group_id = general.group_id.clone().expect("group id");
    println!("[ok] found #general: {}", group_id.value);

    // --- 4 & 5. open stream, send a message, receive it back ---
    let mut messaging = MessagingServiceClient::new(channel.clone());
    let (tx, rx) = tokio::sync::mpsc::channel::<ClientMessage>(8);
    let outbound = ReceiverStream::new(rx);

    let mut inbound = messaging
        .message_stream(authed(Request::new(outbound), &token))
        .await?
        .into_inner();

    let body = "hello from the smoke test";
    tx.send(ClientMessage {
        payload: Some(ClientPayload::PublicMessage(SendPublicMessage {
            group_id: Some(group_id.clone()),
            content: body.into(),
            client_message_id: Some(MessageId {
                value: uuid::Uuid::now_v7().to_string(),
            }),
        })),
    })
    .await?;
    println!("[ok] sent public message");

    // Wait (with timeout) for the message to come back via the bus.
    let received = tokio::time::timeout(Duration::from_secs(5), inbound.message()).await??;
    match received.and_then(|m| m.payload) {
        Some(ServerPayload::PublicMessage(m)) if m.content == body => {
            println!("[ok] received message back over stream: {:?}", m.content);
        }
        other => anyhow::bail!("unexpected stream message: {other:?}"),
    }

    // Drop the sender so the server-side reader task ends and deregisters us.
    drop(tx);

    // --- 6. history ---
    let history = messaging
        .fetch_public_history(authed(
            Request::new(FetchHistoryRequest {
                group_id: Some(GroupId {
                    value: group_id.value.clone(),
                }),
                before_sequence: 0,
                limit: 10,
            }),
            &token,
        ))
        .await?
        .into_inner();
    assert!(
        history.messages.iter().any(|m| m.content == body),
        "sent message should appear in history"
    );
    println!(
        "[ok] message found in public history ({} total)",
        history.messages.len()
    );

    println!("\nALL CHECKS PASSED ");
    Ok(())
}
