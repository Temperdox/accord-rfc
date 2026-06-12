//! End-to-end smoke for **cross-user DMs via open_dms** (federation phase 3).
//!
//! Bob runs a *private* home server (`require_invite = true`). Alice is not a
//! member and has no invite - she is a contact who wants to DM Bob. Because the
//! host has `open_dms = true`, Alice can register as a guest *over the LAN*
//! (non-loopback, so the device-owner bypass does not apply - this exercises the
//! open_dms path), then set up a real MLS DM with Bob and exchange an encrypted
//! message. This is the recipient-hosts-the-DM model from FEDERATION-PLAN.md.
//!
//! Self-contained (spawns the server):
//! ```text
//! cargo run -p accord-server --example contact_dm_smoke
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

const PORT: u16 = 50070;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().expect("token");
    req.metadata_mut().insert("authorization", v);
    req
}

async fn register(ch: &Channel, name: &str) -> Result<(), tonic::Status> {
    AuthServiceClient::new(ch.clone())
        .register(RegisterRequest {
            username: name.into(),
            password: "contactpw123".into(),
            display_name: name.into(),
            invite_token: String::new(),
            identity_pubkey: Vec::new(),
        })
        .await
        .map(|_| ())
}

async fn login(ch: &Channel, name: &str) -> anyhow::Result<(String, String)> {
    let r = AuthServiceClient::new(ch.clone())
        .login(LoginRequest {
            username: name.into(),
            password: "contactpw123".into(),
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
    let inbound = MessagingServiceClient::new(ch.clone())
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Bob's private home server, but open to DMs from contacts.
    let config = accord_server::Config {
        bind_addr: format!("0.0.0.0:{PORT}").parse()?,
        database_url: "sqlite:contact-dm-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "contact-dm-secret".to_owned(),
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

    // Connect over the LAN address so registrations are NOT loopback (which would
    // be treated as the device owner and bypass the gate); this tests open_dms.
    let lan_ip = local_ip_address::local_ip()
        .map_err(|e| anyhow::anyhow!("need a non-loopback LAN address for this test: {e}"))?;
    let ch = connect_with_retry(&format!("http://{lan_ip}:{PORT}")).await?;

    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let bob_name = format!("bob-{suffix}");
    let alice_name = format!("alice-{suffix}");

    // Bob is the owner (first user, no invite needed).
    register(&ch, &bob_name).await?;
    let (bob_tok, _bob_uid) = login(&ch, &bob_name).await?;
    println!("[ok] Bob registered as owner of his private home server");

    // Alice is a non-member contact with NO invite. require_invite is on, but
    // open_dms lets her register to start a DM (this is the cross-user enabler).
    register(&ch, &alice_name)
        .await
        .map_err(|s| anyhow::anyhow!("open_dms should allow a contact to register: {s}"))?;
    let (alice_tok, _alice_uid) = login(&ch, &alice_name).await?;
    println!("[ok] Alice (contact, no invite) registered via open_dms");

    // MLS engines.
    let mut alice_mls = MlsEngine::new(&accord_mls::IdentityKeyPair::generate())?;
    let mut bob_mls = MlsEngine::new(&accord_mls::IdentityKeyPair::generate())?;

    // Bob publishes KeyPackages so a contact can add him.
    let bob_kps = bob_mls.generate_key_packages(3)?;
    MlsServiceClient::new(ch.clone())
        .upload_key_packages(authed(
            Request::new(UploadKeyPackagesRequest {
                key_packages: bob_kps,
            }),
            &bob_tok,
        ))
        .await?;

    let (_bob_tx, mut bob_in) = open_stream(&ch, &bob_tok).await?;
    let (alice_tx, _alice_in) = open_stream(&ch, &alice_tok).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Alice resolves Bob and fetches a KeyPackage per device.
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

    // Alice creates the DM group on Bob's host and adds Bob.
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
    println!("[ok] Alice opened a DM with Bob on Bob's host");

    let welcome_bytes = wait_for(&mut bob_in, |p| match p {
        ServerPayload::WelcomeNotification(w) => Some(w.welcome),
        _ => None,
    })
    .await?;
    bob_mls.join_from_welcome(&welcome_bytes)?;

    let ct = alice_mls.encrypt(&mls_id(&group_id), b"hi bob, we're not on the same server")?;
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
            anyhow::ensure!(
                pt == b"hi bob, we're not on the same server",
                "wrong plaintext"
            );
            println!("[ok] Bob decrypted the contact DM");
        }
        other => anyhow::bail!("expected application message, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nCROSS-USER DM (open_dms) WORKS");
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
