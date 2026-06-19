//! # accord-server (library)
//!
//! The Accord gRPC server as a **library**, so it can be run two ways:
//! * as the standalone `accord-server` binary (see `main.rs`), or
//! * **embedded in-process** by the desktop client for self-hosting - the client
//! calls [`run_with_shutdown`] on a background task to host a server with no
//! external binary, no Docker, nothing to install (ARCHITECTURE.md section 9).
//!
//! Either way the service wiring is identical (one modular monolith, section 8.2), and
//! the server links **neither** `accord-crypto` nor `accord-mls` - it stays an
//! opaque relay for private chats (section 5, section 8.3).

pub mod auth;
pub mod authz;
pub mod backup;
pub mod config;
pub mod error;
pub mod friends;
pub mod groups;
pub mod guardrails;
pub mod messaging;
pub mod mls_relay;
pub mod roles;
pub mod store;
pub mod tls;
pub mod util;
pub mod vault;

pub use config::Config;

use accord_proto::auth_service_server::AuthServiceServer;
use accord_proto::backup_service_server::BackupServiceServer;
use accord_proto::friend_service_server::FriendServiceServer;
use accord_proto::group_service_server::GroupServiceServer;
use accord_proto::messaging_service_server::MessagingServiceServer;
use accord_proto::mls_service_server::MlsServiceServer;
use accord_proto::role_service_server::RoleServiceServer;
use accord_proto::vault_service_server::VaultServiceServer;
use accord_types::perms::Permissions;
use tokio::sync::oneshot;
use tonic::transport::Server;

use crate::auth::AuthSvc;
use crate::auth::jwt::JwtKeys;
use crate::backup::BackupSvc;
use crate::friends::FriendSvc;
use crate::groups::GroupSvc;
use crate::messaging::{Hub, MessagingSvc};
use crate::mls_relay::MlsRelaySvc;
use crate::roles::RoleSvc;
use crate::vault::VaultSvc;

/// Run the server until the process ends.
///
/// # Errors
/// Returns an error if the database, bus, or gRPC server fail to start.
pub async fn run(config: Config) -> anyhow::Result<()> {
    run_inner(config, None).await
}

/// Run the server until `shutdown` resolves (used by the embedded/self-hosted
/// path so the client can stop it cleanly).
///
/// # Errors
/// Returns an error if the database, bus, or gRPC server fail to start.
pub async fn run_with_shutdown(
    config: Config,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    run_inner(config, Some(shutdown)).await
}

async fn run_inner(config: Config, shutdown: Option<oneshot::Receiver<()>>) -> anyhow::Result<()> {
    // Database (Postgres or SQLite, chosen from the URL) + migrations.
    let store = store::connect(&config.database_url, config.db_max_connections).await?;
    tracing::info!(
        backend = if config.database_url.starts_with("sqlite") {
            "sqlite"
        } else {
            "postgres"
        },
        "database connected, migrations applied"
    );

    // Ensure the `@everyone` default role exists (idempotent; default bits live
    // in Rust so they are never stale).
    if let Ok(everyone_id) = uuid::Uuid::parse_str(roles::DEFAULT_ROLE_ID) {
        store
            .ensure_default_role(
                everyone_id,
                "@everyone",
                Permissions::default_everyone().bits() as i64,
            )
            .await?;
    }

    // Ensure the singleton tavern-identity row exists (one server = one tavern).
    if let Ok(tavern_id) = uuid::Uuid::parse_str(groups::TAVERN_ID) {
        store.ensure_tavern(tavern_id).await?;
    }

    // Ensure the default "Text Channels" / "Voice Channels" categories exist and
    // any pre-existing channels (e.g. the seeded #general) are filed under them.
    store.ensure_default_categories().await?;

    // Message bus: Redis (cross-instance) when configured, else in-process.
    let redis_url = (!config.redis_url.is_empty()).then_some(config.redis_url.as_str());
    let hub = Hub::new(redis_url).await?;
    hub.clone().spawn_bus_listener().await?;

    let jwt = JwtKeys::new(&config.jwt_secret, config.access_token_ttl_secs);

    // Guardrail/auto-mod engine: rate-limits + flags privileged actions (even for
    // admins), shared across services. Owner is alerted but not blocked by default.
    let guardrails = std::sync::Arc::new(crate::guardrails::Guardrails::default());

    let auth_service = AuthServiceServer::new(AuthSvc::new(
        store.clone(),
        jwt.clone(),
        config.require_invite,
        config.open_dms,
    ));
    let group_service = GroupServiceServer::new(GroupSvc::new(
        store.clone(),
        jwt.clone(),
        hub.clone(),
        guardrails.clone(),
    ));
    let messaging_service =
        MessagingServiceServer::new(MessagingSvc::new(store.clone(), jwt.clone(), hub.clone()));
    let mls_service =
        MlsServiceServer::new(MlsRelaySvc::new(store.clone(), jwt.clone(), hub.clone()));
    let role_service = RoleServiceServer::new(RoleSvc::new(store.clone(), jwt.clone()));
    let backup_service = BackupServiceServer::new(BackupSvc::new(store.clone(), jwt.clone()));
    let vault_service = VaultServiceServer::new(VaultSvc::new(store.clone(), jwt.clone()));
    let friend_service = FriendServiceServer::new(FriendSvc::new(store.clone(), jwt.clone()));

    // Enable TLS when a cert+key are configured (self-signed; clients pin the
    // cert via the invite key). Otherwise serve plaintext (dev / behind a proxy).
    let mut server = Server::builder();
    if let (Some(cert), Some(key)) = (config.tls_cert_pem.as_ref(), config.tls_key_pem.as_ref()) {
        let identity = tonic::transport::Identity::from_pem(cert, key);
        server = server
            .tls_config(tonic::transport::ServerTlsConfig::new().identity(identity))
            .map_err(|e| anyhow::anyhow!("TLS config error: {e}"))?;
        tracing::info!("TLS enabled (self-signed; clients pin the cert via the invite key)");
    }

    let router = server
        .add_service(auth_service)
        .add_service(group_service)
        .add_service(messaging_service)
        .add_service(mls_service)
        .add_service(role_service)
        .add_service(backup_service)
        .add_service(vault_service)
        .add_service(friend_service);

    // Bind dual-stack so a single listener serves both IPv4 (LAN) and IPv6
    // (e.g. a Yggdrasil mesh address) - important for self-hosting over the mesh.
    let listener = bind_listener(config.bind_addr)?;
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tracing::info!(bind = %config.bind_addr, "accord-server ready");
    match shutdown {
        Some(rx) => {
            router
                .serve_with_incoming_shutdown(incoming, async move {
                    let _ = rx.await;
                    tracing::info!("accord-server shutting down");
                })
                .await?;
        }
        None => router.serve_with_incoming(incoming).await?,
    }
    Ok(())
}

/// Build a TCP listener. For an IPv6 wildcard address we disable `IPV6_V6ONLY`
/// so the socket accepts IPv4-mapped connections too (dual-stack), letting one
/// `[::]:port` bind serve LAN IPv4 and mesh IPv6 alike.
fn bind_listener(addr: std::net::SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Socket, Type};

    let domain = if addr.is_ipv6() {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::STREAM, None)?;
    if addr.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(socket.into())
}
