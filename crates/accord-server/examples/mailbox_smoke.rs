//! End-to-end smoke test for the **offline mailbox** (federation phase 2).
//!
//! Alice and Bob set up an MLS DM. Bob then disconnects. Alice sends a private
//! message while Bob is offline; the server queues it in Bob's mailbox. Bob
//! reconnects, drains the mailbox, and decrypts the message - proving private
//! messages survive an offline recipient instead of being dropped.
//!
//! Self-contained (spawns the server in-process):
//! ```text
//! cargo run -p accord-server --example mailbox_smoke
//! ```

use std::collections::HashMap;
use std::time::Duration;

use accord_mls::{DecryptOutcome, MlsEngine};
use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::messaging_service_client::MessagingServiceClient;
use accord_proto::mls_service_client::MlsServiceClient;
use accord_proto::server_message::Payload as ServerPayload;
use accord_proto::{
    ClientMessage, CreatePrivateGroupRequest, FetchKeyPackagesRequest, GroupId, LoginRequest,
    LookupUserRequest, MessageId, RegisterRequest, SendPrivateMessage, ServerMessage,
    UploadKeyPackagesRequest, UserId, WelcomeTarget, client_message::Payload as ClientPayload,
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Request, Streaming};

const PORT: u16 = 50069;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().expect("token");
    req.metadata_mut().insert("authorization", v);
    req
}

