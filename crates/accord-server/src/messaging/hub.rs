//! Real-time delivery hub.
//!
//! Bridges local stream connections with cross-instance fan-out via a Redis
//! pub/sub bus (ARCHITECTURE.md section 8, section 17.2-section 17.3). Every message a server wants
//! delivered is published as a [`BusEvent`]; every instance consumes the bus and
//! routes each event to its locally-connected recipients.
//!
//! Routing targets:
//! * **group** - public messages, private ciphertext, Commits (all members,
//! optionally excluding the sender's own device).
//! * **device** - Welcomes, and Commits/Welcomes drained from a device's offline
//! inbox.
//!
//! Sender exclusion matters for MLS: a sender cannot process its own Commit or
//! decrypt its own application message, so those events skip the sender's device.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use accord_proto::server_message::Payload;
use accord_proto::{
    DeviceId, GroupId, IncomingPrivateMessage, IncomingPublicMessage, MessageId,
    MlsCommitNotification, MlsWelcomeNotification, ModAlert, ServerMessage, UserId, VoiceParticipant,
};
use futures::StreamExt;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tonic::Status;
use uuid::Uuid;

const BUS_CHANNEL: &str = "accord:bus";
const CLIENT_BUFFER: usize = 128;

type ClientSender = mpsc::Sender<Result<ServerMessage, Status>>;

/// Envelope for a public message on the bus.
#[derive(Debug, Serialize, Deserialize)]
pub struct PublicBusMessage {
    pub message_id: String,
    pub group_id: String,
    pub sender_id: String,
    pub sender_display_name: String,
    pub content: String,
    pub timestamp_secs: i64,
    pub timestamp_nanos: i32,
    pub sequence_number: u64,
}

/// Envelope for a private (encrypted) message on the bus.
#[derive(Debug, Serialize, Deserialize)]
pub struct PrivateBusMessage {
    pub group_id: String,
    pub sender_id: String,
    pub ciphertext: Vec<u8>,
    pub epoch: u64,
    pub timestamp_secs: i64,
    pub timestamp_nanos: i32,
    pub sequence_number: u64,
}

/// Everything that can travel over the Redis bus.
#[derive(Debug, Serialize, Deserialize)]
enum BusEvent {
    /// Public message -> all members of the group.
    Public(PublicBusMessage),
    /// Private ciphertext -> group members except the sender's device.
    Private {
        msg: PrivateBusMessage,
        exclude_device: String,
    },
    /// MLS Welcome -> a specific device.
    Welcome {
        device_id: String,
        group_id: String,
        welcome: Vec<u8>,
    },
    /// MLS Commit -> group members except the committing device.
    Commit {
        group_id: String,
        commit: Vec<u8>,
        epoch: u64,
        exclude_device: String,
    },
}

impl PublicBusMessage {
    fn into_server_message(self) -> ServerMessage {
        ServerMessage {
            payload: Some(Payload::PublicMessage(IncomingPublicMessage {
                message_id: Some(MessageId {
                    value: self.message_id,
                }),
                group_id: Some(GroupId {
                    value: self.group_id,
                }),
                sender_id: Some(UserId {
                    value: self.sender_id,
                }),
                sender_display_name: self.sender_display_name,
                content: self.content,
                timestamp: Some(prost_types::Timestamp {
                    seconds: self.timestamp_secs,
                    nanos: self.timestamp_nanos,
                }),
                sequence_number: self.sequence_number,
            })),
        }
    }
}

impl PrivateBusMessage {
    fn into_server_message(self) -> ServerMessage {
        ServerMessage {
            payload: Some(Payload::PrivateMessage(IncomingPrivateMessage {
                group_id: Some(GroupId {
                    value: self.group_id,
                }),
                sender_id: Some(UserId {
                    value: self.sender_id,
                }),
                mls_ciphertext: self.ciphertext,
                epoch: self.epoch,
                timestamp: Some(prost_types::Timestamp {
                    seconds: self.timestamp_secs,
                    nanos: self.timestamp_nanos,
                }),
                sequence_number: self.sequence_number,
            })),
        }
    }
}

