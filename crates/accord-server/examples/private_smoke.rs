//! End-to-end smoke test for **private (MLS) chat** through the real server.
//!
//! Drives two actual `accord_mls::MlsEngine`s (Alice, Bob) over the live gRPC
//! relay, proving the whole private path: KeyPackage publish/fetch -> group
//! create + Welcome relay -> encrypted send -> decrypt, in both directions. The
//! server only ever handles opaque ciphertext.
//!
//! Run with the stack up and the server running:
//! ```text
//! cargo run -p accord-server --example private_smoke
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
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Request, Streaming};

const ENDPOINT: &str = "http://127.0.0.1:50051";

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().expect("token");
    req.metadata_mut().insert("authorization", v);
    req
}

/// Register + login, returning (token, user_id, device_id).
async fn make_user(ch: &Channel, name: &str) -> anyhow::Result<(String, String, String)> {
    let mut auth = AuthServiceClient::new(ch.clone());
    auth.register(RegisterRequest {
        username: name.into(),
        password: "privatepw1".into(),
        display_name: name.into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let r = auth
        .login(LoginRequest {
            username: name.into(),
            password: "privatepw1".into(),
            device_name: format!("{name}-dev"),
        })
        .await?
        .into_inner();
    Ok((
        r.access_token,
        r.user_id.unwrap().value,
        r.device_id.unwrap().value,
    ))
}

/// Open a bidirectional stream; return (sender, inbound).
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

/// Read messages until one matches `f`, or time out.
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let ch = Channel::from_static(ENDPOINT).connect().await?;

    // Unique usernames per run.
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let alice_name = format!("alice-{suffix}");
    let bob_name = format!("bob-{suffix}");

    let (alice_tok, _alice_uid, _alice_dev) = make_user(&ch, &alice_name).await?;
    let (bob_tok, _bob_uid, _bob_dev) = make_user(&ch, &bob_name).await?;
    println!("[ok] registered {alice_name} and {bob_name}");

    // Each party has its own MLS engine, signing with its own identity key.
    let alice_key = accord_mls::IdentityKeyPair::generate();
    let bob_key = accord_mls::IdentityKeyPair::generate();
    let mut alice_mls = MlsEngine::new(&alice_key)?;
    let mut bob_mls = MlsEngine::new(&bob_key)?;

    // Bob publishes KeyPackages so Alice can add him.
    let bob_kps = bob_mls.generate_key_packages(3)?;
    MlsServiceClient::new(ch.clone())
        .upload_key_packages(authed(
            Request::new(UploadKeyPackagesRequest {
                key_packages: bob_kps,
            }),
            &bob_tok,
        ))
        .await?;
    println!("[ok] Bob uploaded KeyPackages");

    // Both open their streams (Bob must be connected before group creation so he
    // receives the Welcome live and gets subscribed).
    let (_bob_tx, mut bob_in) = open_stream(&ch, &bob_tok).await?;
    let (alice_tx, mut alice_in) = open_stream(&ch, &alice_tok).await?;
    // Give the server a moment to register both streams.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Alice resolves Bob and fetches a KeyPackage per Bob device.
    let bob_lookup = AuthServiceClient::new(ch.clone())
        .lookup_user(authed(
            Request::new(LookupUserRequest {
                username: bob_name.clone(),
            }),
            &alice_tok,
        ))
        .await?
        .into_inner();
    let bob_uid = bob_lookup.user_id.unwrap().value;

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
    println!(
        "[ok] Alice fetched {} device KeyPackage(s)",
        bundle.device_packages.len()
    );

    // Alice creates the MLS group and adds Bob's device(s).
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

    // Register the private group on the server (relays Welcomes to Bob).
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
    println!("[ok] Alice created private group {group_id}");

    // Bob receives the Welcome and joins.
    let welcome_bytes = wait_for(&mut bob_in, |p| match p {
        ServerPayload::WelcomeNotification(w) => Some(w.welcome),
        _ => None,
    })
    .await?;
    bob_mls.join_from_welcome(&welcome_bytes)?;
    println!("[ok] Bob joined from Welcome");

    // Alice -> Bob.
    let ct = alice_mls.encrypt(&mls_id(&group_id), b"hello bob (encrypted)")?;
    alice_tx
        .send(ClientMessage {
            payload: Some(ClientPayload::PrivateMessage(SendPrivateMessage {
                group_id: Some(GroupId {
                    value: group_id.clone(),
                }),
                mls_ciphertext: ct,
                epoch: 0,
                client_message_id: Some(MessageId {
                    value: uuid::Uuid::now_v7().to_string(),
                }),
            })),
        })
        .await?;

    let ct_in = wait_for(&mut bob_in, |p| match p {
        ServerPayload::PrivateMessage(m) => Some(m.mls_ciphertext),
        _ => None,
    })
    .await?;
    match bob_mls.process_incoming(&mls_id(&group_id), &ct_in)? {
        DecryptOutcome::Application(pt) => {
            assert_eq!(pt, b"hello bob (encrypted)");
            println!("[ok] Bob decrypted Alice's message");
        }
        other => anyhow::bail!("expected application message, got {other:?}"),
    }

    // Bob -> Alice.
    let ct = bob_mls.encrypt(&mls_id(&group_id), b"hi alice (encrypted)")?;
    _bob_tx
        .send(ClientMessage {
            payload: Some(ClientPayload::PrivateMessage(SendPrivateMessage {
                group_id: Some(GroupId {
                    value: group_id.clone(),
                }),
                mls_ciphertext: ct,
                epoch: 0,
                client_message_id: Some(MessageId {
                    value: uuid::Uuid::now_v7().to_string(),
                }),
            })),
        })
        .await?;

    let ct_in = wait_for(&mut alice_in, |p| match p {
        ServerPayload::PrivateMessage(m) => Some(m.mls_ciphertext),
        _ => None,
    })
    .await?;
    match alice_mls.process_incoming(&mls_id(&group_id), &ct_in)? {
        DecryptOutcome::Application(pt) => {
            assert_eq!(pt, b"hi alice (encrypted)");
            println!("[ok] Alice decrypted Bob's message");
        }
        other => anyhow::bail!("expected application message, got {other:?}"),
    }

    println!("\nPRIVATE (MLS) CHAT WORKS END-TO-END ");
    Ok(())
}
