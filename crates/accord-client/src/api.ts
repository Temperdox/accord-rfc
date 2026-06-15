/**
 * Typed wrappers over the Tauri IPC bridge.
 *
 * Every function here calls a `#[tauri::command]` defined in
 * `src-tauri/src/commands/`. Keeping the bridge in one file means the React
 * components never touch `invoke` directly and the command names live in exactly
 * one place.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface LoginInfo {
  userId: string;
  deviceId: string;
}

export interface GroupDto {
  id: string;
  name: string;
  kind: string;
  memberCount: number;
}

export interface MessageDto {
  serverId: string;
  id: string;
  groupId: string;
  senderId: string;
  senderDisplayName: string;
  content: string;
  timestampMs: number;
  sequenceNumber: number;
}

export interface PrivateMessageDto {
  serverId: string;
  groupId: string;
  senderId: string;
  /** True when this session's own user sent it. */
  mine: boolean;
  content: string;
  timestampMs: number;
}

/** Connect to a server under a client-chosen `serverId` and make it active. For
 * https endpoints, `cert` (from an invite key) pins the server's self-signed
 * certificate. Other connected servers stay connected. */
export const connect = (
  serverId: string,
  endpoint: string,
  cert?: string | null
): Promise<void> => invoke("connect", { serverId, endpoint, cert: cert ?? null });

/** Switch which connected server the UI is viewing (instant; no reconnect). */
export const setActiveServer = (serverId: string): Promise<void> =>
  invoke("set_active_server", { serverId });

// NOTE: Tauri maps Rust snake_case command params to camelCase on the JS side,
// so multi-word keys here MUST be camelCase (displayName, deviceName, groupId).

/** Create a new account (optionally with an invite token for private servers). */
export const register = (
  username: string,
  password: string,
  displayName: string,
  inviteToken?: string
): Promise<void> =>
  invoke("register", { username, password, displayName, inviteToken: inviteToken ?? "" });

/** Log in; on success the Rust side also opens the message stream. */
export const login = (
  username: string,
  password: string,
  deviceName: string
): Promise<LoginInfo> =>
  invoke("login", { username, password, deviceName });

/** List the channels the logged-in user belongs to. */
export const listGroups = (): Promise<GroupDto[]> => invoke("list_groups");

/** An account known on this device, for the login-screen pills. */
export interface AccountPill {
  username: string;
  isMain: boolean;
}

/** Accounts created on this device (oldest first; main account first). */
export const listAccounts = (): Promise<AccountPill[]> => invoke("list_accounts");

/** A saved contact (cross-user DM addressing, federation phase 1). */
export interface ContactDto {
  id: string;
  name: string;
  fingerprint: string;
  addresses: string[];
  verified: boolean;
}

/** Build this device's shareable contact code (accordc:...). */
export const myContactCode = (name?: string): Promise<string> =>
  invoke("my_contact_code", { name: name ?? null });

/** Add (or update) a contact from a pasted contact code. */
export const addContact = (code: string): Promise<ContactDto> =>
  invoke("add_contact", { code });

/** List saved contacts. */
export const listContacts = (): Promise<ContactDto[]> => invoke("list_contacts");

/** Remove a contact by id (hex public key). */
export const removeContact = (id: string): Promise<void> =>
  invoke("remove_contact", { id });

/** Mark a contact verified/unverified after comparing fingerprints. */
export const setContactVerified = (id: string, verified: boolean): Promise<void> =>
  invoke("set_contact_verified", { id, verified });

/** An incoming friend request parked on my home node. */
export interface IncomingFriendRequest {
  id: string;
  name: string;
  fingerprint: string;
  code: string;
  createdAtMs: number;
}

/** A friend request I sent, awaiting (or retrying) delivery / acceptance. */
export interface PendingSentRequest {
  peerId: string;
  /** Name from the pasted code (placeholder until the profile fetch). */
  name: string;
  fingerprint: string;
  delivered: boolean;
  sentAtMs: number;
  /** Live account data fetched from their home node after delivery
   * (avatar/banner join these when profile media ships). */
  username?: string | null;
  displayName?: string | null;
}

/** What a pasted fr code identifies (decoded locally, nothing sent). */
export interface CodePeek {
  peerId: string;
  name: string;
  fingerprint: string;
}

/** Friend-request sync result: what the Friend Requests view shows. */
export interface FriendsSync {
  incoming: IncomingFriendRequest[];
  pending: PendingSentRequest[];
}

/** Send a friend request from a pasted fr code (queued + retried if their node
 * is unreachable right now). */
export const sendFriendRequest = (
  code: string,
  myDisplay: string
): Promise<PendingSentRequest> => invoke("send_friend_request", { code, myDisplay });

/** Retry queued deliveries, consume acceptances, and list requests. */
export const syncFriends = (myDisplay: string): Promise<FriendsSync> =>
  invoke("sync_friends", { myDisplay });