/// Build a Welcome-notification `ServerMessage` (used for live + inbox delivery).
#[must_use]
pub fn welcome_notification(group_id: &str, welcome: Vec<u8>) -> ServerMessage {
    ServerMessage {
        payload: Some(Payload::WelcomeNotification(MlsWelcomeNotification {
            group_id: Some(GroupId {
                value: group_id.to_owned(),
            }),
            welcome,
        })),
    }
}

/// Build a `VoiceParticipant` `ServerMessage` for a device's state in a channel.
#[must_use]
pub fn voice_participant_msg(
    group: Uuid,
    device: Uuid,
    state: &VoiceState,
    joined: bool,
) -> ServerMessage {
    ServerMessage {
        payload: Some(Payload::VoiceParticipant(VoiceParticipant {
            group_id: Some(GroupId {
                value: group.to_string(),
            }),
            user_id: Some(UserId {
                value: state.user_id.to_string(),
            }),
            device_id: Some(DeviceId {
                value: device.to_string(),
            }),
            joined,
            muted: state.muted,
            camera_on: state.camera_on,
            screen_on: state.screen_on,
        })),
    }
}

/// Build a Commit-notification `ServerMessage`.
#[must_use]
pub fn commit_notification(group_id: &str, commit: Vec<u8>, epoch: u64) -> ServerMessage {
    ServerMessage {
        payload: Some(Payload::CommitNotification(MlsCommitNotification {
            group_id: Some(GroupId {
                value: group_id.to_owned(),
            }),
            commit,
            epoch,
        })),
    }
}

/// Metadata stored with a private ciphertext in a device's offline mailbox, so a
/// drain can rebuild the full notification. Encoded as JSON in the inbox `payload`
/// for `kind = "private"`. (The server still only ever holds opaque ciphertext.)
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PrivateInboxPayload {
    pub ciphertext: Vec<u8>,
    pub sender_id: String,
    pub epoch: u64,
    pub timestamp_secs: i64,
    pub timestamp_nanos: i32,
    pub sequence_number: u64,
}

/// Encode a private message for a device's offline mailbox.
#[must_use]
pub fn encode_private_inbox(payload: &PrivateInboxPayload) -> Vec<u8> {
    serde_json::to_vec(payload).expect("PrivateInboxPayload always serializes")
}

/// Rebuild a private-message `ServerMessage` from a drained mailbox payload.
#[must_use]
pub fn private_notification(group_id: &str, payload: &[u8]) -> Option<ServerMessage> {
    let p: PrivateInboxPayload = serde_json::from_slice(payload).ok()?;
    Some(ServerMessage {
        payload: Some(Payload::PrivateMessage(IncomingPrivateMessage {
            group_id: Some(GroupId {
                value: group_id.to_owned(),
            }),
            sender_id: Some(UserId { value: p.sender_id }),
            mls_ciphertext: p.ciphertext,
            epoch: p.epoch,
            timestamp: Some(prost_types::Timestamp {
                seconds: p.timestamp_secs,
                nanos: p.timestamp_nanos,
            }),
            sequence_number: p.sequence_number,
        })),
    })
}

#[derive(Default)]
struct Registry {
    /// group -> (device -> sender)
    group_subs: HashMap<Uuid, HashMap<Uuid, ClientSender>>,
    /// device -> sender (for device-targeted delivery)
    device_subs: HashMap<Uuid, ClientSender>,
    /// device -> groups it is registered under (for cleanup)
    device_groups: HashMap<Uuid, Vec<Uuid>>,
}

/// A device's voice state in a voice channel (scaffold; media is P2P in the
/// client). Tracked in-memory per instance - single-instance only, like the
/// dynamic `subscribe`/`is_connected` machinery above. Cross-instance voice
/// presence would propagate over the bus, deferred.
#[derive(Debug, Clone, Copy)]
pub struct VoiceState {
    pub user_id: Uuid,
    pub muted: bool,
    pub camera_on: bool,
    pub screen_on: bool,
}

/// How the hub fans messages out to other server instances.
enum Transport {
    /// Cross-instance fan-out via Redis pub/sub (platform deployments).
    Redis {
        client: redis::Client,
        publisher: ConnectionManager,
    },
    /// Single-instance, in-process delivery - no external service. This is what
    /// lets a client-hosted server run with zero dependencies.
    Local,
}

