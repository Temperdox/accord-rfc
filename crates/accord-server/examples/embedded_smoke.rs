//! Verifies the **embedded / self-hosted** path the desktop client uses:
//! starts an `accord-server` *in-process* via the library (SQLite + in-process
//! bus, graceful shutdown) and runs a register -> login -> send -> receive round
//! trip against it. No external server, no Docker.
//!
//! ```text
//! cargo run -p accord-server --example embedded_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::messaging_service_client::MessagingServiceClient;
use accord_proto::server_message::Payload as ServerPayload;
use accord_proto::{
    ClientMessage, ListGroupsRequest, LoginRequest, MessageId, RegisterRequest, SendPublicMessage,
    client_message::Payload as ClientPayload,
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

const PORT: u16 = 50061;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Start the server in-process, exactly like the client's hosting module.
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:embedded-smoke.db".to_owned(),
        redis_url: String::new(), // in-process bus
        jwt_secret: "embedded-smoke-secret".to_owned(),
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

    // 2. Wait for it to accept connections.
    let endpoint = format!("http://127.0.0.1:{PORT}");
    let channel = connect_with_retry(&endpoint).await?;
    println!("[ok] embedded server is up on {endpoint}");

    // 3. Register + login.
    let username = format!("embed-{}", uuid::Uuid::now_v7());
    let mut auth = AuthServiceClient::new(channel.clone());
    auth.register(RegisterRequest {
        username: username.clone(),
        password: "embeddedpw".into(),
        display_name: "Embedded".into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let token = auth
        .login(LoginRequest {
            username: username.clone(),
            password: "embeddedpw".into(),
            device_name: "embed".into(),
        })
        .await?
        .into_inner()
        .access_token;
    println!("[ok] registered + logged in");

    // 4. Find #general, open the stream, send a message, receive it back.
    let general = GroupServiceClient::new(channel.clone())
        .list_groups(authed(Request::new(ListGroupsRequest {}), &token))
        .await?
        .into_inner()
        .groups
        .into_iter()
        .find(|g| g.name == "general")
        .expect("#general exists")
        .group_id
        .expect("group id");

    let (tx, rx) = mpsc::channel::<ClientMessage>(8);
    let mut inbound = MessagingServiceClient::new(channel)
        .message_stream(authed(Request::new(ReceiverStream::new(rx)), &token))
        .await?
        .into_inner();

    tx.send(ClientMessage {
        payload: Some(ClientPayload::PublicMessage(SendPublicMessage {
            group_id: Some(general),
            content: "hello from an embedded server".into(),
            client_message_id: Some(MessageId {
                value: uuid::Uuid::now_v7().to_string(),
            }),
        })),
    })
    .await?;

    let received = tokio::time::timeout(Duration::from_secs(5), inbound.message()).await??;
    match received.and_then(|m| m.payload) {
        Some(ServerPayload::PublicMessage(m)) if m.content == "hello from an embedded server" => {
            println!("[ok] round-tripped a message through the embedded server");
        }
        other => anyhow::bail!("unexpected: {other:?}"),
    }

    // 5. Graceful shutdown (what the client does on "Stop host").
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("[ok] embedded server shut down cleanly");

    println!("\nEMBEDDED (CLIENT-HOSTED) SERVER WORKS ");
    Ok(())
}

async fn connect_with_retry(endpoint: &str) -> anyhow::Result<Channel> {
    for _ in 0..50 {
        if let Ok(ch) = Channel::from_shared(endpoint.to_owned())?.connect().await {
            return Ok(ch);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("embedded server did not start in time")
}