/** Accept or decline an incoming friend request. */
export const respondFriendRequest = (
  id: string,
  code: string,
  accept: boolean,
  myDisplay: string
): Promise<void> => invoke("respond_friend_request", { id, code, accept, myDisplay });

/** Withdraw a pending sent request (local; their parked copy can't be recalled). */
export const cancelFriendRequest = (peerId: string): Promise<void> =>
  invoke("cancel_friend_request", { peerId });

/** Re-attempt delivery of a pending request right now (their node dedupes). */
export const resendFriendRequest = (
  peerId: string,
  myDisplay: string
): Promise<PendingSentRequest> => invoke("resend_friend_request", { peerId, myDisplay });

/** Decode a pasted fr code locally (drives the send-button pending state). */
export const peekContactCode = (code: string): Promise<CodePeek> =>
  invoke("peek_contact_code", { code });

/** Fired when the friends list changes (request accepted either direction). */
export const onFriendsChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("friends-changed", () => handler());

/** A blocked contact (scaffold; enforcement arrives with federation + bans). */
export interface BlockDto {
  id: string;
  name: string;
}

/** Block a contact by id (covers their alts once enforcement lands). */
export const blockContact = (id: string, name: string): Promise<void> =>
  invoke("block_contact", { id, name });

/** Unblock a contact by id. */
export const unblockContact = (id: string): Promise<void> =>
  invoke("unblock_contact", { id });

/** List blocked contacts. */
export const listBlocks = (): Promise<BlockDto[]> => invoke("list_blocks");

/** Result of opening a DM with a contact: the DM group + the backend session id
 * of the contact's host it lives on. */
export interface OpenedDm {
  serverId: string;
  group: GroupDto;
}

/** Open a DM with a saved contact on the contact's own home node. `myDisplay` is
 * the name the contact sees the DM come from. */
export const openContactDm = (contactId: string, myDisplay: string): Promise<OpenedDm> =>
  invoke("open_contact_dm", { contactId, myDisplay });

/** A DM conversation in the Direct Messages list. */
export interface DmConversation {
  serverId: string;
  groupId: string;
  peerId: string;
  peerName: string;
  fingerprint: string;
}

/** List DM conversations across the home + contact-DM sessions. */
export const listDms = (): Promise<DmConversation[]> => invoke("list_dms");

/** Fired when the DM conversation list changes (e.g. a persisted DM reconnected
 * after login). Refresh the list. */
export const onDmsChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("dms-changed", () => handler());

/** Send a plaintext message to a public channel. */
export const sendPublicMessage = (
  groupId: string,
  content: string
): Promise<void> => invoke("send_public_message", { groupId, content });

/** Fetch recent history for a public channel (oldest-first). */
export const fetchPublicHistory = (groupId: string): Promise<MessageDto[]> =>
  invoke("fetch_public_history", { groupId });

/** One slot of loaded private history. `message` is null while the (received,
 * encrypted) record is still being decrypted; the UI shows a placeholder until
 * the matching `onPrivateHistoryDecrypted` event arrives. */
export interface HistoryEntry {
  id: string;
  message: PrivateMessageDto | null;
}

/** Fetch a private group's recent history (oldest-first). Plaintext records come
 * back immediately; encrypted ones come back as placeholders and stream in via
 * onPrivateHistoryDecrypted as they decrypt. */
export const fetchPrivateHistory = (
  groupId: string,
  limit?: number
): Promise<HistoryEntry[]> =>
  invoke("fetch_private_history", { groupId, limit: limit ?? null });

/** Fired as each placeholder history slot finishes decrypting. */
export const onPrivateHistoryDecrypted = (
  handler: (payload: { id: string; message: PrivateMessageDto }) => void
): Promise<UnlistenFn> =>
  listen<{ id: string; message: PrivateMessageDto }>(
    "private-history-decrypted",
    (e) => handler(e.payload)
  );

/** Who may send me a friend request. */
export type FriendRequestPolicy = "everyone" | "tavern_members" | "friends_of_friends" | "no_one";

/** A rendezvous / mailbox node the user routes DMs through. `mine` = your own
 * trusted node; otherwise a public node whose operator may log metadata. */
export interface RendezvousNode {
  url: string;
  label: string;
  mine: boolean;
}

/** Which Yggdrasil peers the mesh connects through. */
export type YggPeerMode = "authorized" | "private" | "public";

/** Local client settings. */
export interface Settings {
  encryptAtRest: boolean;
  friendRequestPolicy: FriendRequestPolicy;
  meshEnabled: boolean;
  rendezvousNode: RendezvousNode | null;
  yggPeerMode: YggPeerMode;
  yggPrivatePeers: string[];
}

/** Read the current client settings. */
export const getSettings = (): Promise<Settings> => invoke("get_settings");

/** Toggle "encrypt my own messages at rest" (received messages are always
 * encrypted regardless). */