/// The delivery hub. Shared as `Arc<Hub>`.
pub struct Hub {
    registry: Mutex<Registry>,
    transport: Transport,
    /// Voice channel participants: group -> (device -> state). Single-instance.
    voice: Mutex<HashMap<Uuid, HashMap<Uuid, VoiceState>>>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").finish_non_exhaustive()
    }
}

impl Hub {
    /// Build the hub. With `Some(non-empty url)` it uses Redis for cross-instance
    /// fan-out; with `None`/empty it uses an in-process bus (self-contained).
    ///
    /// # Errors
    /// Returns a [`redis::RedisError`] if a Redis connection is requested but
    /// cannot be established.
    pub async fn new(redis_url: Option<&str>) -> Result<Arc<Self>, redis::RedisError> {
        let transport = match redis_url {
            Some(url) if !url.is_empty() => {
                let client = redis::Client::open(url)?;
                let publisher = client.get_connection_manager().await?;
                Transport::Redis { client, publisher }
            }
            _ => Transport::Local,
        };
        Ok(Arc::new(Self {
            registry: Mutex::new(Registry::default()),
            transport,
            voice: Mutex::new(HashMap::new()),
        }))
    }

    /// Register a connected device under its groups; returns the stream of
    /// messages to forward to that client. Caller must [`Hub::unregister`] on
    /// disconnect.
    pub fn register(
        &self,
        device: Uuid,
        groups: &[Uuid],
    ) -> mpsc::Receiver<Result<ServerMessage, Status>> {
        let (tx, rx) = mpsc::channel(CLIENT_BUFFER);
        let mut reg = self.registry.lock().expect("hub registry poisoned");
        for &group in groups {
            reg.group_subs
                .entry(group)
                .or_default()
                .insert(device, tx.clone());
        }
        reg.device_subs.insert(device, tx);
        reg.device_groups.insert(device, groups.to_vec());
        rx
    }

    /// Remove a device from all routing tables (and any voice channels).
    pub fn unregister(&self, device: Uuid) {
        {
            let mut reg = self.registry.lock().expect("hub registry poisoned");
            reg.device_subs.remove(&device);
            if let Some(groups) = reg.device_groups.remove(&device) {
                for group in groups {
                    if let Some(members) = reg.group_subs.get_mut(&group) {
                        members.remove(&device);
                        if members.is_empty() {
                            reg.group_subs.remove(&group);
                        }
                    }
                }
            }
        }
        // A disconnect leaves any voice channels the device was in; notify peers.
        for (group, state) in self.voice_drop_device(device) {
            self.publish_voice_participant(group, voice_participant_msg(group, device, &state, false));
        }
    }

    /// Dynamically subscribe an already-connected device to a group it just
    /// joined/created, so it receives live traffic without reconnecting.
    ///
    /// Only affects devices connected to *this* instance. (Cross-instance
    /// dynamic subscription would require propagating the intent over the bus;
    /// for the single-instance dev deployment this is sufficient. A device also
    /// re-subscribes to all its groups whenever it reopens its stream.)
    pub fn subscribe(&self, device: Uuid, group: Uuid) {
        let mut reg = self.registry.lock().expect("hub registry poisoned");
        if let Some(sender) = reg.device_subs.get(&device).cloned() {
            reg.group_subs
                .entry(group)
                .or_default()
                .insert(device, sender);
            reg.device_groups.entry(device).or_default().push(group);
        }
    }

    /// Whether a device is currently connected to *this* instance. Used to queue
    /// private messages into a device's offline mailbox instead of dropping them.
    /// (Single-instance presence; cross-instance global presence is a later step,
    /// like the other single-instance caveats here.)
    #[must_use]
    pub fn is_connected(&self, device: Uuid) -> bool {
        let reg = self.registry.lock().expect("hub registry poisoned");
        reg.device_subs.contains_key(&device)
    }

    /// Whether this hub runs the in-process (single-instance) transport. The
    /// offline mailbox relies on single-instance presence: in Redis mode a device
    /// connected to *another* instance would look offline here and get a
    /// duplicate queued copy, so the mailbox only runs when this is true.
    #[must_use]
    pub fn is_single_instance(&self) -> bool {
        matches!(self.transport, Transport::Local)
    }

