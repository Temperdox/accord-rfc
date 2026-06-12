//! `MessagingService` gRPC implementation: the bidirectional message stream plus
//! history fetches.

use std::pin::Pin;
use std::sync::Arc;

use accord_proto::client_message::Payload as ClientPayload;
use accord_proto::messaging_service_server::MessagingService;
use accord_proto::{
    ClientMessage, FetchHistoryRequest, FetchPrivateHistoryResponse, FetchPublicHistoryResponse,
    GroupId, IncomingPrivateMessage, IncomingPublicMessage, MessageId, SendPrivateMessage,
    SendPublicMessage, ServerMessage, UserId,
};
use futures::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::error::ServerError;
use crate::messaging::hub::{
    Hub, PrivateBusMessage, PrivateInboxPayload, PublicBusMessage, commit_notification,
    encode_private_inbox, private_notification, welcome_notification,
};
use crate::store::Store;
use crate::util::{authenticate, to_proto_timestamp};

/// Default / maximum page sizes for history fetches.
const DEFAULT_HISTORY_LIMIT: i64 = 50;
const MAX_HISTORY_LIMIT: i64 = 200;

/// Boxed server->client stream type returned by `MessageStream`.
type ResponseStream = Pin<Box<dyn Stream<Item = Result<ServerMessage, Status>> + Send>>;

/// Unregisters a device from the [`Hub`] when the connection ends (on drop).
struct ConnectionGuard {
    hub: Arc<Hub>,
    device: Uuid,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.hub.unregister(self.device);
        tracing::debug!(device = %self.device, "message stream closed; deregistered");
    }
}

/// Implements the `MessagingService` RPCs.
#[derive(Debug)]
pub struct MessagingSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
    hub: Arc<Hub>,
}

impl MessagingSvc {
    /// Construct the service from its dependencies.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys, hub: Arc<Hub>) -> Self {
        Self { store, jwt, hub }
    }
}

#[tonic::async_trait]
impl MessagingService for MessagingSvc {
    type MessageStreamStream = ResponseStream;

    async fn message_stream(
        &self,
        request: Request<Streaming<ClientMessage>>,
    ) -> Result<Response<Self::MessageStreamStream>, Status> {
        // Authenticate from stream metadata before doing anything else.
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub, "user id in token")?;
        let device_id = parse_uuid(&claims.device_id, "device id in token")?;

        let mut inbound = request.into_inner();

        // Subscribe this device to all of its groups for real-time delivery.
        let groups = self.store.group_ids_for_user(user_id).await?;
        let rx = self.hub.register(device_id, &groups);
        tracing::info!(%user_id, %device_id, group_count = groups.len(), "message stream opened");