async fn make_user(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let mut auth = AuthServiceClient::new(ch.clone());
    auth.register(RegisterRequest {
        username: name.into(),
        password: "mailboxpw1".into(),
        display_name: name.into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let r = auth
        .login(LoginRequest {
            username: name.into(),
            password: "mailboxpw1".into(),
            device_name: format!("{name}-dev"),
        })
        .await?
        .into_inner();
    Ok((r.access_token, r.user_id.unwrap().value))
}

async fn open_stream(
    ch: &Channel,
    token: &str,
) -> anyhow::Result<(mpsc::Sender<ClientMessage>, Streaming<ServerMessage>)> {
    let (tx, rx) = mpsc::channel::<ClientMessage>(8);
    let mut client = MessagingServiceClient::new(ch.clone());
    let inbound = client
        .message_stream(authed(Request::new(ReceiverStream::new(rx)), token))
        .await?
        .into_inner();
    Ok((tx, inbound))
}

async fn wait_for<T>(
    inbound: &mut Streaming<ServerMessage>,
    mut f: impl FnMut(ServerPayload) -> Option<T>,
) -> anyhow::Result<T> {
    let fut = async {
        loop {
            match inbound.message().await? {
                Some(ServerMessage { payload: Some(p) }) => {
                    if let Some(v) = f(p) {
                        return Ok::<T, anyhow::Error>(v);
                    }
                }
                Some(_) => {}
                None => anyhow::bail!("stream closed"),
            }
        }
    };
    Ok(tokio::time::timeout(Duration::from_secs(5), fut).await??)
}

fn mls_id(group_id: &str) -> Vec<u8> {
    uuid::Uuid::parse_str(group_id)
        .expect("uuid")
        .as_bytes()
        .to_vec()
}

fn send_private(group_id: &str, ciphertext: Vec<u8>) -> ClientMessage {
    ClientMessage {
        payload: Some(ClientPayload::PrivateMessage(SendPrivateMessage {
            group_id: Some(GroupId {
                value: group_id.to_owned(),
            }),
            mls_ciphertext: ciphertext,
            epoch: 0,
            client_message_id: Some(MessageId {
                value: uuid::Uuid::now_v7().to_string(),
            }),
        })),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:mailbox-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "mailbox-smoke-secret".to_owned(),
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
    let ch = connect_with_retry(&endpoint).await?;

    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let alice_name = format!("alice-{suffix}");
    let bob_name = format!("bob-{suffix}");
    let (alice_tok, _alice_uid) = make_user(&ch, &alice_name).await?;
    let (bob_tok, _bob_uid) = make_user(&ch, &bob_name).await?;

    let alice_key = accord_mls::IdentityKeyPair::generate();
    let bob_key = accord_mls::IdentityKeyPair::generate();
    let mut alice_mls = MlsEngine::new(&alice_key)?;
    let mut bob_mls = MlsEngine::new(&bob_key)?;

    let bob_kps = bob_mls.generate_key_packages(3)?;
    MlsServiceClient::new(ch.clone())
        .upload_key_packages(authed(
            Request::new(UploadKeyPackagesRequest {
                key_packages: bob_kps,
            }),
            &bob_tok,
        ))
        .await?;

    // Alice connects; Bob stays OFFLINE the whole time, so both his Welcome and
    // the message must travel through his mailbox.
    let (alice_tx, _alice_in) = open_stream(&ch, &alice_tok).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let bob_uid = AuthServiceClient::new(ch.clone())
        .lookup_user(authed(
            Request::new(LookupUserRequest {
                username: bob_name.clone(),
            }),
            &alice_tok,
        ))
        .await?
        .into_inner()
        .user_id
        .unwrap()
        .value;

    let fetched: HashMap<String, _> = MlsServiceClient::new(ch.clone())
        .fetch_key_packages(authed(
            Request::new(FetchKeyPackagesRequest {
                user_ids: vec![UserId {
                    value: bob_uid.clone(),
                }],
            }),
            &alice_tok,
        ))
        .await?
        .into_inner()
        .packages;
    let bundle = fetched.get(&bob_uid).expect("bob has key packages");

    let group_uuid = uuid::Uuid::now_v7();
    let group_id = group_uuid.to_string();
    alice_mls.create_group(group_uuid.as_bytes())?;
    let mut welcomes = Vec::new();
    let mut last_commit = Vec::new();
    for dp in &bundle.device_packages {
        let (commit, welcome) = alice_mls.add_member(group_uuid.as_bytes(), &dp.key_package)?;
        last_commit = commit;
        welcomes.push(WelcomeTarget {
            device_id: dp.device_id.clone(),
            welcome,
        });
    }
    GroupServiceClient::new(ch.clone())
        .create_private_group(authed(
            Request::new(CreatePrivateGroupRequest {
                name: "Alice & Bob".into(),
                member_ids: vec![UserId {
                    value: bob_uid.clone(),
                }],
                initial_commit: last_commit,
                welcomes,
                group_id: Some(GroupId {
                    value: group_id.clone(),
                }),
            }),
            &alice_tok,
        ))
        .await?;

    println!("[ok] Alice created the DM while Bob was offline (Welcome queued)");

    // Alice sends a message while Bob is still offline -> queued in his mailbox.
    let ct = alice_mls.encrypt(&mls_id(&group_id), b"sent while you were away")?;
    alice_tx.send(send_private(&group_id, ct)).await?;
    println!("[ok] Alice sent a message while Bob was offline");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Bob comes online for the first time; the drain delivers the queued Welcome
    // (he joins) then the queued message (he decrypts).
    let (_bob_tx, mut bob_in) = open_stream(&ch, &bob_tok).await?;
    let welcome_bytes = wait_for(&mut bob_in, |p| match p {
        ServerPayload::WelcomeNotification(w) => Some(w.welcome),
        _ => None,
    })
    .await?;
    bob_mls.join_from_welcome(&welcome_bytes)?;
    println!("[ok] Bob came online and joined from the queued Welcome");

    let ct_in = wait_for(&mut bob_in, |p| match p {
        ServerPayload::PrivateMessage(m) => Some(m.mls_ciphertext),
        _ => None,
    })
    .await?;
    match bob_mls.process_incoming(&mls_id(&group_id), &ct_in)? {
        DecryptOutcome::Application(pt) => {
            anyhow::ensure!(pt == b"sent while you were away", "wrong plaintext");
            println!("[ok] Bob drained the mailbox and decrypted the offline message");
        }
        other => anyhow::bail!("expected application message, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nOFFLINE MAILBOX WORKS");
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