    /// Send a `ServerMessage` directly to a locally-connected device (used for
    /// inbox drains right after the device registers on this instance).
    pub fn send_to_device(&self, device: Uuid, message: ServerMessage) {
        let reg = self.registry.lock().expect("hub registry poisoned");
        if let Some(sender) = reg.device_subs.get(&device) {
            let _ = sender.try_send(Ok(message));
        }
    }

    // --- voice (scaffold, single-instance) ----------------------------------

    /// Record/refresh a device's voice state in a channel and return the full
    /// participant list afterwards (so the caller can burst it to the newcomer).
    pub fn voice_join(&self, group: Uuid, device: Uuid, state: VoiceState) -> Vec<(Uuid, VoiceState)> {
        let mut voice = self.voice.lock().expect("hub voice poisoned");
        let members = voice.entry(group).or_default();
        members.insert(device, state);
        members.iter().map(|(d, s)| (*d, *s)).collect()
    }

    /// Update a device's voice state in a channel (no-op if not present).
    pub fn voice_set_state(&self, group: Uuid, device: Uuid, state: VoiceState) {
        let mut voice = self.voice.lock().expect("hub voice poisoned");
        if let Some(members) = voice.get_mut(&group) {
            if let Some(slot) = members.get_mut(&device) {
                *slot = state;
            }
        }
    }

    /// Remove a device from a voice channel. Returns its last state if present.
    pub fn voice_leave(&self, group: Uuid, device: Uuid) -> Option<VoiceState> {
        let mut voice = self.voice.lock().expect("hub voice poisoned");
        let members = voice.get_mut(&group)?;
        let state = members.remove(&device);
        if members.is_empty() {
            voice.remove(&group);
        }
        state
    }

    /// Remove a device from every voice channel (on disconnect). Returns the
    /// `(group, last_state)` pairs it was in.
    fn voice_drop_device(&self, device: Uuid) -> Vec<(Uuid, VoiceState)> {
        let mut voice = self.voice.lock().expect("hub voice poisoned");
        let mut left = Vec::new();
        voice.retain(|group, members| {
            if let Some(state) = members.remove(&device) {
                left.push((*group, state));
            }
            !members.is_empty()
        });
        left
    }

    /// Fan a `VoiceParticipant` update out to a voice channel's members
    /// (single-instance: routed directly to local subscribers).
    pub fn publish_voice_participant(&self, group: Uuid, message: ServerMessage) {
        self.deliver_to_group(group, &message, None);
    }

    /// Deliver a `ModAlert` to each of `devices` that is connected here.
    pub fn send_mod_alert(&self, devices: &[Uuid], alert: ModAlert) {
        let msg = ServerMessage {
            payload: Some(Payload::ModAlert(alert)),
        };
        let reg = self.registry.lock().expect("hub registry poisoned");
        for device in devices {
            if let Some(sender) = reg.device_subs.get(device) {
                let _ = sender.try_send(Ok(msg.clone()));
            }
        }
    }

    // --- publish helpers (called by services) -------------------------------

    /// Publish a public message to all group members.
    ///
    /// # Errors
    /// Returns a [`redis::RedisError`] if publishing fails.
    pub async fn publish_public(&self, msg: PublicBusMessage) -> Result<(), redis::RedisError> {
        self.publish(BusEvent::Public(msg)).await
    }

    /// Publish a private ciphertext to group members except `exclude_device`.
    ///
    /// # Errors
    /// Returns a [`redis::RedisError`] if publishing fails.
    pub async fn publish_private(
        &self,
        msg: PrivateBusMessage,
        exclude_device: Uuid,
    ) -> Result<(), redis::RedisError> {
        self.publish(BusEvent::Private {
            msg,
            exclude_device: exclude_device.to_string(),
        })
        .await
    }

    /// Publish a Welcome targeted at a single device.
    ///
    /// # Errors
    /// Returns a [`redis::RedisError`] if publishing fails.
    pub async fn publish_welcome(
        &self,
        device_id: Uuid,
        group_id: Uuid,
        welcome: Vec<u8>,
    ) -> Result<(), redis::RedisError> {
        self.publish(BusEvent::Welcome {
            device_id: device_id.to_string(),
            group_id: group_id.to_string(),
            welcome,
        })
        .await
    }