        // Drain any MLS handshake messages (Welcome/Commit) queued while this
        // device was offline, pushing them into the freshly-opened stream.
        match self.store.drain_inbox(device_id).await {
            Ok(items) => {
                for item in items {
                    let group = item.group_id.to_string();
                    let msg = match item.kind.as_str() {
                        "welcome" => welcome_notification(&group, item.payload),
                        "commit" => commit_notification(&group, item.payload, 0),
                        "private" => match private_notification(&group, &item.payload) {
                            Some(m) => m,
                            None => continue,
                        },
                        _ => continue,
                    };
                    self.hub.send_to_device(device_id, msg);
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not drain MLS inbox"),
        }

        // Reader task: pump inbound client messages until the client disconnects.
        // The guard unregisters the device when this task ends.
        let store = self.store.clone();
        let hub = self.hub.clone();
        tokio::spawn(async move {
            let _guard = ConnectionGuard {
                hub: hub.clone(),
                device: device_id,
            };
            loop {
                match inbound.message().await {
                    Ok(Some(msg)) => {
                        if let Err(e) =
                            handle_client_message(store.as_ref(), &hub, user_id, device_id, msg)
                                .await
                        {
                            tracing::warn!(error = %e, "error handling client message");
                        }
                    }
                    Ok(None) => break, // client closed cleanly
                    Err(status) => {
                        tracing::debug!(%status, "inbound stream ended");
                        break;
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn fetch_public_history(
        &self,
        request: Request<FetchHistoryRequest>,
    ) -> Result<Response<FetchPublicHistoryResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub, "user id in token")?;
        let req = request.into_inner();

        let group_id = require_group_id(&req.group_id)?;
        if !self.store.is_member(group_id, user_id).await? {
            return Err(ServerError::PermissionDenied.into());
        }

        let limit = clamp_limit(req.limit);
        let rows = self
            .store
            .fetch_public_history(group_id, req.before_sequence as i64, limit)
            .await?;

        let messages = rows.into_iter().map(public_row_to_proto).collect();
        Ok(Response::new(FetchPublicHistoryResponse { messages }))
    }

    async fn fetch_private_history(
        &self,
        request: Request<FetchHistoryRequest>,
    ) -> Result<Response<FetchPrivateHistoryResponse>, Status> {
        let claims = authenticate(&request, &self.jwt)?;
        let user_id = parse_uuid(&claims.sub, "user id in token")?;
        let req = request.into_inner();

        let group_id = require_group_id(&req.group_id)?;
        if !self.store.is_member(group_id, user_id).await? {
            return Err(ServerError::PermissionDenied.into());
        }

        let limit = clamp_limit(req.limit);
        let rows = self
            .store
            .fetch_private_history(group_id, req.before_sequence as i64, limit)
            .await?;
        let messages = rows.into_iter().map(private_row_to_proto).collect();
        Ok(Response::new(FetchPrivateHistoryResponse { messages }))
    }
}

/// Route a single inbound client message.
async fn handle_client_message(
    store: &dyn Store,
    hub: &Hub,
    user_id: Uuid,
    device_id: Uuid,
    msg: ClientMessage,
) -> Result<(), ServerError> {
    match msg.payload {
        Some(ClientPayload::PublicMessage(m)) => {
            handle_public_message(store, hub, user_id, m).await
        }
        Some(ClientPayload::PrivateMessage(m)) => {
            handle_private_message(store, hub, user_id, device_id, m).await
        }
        Some(ClientPayload::Typing(_)) => {
            // Typing indicators are a later enhancement.
            Ok(())
        }
        Some(ClientPayload::Ack(_)) => {
            // Acks matter once the per-device offline push queue exists.
            Ok(())
        }
        None => Ok(()),
    }
}

/// Persist + fan out a public chat message.
async fn handle_public_message(
    store: &dyn Store,
    hub: &Hub,
    user_id: Uuid,
    m: SendPublicMessage,
) -> Result<(), ServerError> {
    let group_id = require_group_id(&m.group_id)?;
    if m.content.trim().is_empty() {
        return Err(ServerError::InvalidArgument(
            "message content is empty".into(),
        ));
    }
    if !store.is_member(group_id, user_id).await? {
        return Err(ServerError::PermissionDenied);
    }

    // A missing client_message_id just means we mint one (idempotency is then a
    // no-op for this send, which is fine).
    let client_message_id = m
        .client_message_id
        .and_then(|id| Uuid::parse_str(&id.value).ok())
        .unwrap_or_else(Uuid::now_v7);

    let row = store
        .insert_public_message(group_id, user_id, m.content.trim(), client_message_id)
        .await?;

    let bus = PublicBusMessage {
        message_id: row.id.to_string(),
        group_id: row.group_id.to_string(),
        sender_id: row.sender_id.to_string(),
        sender_display_name: row.sender_display_name,
        content: row.content,
        timestamp_secs: row.created_at.timestamp(),
        timestamp_nanos: row.created_at.timestamp_subsec_nanos() as i32,
        sequence_number: row.seq as u64,
    };
    hub.publish_public(bus).await?;
    Ok(())
}

/// Persist + relay an encrypted private message. The server treats the
/// ciphertext as opaque and never decrypts it (ARCHITECTURE section 5).
async fn handle_private_message(
    store: &dyn Store,
    hub: &Hub,
    user_id: Uuid,
    device_id: Uuid,
    m: SendPrivateMessage,
) -> Result<(), ServerError> {
    let group_id = require_group_id(&m.group_id)?;
    if m.mls_ciphertext.is_empty() {
        return Err(ServerError::InvalidArgument("empty ciphertext".into()));
    }
    if !store.is_member(group_id, user_id).await? {
        return Err(ServerError::PermissionDenied);
    }

    let client_message_id = m
        .client_message_id
        .and_then(|id| Uuid::parse_str(&id.value).ok())
        .unwrap_or_else(Uuid::now_v7);

    let row = store
        .store_private_message(
            group_id,
            user_id,
            device_id,
            &m.mls_ciphertext,
            m.epoch as i64,
            client_message_id,
        )
        .await?;

    // Encode for any offline members' mailboxes before the ciphertext is moved
    // into the live bus message.
    let inbox_payload = encode_private_inbox(&PrivateInboxPayload {
        ciphertext: row.ciphertext.clone(),
        sender_id: row.sender_id.to_string(),
        epoch: row.epoch as u64,
        timestamp_secs: row.created_at.timestamp(),
        timestamp_nanos: row.created_at.timestamp_subsec_nanos() as i32,
        sequence_number: row.seq as u64,
    });

    let bus = PrivateBusMessage {
        group_id: row.group_id.to_string(),
        sender_id: row.sender_id.to_string(),
        ciphertext: row.ciphertext,
        epoch: row.epoch as u64,
        timestamp_secs: row.created_at.timestamp(),
        timestamp_nanos: row.created_at.timestamp_subsec_nanos() as i32,
        sequence_number: row.seq as u64,
    };
    // Exclude the sender's own device - it cannot decrypt its own ciphertext.
    hub.publish_private(bus, device_id).await?;

    // Mailbox: queue the message for member devices that are offline right now, so
    // they receive it on reconnect (in drain order, after any queued handshakes).
    // Connected devices already got it live above, so they are skipped to avoid a
    // duplicate. Presence is single-instance, so the mailbox only runs on the
    // in-process transport - in Redis mode a device on another instance would
    // look offline here and get a duplicate copy.
    if !hub.is_single_instance() {
        return Ok(());
    }
    for dev in store.device_ids_for_group(group_id).await? {
        if dev == device_id || hub.is_connected(dev) {
            continue;
        }
        if let Err(e) = store
            .enqueue_inbox(dev, "private", group_id, &inbox_payload)
            .await
        {
            tracing::warn!(error = %e, device = %dev, "could not queue private message for offline device");
        }
    }
    Ok(())
}

// --- small mapping/validation helpers ---------------------------------------

fn private_row_to_proto(row: crate::store::model::PrivateMessageRow) -> IncomingPrivateMessage {
    IncomingPrivateMessage {
        group_id: Some(GroupId {
            value: row.group_id.to_string(),
        }),
        sender_id: Some(UserId {
            value: row.sender_id.to_string(),
        }),
        mls_ciphertext: row.ciphertext,
        epoch: row.epoch as u64,
        timestamp: Some(to_proto_timestamp(row.created_at)),
        sequence_number: row.seq as u64,
    }
}

fn public_row_to_proto(row: crate::store::model::PublicMessageRow) -> IncomingPublicMessage {
    IncomingPublicMessage {
        message_id: Some(MessageId {
            value: row.id.to_string(),
        }),
        group_id: Some(GroupId {
            value: row.group_id.to_string(),
        }),
        sender_id: Some(UserId {
            value: row.sender_id.to_string(),
        }),
        sender_display_name: row.sender_display_name,
        content: row.content,
        timestamp: Some(to_proto_timestamp(row.created_at)),
        sequence_number: row.seq as u64,
    }
}

fn require_group_id(group_id: &Option<GroupId>) -> Result<Uuid, ServerError> {
    let value = group_id
        .as_ref()
        .ok_or_else(|| ServerError::InvalidArgument("group_id is required".into()))?;
    Uuid::parse_str(&value.value)
        .map_err(|_| ServerError::InvalidArgument("group_id is not a valid UUID".into()))
}

fn parse_uuid(s: &str, what: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(s).map_err(|_| ServerError::InvalidArgument(format!("invalid {what}")))
}

fn clamp_limit(requested: u32) -> i64 {
    if requested == 0 {
        DEFAULT_HISTORY_LIMIT
    } else {
        (requested as i64).min(MAX_HISTORY_LIMIT)
    }
}