export const setEncryptAtRest = (enabled: boolean): Promise<void> =>
  invoke("set_encrypt_at_rest", { enabled });

/** Set who may send me a friend request (enforced once request delivery lands). */
export const setFriendRequestPolicy = (policy: FriendRequestPolicy): Promise<void> =>
  invoke("set_friend_request_policy", { policy });

/** Set (or clear with null) the rendezvous node to route DMs through. */
export const setRendezvousNode = (node: RendezvousNode | null): Promise<void> =>
  invoke("set_rendezvous_node", { node });

/** Start an end-to-end encrypted DM with a user by username. */
export const startDm = (username: string): Promise<GroupDto> =>
  invoke("start_dm", { username });

/** Encrypt + send a message to a private (MLS) channel. */
export const sendPrivateMessage = (
  groupId: string,
  content: string
): Promise<void> => invoke("send_private_message", { groupId, content });

/** Subscribe to decrypted incoming private messages. */
export const onIncomingPrivateMessage = (
  handler: (msg: PrivateMessageDto) => void
): Promise<UnlistenFn> =>
  listen<PrivateMessageDto>("incoming-private-message", (e) => handler(e.payload));

/** Fired when this device joins a group from a Welcome (refresh the list). */
export const onJoinedGroup = (
  handler: (payload: { serverId: string; groupId: string }) => void
): Promise<UnlistenFn> =>
  listen<{ serverId: string; groupId: string }>("joined-group", (e) => handler(e.payload));

/** Fired when a server's live message stream connects or drops (auto-reconnects). */
export const onConnection = (
  handler: (status: { serverId: string; connected: boolean }) => void
): Promise<UnlistenFn> =>
  listen<{ serverId: string; connected: boolean }>("connection-status", (e) =>
    handler(e.payload)
  );

// ---- Mesh networking (Settings) ---------------------------------------------
// Dev tooling (start/stop server, logs, factory reset) lives in the native Dev
// menu, not in the webview.

export interface MeshStatus {
  running: boolean;
  address: string | null;
  available: boolean;
  /** Persisted preference; enabled && !running means the start failed. */
  enabled: boolean;
}

/** Current mesh status (Settings). */
export const getMeshStatus = (): Promise<MeshStatus> => invoke("get_mesh_status");

/** Enable/disable the mesh (persisted; auto-started on launch when enabled). */
export const setMeshEnabled = (enabled: boolean): Promise<MeshStatus> =>
  invoke("set_mesh_enabled", { enabled });

/** One line of mesh connect progress: orange connecting, red error, green
 * connected, idle after a disconnect. */
export interface MeshConnectStatus {
  state: "connecting" | "connected" | "error" | "idle";
  message: string;
}

/** Connect the mesh through a peer mode (persisted); progress arrives via
 * `onMeshConnectStatus`. */
export const meshConnect = (
  mode: YggPeerMode,
  privatePeers: string[]
): Promise<MeshStatus> => invoke("mesh_connect", { mode, privatePeers });

/** Disconnect the mesh and turn auto-start off. */
export const meshDisconnect = (): Promise<MeshStatus> => invoke("mesh_disconnect");

/** Subscribe to mesh connect-progress lines. */
export const onMeshConnectStatus = (
  handler: (status: MeshConnectStatus) => void
): Promise<UnlistenFn> =>
  listen<MeshConnectStatus>("mesh-connect-status", (e) => handler(e.payload));

// ---- Create / Join server (invite keys) ------------------------------------

export interface InviteInfo {
  endpoint: string;
  token: string;
  transport: string; // "direct" | "mesh"
  peers: string[];
  name: string | null;
  cert: string | null;
}

/** Localhost endpoint + TLS cert the owner uses to connect to their new server. */
export interface HostInfo {
  endpoint: string;
  cert: string | null;
}

/** Host a new private (invite-only, TLS) server in-process. */
export const hostPrivateServer = (): Promise<HostInfo> => invoke("host_private_server");

/** Host a new public (open, TLS) server in-process. */
export const hostPublicServer = (): Promise<HostInfo> => invoke("host_public_server");

/** Owner-only: mint + return a shareable invite key for the hosted server. */
export const createInviteKey = (): Promise<string> => invoke("create_invite_key");

/** Decode an invite key into its parts (endpoint, token, transport, peers). */
export const decodeInvite = (key: string): Promise<InviteInfo> =>
  invoke("decode_invite", { key });

/** Prepare mesh transport for a join (persist peers + start the mesh node). */
export const prepareMesh = (peers: string[]): Promise<string> =>
  invoke("prepare_mesh", { peers });

/**
 * Subscribe to real-time incoming messages pushed from the Rust message stream.
 * Returns an unlisten function to call on cleanup.
 */
export const onIncomingMessage = (
  handler: (msg: MessageDto) => void
): Promise<UnlistenFn> =>
  listen<MessageDto>("incoming-message", (event) => handler(event.payload));