    /// Publish a Commit to group members except `exclude_device`.
    ///
    /// # Errors
    /// Returns a [`redis::RedisError`] if publishing fails.
    pub async fn publish_commit(
        &self,
        group_id: Uuid,
        commit: Vec<u8>,
        epoch: u64,
        exclude_device: Uuid,
    ) -> Result<(), redis::RedisError> {
        self.publish(BusEvent::Commit {
            group_id: group_id.to_string(),
            commit,
            epoch,
            exclude_device: exclude_device.to_string(),
        })
        .await
    }

    /// Fan an event out: over Redis (other instances consume it) or, in local
    /// mode, route it directly to this instance's connections.
    async fn publish(&self, event: BusEvent) -> Result<(), redis::RedisError> {
        match &self.transport {
            Transport::Redis { publisher, .. } => {
                let payload = serde_json::to_string(&event).expect("BusEvent always serializes");
                let mut conn = publisher.clone();
                let _: () = conn.publish(BUS_CHANNEL, payload).await?;
            }
            Transport::Local => self.route(event),
        }
        Ok(())
    }

    // --- local delivery (called by the bus listener) ------------------------

    fn deliver_to_group(&self, group: Uuid, message: &ServerMessage, exclude: Option<Uuid>) {
        let reg = self.registry.lock().expect("hub registry poisoned");
        if let Some(members) = reg.group_subs.get(&group) {
            for (device, sender) in members {
                if Some(*device) == exclude {
                    continue;
                }
                let _ = sender.try_send(Ok(message.clone()));
            }
        }
    }

    fn deliver_to_device(&self, device: Uuid, message: ServerMessage) {
        let reg = self.registry.lock().expect("hub registry poisoned");
        if let Some(sender) = reg.device_subs.get(&device) {
            let _ = sender.try_send(Ok(message));
        }
    }

    /// Spawn the background task consuming the Redis bus and routing events.
    ///
    /// # Errors
    /// Returns a [`redis::RedisError`] if the initial subscription fails.
    pub async fn spawn_bus_listener(self: Arc<Self>) -> Result<(), redis::RedisError> {
        let client = match &self.transport {
            Transport::Redis { client, .. } => client.clone(),
            Transport::Local => {
                tracing::info!("using in-process message bus (no Redis)");
                return Ok(());
            }
        };
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub.subscribe(BUS_CHANNEL).await?;
        tracing::info!(channel = BUS_CHANNEL, "subscribed to Redis message bus");

        tokio::spawn(async move {
            let mut stream = pubsub.on_message();
            while let Some(msg) = stream.next().await {
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, "bad bus payload");
                        continue;
                    }
                };
                match serde_json::from_str::<BusEvent>(&payload) {
                    Ok(event) => self.route(event),
                    Err(e) => tracing::warn!(error = %e, "could not decode bus event"),
                }
            }
            tracing::warn!("Redis bus listener stopped");
        });
        Ok(())
    }

    /// Route one decoded bus event to its local recipients.
    fn route(&self, event: BusEvent) {
        match event {
            BusEvent::Public(msg) => {
                if let Ok(group) = Uuid::parse_str(&msg.group_id) {
                    self.deliver_to_group(group, &msg.into_server_message(), None);
                }
            }
            BusEvent::Private {
                msg,
                exclude_device,
            } => {
                let group = Uuid::parse_str(&msg.group_id).ok();
                let exclude = Uuid::parse_str(&exclude_device).ok();
                if let Some(group) = group {
                    self.deliver_to_group(group, &msg.into_server_message(), exclude);
                }
            }
            BusEvent::Welcome {
                device_id,
                group_id,
                welcome,
            } => {
                if let Ok(device) = Uuid::parse_str(&device_id) {
                    self.deliver_to_device(device, welcome_notification(&group_id, welcome));
                }
            }
            BusEvent::Commit {
                group_id,
                commit,
                epoch,
                exclude_device,
            } => {
                let group = Uuid::parse_str(&group_id).ok();
                let exclude = Uuid::parse_str(&exclude_device).ok();
                if let Some(group) = group {
                    self.deliver_to_group(
                        group,
                        &commit_notification(&group_id, commit, epoch),
                        exclude,
                    );
                }
            }
        }
    }
}
