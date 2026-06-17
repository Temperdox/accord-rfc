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
  /** "text" | "voice" for public channels (DMs/private are "text"). */
  channelKind: string;
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

/** Password-less key login (taverns): authenticates with this server's derived
 * identity key via a signed challenge. No password - the master key is created
 * at home signup and protected by that one password for backup/recovery. */
export const loginWithKey = (
  username: string,
  deviceName: string
): Promise<LoginInfo> => invoke("login_with_key", { username, deviceName });

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
  /** Max taverns this client will host at once (default 16). */
  maxHostedTaverns: number;
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

/** Set the max number of taverns this client will host at once (clamped 1–200
 * server-side). Higher values consume more ports starting at 50052. */
export const setMaxHostedTaverns = (max: number): Promise<void> =>
  invoke("set_max_hosted_taverns", { max });

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

/** Mint + return a shareable invite key for the tavern with rail id `serverId`
 * (the active tavern). Server gates this on CREATE_INVITE. */
export const createInviteKey = (serverId: string): Promise<string> =>
  invoke("create_invite_key", { serverId });

/** Decode an invite key into its parts (endpoint, token, transport, peers). */
export const decodeInvite = (key: string): Promise<InviteInfo> =>
  invoke("decode_invite", { key });

/** Prepare mesh transport for a join (persist peers + start the mesh node). */
export const prepareMesh = (peers: string[]): Promise<string> =>
  invoke("prepare_mesh", { peers });

// ---- Hosting your own taverns (multi-instance) ------------------------------

/** Connect info for a tavern this client hosts (own in-process server instance). */
export interface TavernConnect {
  id: string;
  name: string;
  endpoint: string;
  cert: string | null;
}

/** Create + host a new PRIVATE tavern (its own server instance: own port, DB,
 * TLS cert). Returns connect info; the caller then registers (owner) + logs in +
 * adds it to the rail. Public taverns aren't hostable yet (no central nodes). */
export const createTavern = (name: string): Promise<TavernConnect> =>
  invoke("create_tavern", { name });

/** Re-spawn all persisted hosted taverns (called after login); returns the
 * connect info for each that came back up so the UI can re-attach them. */
export const resumeHostedTaverns = (): Promise<TavernConnect[]> =>
  invoke("resume_hosted_taverns");

/** Delete a tavern you host (stops it + wipes its data). Irreversible. */
export const deleteTavern = (id: string): Promise<void> =>
  invoke("delete_tavern", { id });

/**
 * Subscribe to real-time incoming messages pushed from the Rust message stream.
 * Returns an unlisten function to call on cleanup.
 */
export const onIncomingMessage = (
  handler: (msg: MessageDto) => void
): Promise<UnlistenFn> =>
  listen<MessageDto>("incoming-message", (event) => handler(event.payload));

// ---- Taverns: channels, members, identity, moderation ----------------------

/** Permission bits (mirror of accord-types::perms; serialized as decimal u64
 * strings - we test bits with BigInt). Only the ones the UI gates on. */
export const PERM = {
  ADMINISTRATOR: 1n << 0n,
  VIEW_CHANNELS: 1n << 1n,
  SEND_MESSAGES: 1n << 2n,
  MANAGE_MESSAGES: 1n << 3n,
  MANAGE_CHANNELS: 1n << 4n,
  MANAGE_ROLES: 1n << 5n,
  CREATE_INVITE: 1n << 6n,
  KICK_MEMBERS: 1n << 7n,
  BAN_MEMBERS: 1n << 8n,
  MANAGE_SERVER: 1n << 9n,
  CREATE_PRIVATE_CHAT: 1n << 10n,
  MENTION_EVERYONE: 1n << 11n,
  ATTACH_FILES: 1n << 12n,
  ADD_REACTIONS: 1n << 13n,
};

/** One togglable permission, with a clear label and a full description. */
export interface PermDef {
  bit: bigint;
  key: string;
  label: string;
  desc: string;
}
/** A category of related permissions, shown under a header in the role editor. */
export interface PermGroup {
  category: string;
  perms: PermDef[];
}

/** All togglable permissions, grouped by what they affect. Only the bits the
 * server actually enforces appear here (accord-types::perms). ADMINISTRATOR is
 * isolated in its own "Advanced" group and flagged as override-everything. */
export const PERM_GROUPS: PermGroup[] = [
  {
    category: "General Server Permissions",
    perms: [
      {
        bit: PERM.VIEW_CHANNELS,
        key: "VIEW_CHANNELS",
        label: "View Channels",
        desc: "Allows members to view channels by default. Without this, a member sees no channels in the tavern.",
      },
      {
        bit: PERM.MANAGE_CHANNELS,
        key: "MANAGE_CHANNELS",
        label: "Manage Channels",
        desc: "Allows members to create, rename, and delete text and voice channels in this tavern.",
      },
      {
        bit: PERM.MANAGE_ROLES,
        key: "MANAGE_ROLES",
        label: "Manage Roles",
        desc: "Allows members to create new roles and edit or delete roles lower than their highest role. They can never grant a permission they do not already have.",
      },
      {
        bit: PERM.MANAGE_SERVER,
        key: "MANAGE_SERVER",
        label: "Manage Tavern",
        desc: "Allows members to change the tavern's name, description, and icon, and to configure tavern-wide settings such as AutoMod.",
      },
    ],
  },
  {
    category: "Membership Permissions",
    perms: [
      {
        bit: PERM.CREATE_INVITE,
        key: "CREATE_INVITE",
        label: "Create Invite",
        desc: "Allows members to generate invite keys that let new people join this tavern.",
      },
      {
        bit: PERM.KICK_MEMBERS,
        key: "KICK_MEMBERS",
        label: "Kick Members",
        desc: "Allows members to remove other members from this tavern. A kicked member can rejoin with a new invite. You can only kick members ranked below your highest role.",
      },
      {
        bit: PERM.BAN_MEMBERS,
        key: "BAN_MEMBERS",
        label: "Ban Members",
        desc: "Allows members to permanently ban accounts from this tavern. You can only ban members ranked below your highest role; the owner can never be banned.",
      },
    ],
  },
  {
    category: "Text & Messaging Permissions",
    perms: [
      {
        bit: PERM.SEND_MESSAGES,
        key: "SEND_MESSAGES",
        label: "Send Messages",
        desc: "Allows members to send messages in text channels they can view.",
      },
      {
        bit: PERM.CREATE_PRIVATE_CHAT,
        key: "CREATE_PRIVATE_CHAT",
        label: "Start Private Chats",
        desc: "Allows members to open end-to-end-encrypted private DMs with other members.",
      },
      {
        bit: PERM.ATTACH_FILES,
        key: "ATTACH_FILES",
        label: "Attach Files",
        desc: "Allows members to upload and share files and media in text channels.",
      },
      {
        bit: PERM.ADD_REACTIONS,
        key: "ADD_REACTIONS",
        label: "Add Reactions",
        desc: "Allows members to add new emoji reactions to messages. Members can always use reactions already present on a message.",
      },
      {
        bit: PERM.MENTION_EVERYONE,
        key: "MENTION_EVERYONE",
        label: "Mention @everyone and @here",
        desc: "Allows members to use @everyone (everyone in the tavern) and @here (online members) to notify many people at once.",
      },
      {
        bit: PERM.MANAGE_MESSAGES,
        key: "MANAGE_MESSAGES",
        label: "Manage Messages",
        desc: "Allows members to delete messages sent by other members and to pin or unpin any message.",
      },
    ],
  },
  {
    category: "Advanced Permissions",
    perms: [
      {
        bit: PERM.ADMINISTRATOR,
        key: "ADMINISTRATOR",
        label: "Administrator",
        desc: "Members with this permission have every permission and bypass all channel-specific restrictions. This is the most dangerous permission to grant - hand it out carefully.",
      },
    ],
  },
];

/** The caller's effective permissions on the active server. */
export interface MyPerms {
  permissions: string; // decimal u64
  isOwner: boolean;
}

/** Whether `perms` grant `bit` (owner/ADMINISTRATOR short-circuit). */
export const can = (perms: MyPerms | null, bit: bigint): boolean => {
  if (!perms) return false;
  if (perms.isOwner) return true;
  const p = BigInt(perms.permissions);
  if (p & PERM.ADMINISTRATOR) return true;
  return (p & bit) !== 0n;
};

/** Fetch the caller's effective permissions (gates admin affordances). */
export const getMyPermissions = (): Promise<MyPerms> => invoke("get_my_permissions");

/** A role. `permissions` is a decimal-string u64; `position` orders power
 * (higher = more power, @everyone = 0); `icon` is a base64 data URL or "". */
export interface RoleDto {
  id: string;
  name: string;
  permissions: string;
  position: number;
  isDefault: boolean;
  color: string;
  icon: string;
  hoist: boolean;
  mentionable: boolean;
}

/** The editable display + behaviour fields of a role. */
export interface RoleWrite {
  name: string;
  permissions: string;
  color: string;
  icon: string;
  hoist: boolean;
  mentionable: boolean;
}

/** List the tavern's roles (highest power first; @everyone last). */
export const listRoles = (): Promise<RoleDto[]> => invoke("list_roles");

/** Create a role (gated by MANAGE_ROLES, with anti-escalation). */
export const createRole = (w: RoleWrite): Promise<RoleDto> =>
  invoke("create_role", {
    name: w.name,
    permissions: w.permissions,
    color: w.color,
    icon: w.icon,
    hoist: w.hoist,
    mentionable: w.mentionable,
  });

/** Update a role's name, permission bits, and display fields. */
export const updateRole = (id: string, w: RoleWrite): Promise<RoleDto> =>
  invoke("update_role", {
    id,
    name: w.name,
    permissions: w.permissions,
    color: w.color,
    icon: w.icon,
    hoist: w.hoist,
    mentionable: w.mentionable,
  });

/** Reorder roles top-to-bottom (highest power first); omit @everyone. */
export const reorderRoles = (roleIds: string[]): Promise<void> =>
  invoke("reorder_roles", { roleIds });

/** Delete a role (the @everyone default cannot be deleted). */
export const deleteRole = (id: string): Promise<void> => invoke("delete_role", { id });

/** Assign a role to a member. */
export const assignRole = (userId: string, roleId: string): Promise<void> =>
  invoke("assign_role", { userId, roleId });

/** Remove a role from a member. */
export const unassignRole = (userId: string, roleId: string): Promise<void> =>
  invoke("unassign_role", { userId, roleId });

/** Create a public channel (text or voice). Gated server-side. */
export const createChannel = (
  name: string,
  channelKind: "text" | "voice",
  description?: string
): Promise<GroupDto> =>
  invoke("create_channel", { name, channelKind, description: description ?? "" });

/** Delete a public channel. Gated server-side. */
export const deleteChannel = (groupId: string): Promise<void> =>
  invoke("delete_channel", { groupId });

/** A tavern (server) member for the member list. */
export interface MemberDto {
  userId: string;
  username: string;
  displayName: string;
  isOwner: boolean;
  online: boolean;
  roleIds: string[];
}

/** List the members of a channel/server. */
export const listMembers = (groupId: string): Promise<MemberDto[]> =>
  invoke("list_members", { groupId });

/** Tavern (server) identity. `iconUrl`/`bannerUrl` are base64 data URLs or "". */
export interface TavernDto {
  name: string;
  iconUrl: string;
  description: string;
  linkingEnabled: boolean;
  bannerUrl: string;
}

/** Fetch the active tavern's identity. */
export const getTavern = (): Promise<TavernDto> => invoke("get_tavern");

/** Fetch a specific server's tavern identity without switching sessions (used
 * to show every rail server's icon, not just the active one). */
export const getTavernFor = (serverId: string): Promise<TavernDto> =>
  invoke("get_tavern_for", { serverId });

/** Update the tavern identity (gated by MANAGE_SERVER). Pass every field; omit
 * one (undefined) to clear it. */
export const updateTavern = (
  name: string,
  iconUrl?: string,
  description?: string,
  bannerUrl?: string
): Promise<TavernDto> =>
  invoke("update_tavern", {
    name,
    iconUrl: iconUrl ?? "",
    description: description ?? "",
    bannerUrl: bannerUrl ?? "",
  });

/** Kick a member from a channel (gated by KICK_MEMBERS). */
export const kickMember = (groupId: string, userId: string): Promise<void> =>
  invoke("kick_member", { groupId, userId });

/** Ban an account from the server (gated by BAN_MEMBERS). */
export const banMember = (userId: string, reason?: string): Promise<void> =>
  invoke("ban_member", { userId, reason: reason ?? "" });

/** Lift a ban (gated by BAN_MEMBERS). */
export const unbanMember = (userId: string): Promise<void> =>
  invoke("unban_member", { userId });

/** A ban entry. */
export interface BanDto {
  userId: string;
  reason: string;
  bannedBy: string;
  createdAtMs: number;
}

/** List the server's bans (gated by BAN_MEMBERS). */
export const listBans = (): Promise<BanDto[]> => invoke("list_bans");

/** A moderation audit-log entry. */
export interface AuditEntry {
  actorId: string;
  action: string;
  target: string;
  verdict: string;
  reason: string;
  createdAtMs: number;
}

/** The moderation audit log, newest first (gated by MANAGE_SERVER). */
export const listAudit = (limit?: number): Promise<AuditEntry[]> =>
  invoke("list_audit", { limit: limit ?? 100 });

/** Payload of the `mod-alert` event (guardrail decision shown to admins). */
export interface ModAlert {
  serverId: string;
  actorId: string;
  action: string;
  target: string;
  reason: string;
  severity: "info" | "warn" | "hostile";
  timestampMs: number;
}

/** Subscribe to moderation alerts (owner/admins only receive these). */
export const onModAlert = (handler: (a: ModAlert) => void): Promise<UnlistenFn> =>
  listen<ModAlert>("mod-alert", (e) => handler(e.payload));

// ---- Voice/video (scaffold) -------------------------------------------------
// Media is WebRTC P2P in the webview (src/voice.ts, currently stubbed); these
// commands carry only signaling over the message stream.

/** Join a voice channel (announces presence; webview then negotiates WebRTC). */
export const joinVoice = (groupId: string): Promise<void> =>
  invoke("join_voice", { groupId });

/** Leave a voice channel. */
export const leaveVoice = (groupId: string): Promise<void> =>
  invoke("leave_voice", { groupId });

/** Update mic/camera/screen flags while in a voice channel. */
export const setVoiceState = (
  groupId: string,
  muted: boolean,
  cameraOn: boolean,
  screenOn: boolean
): Promise<void> => invoke("set_voice_state", { groupId, muted, cameraOn, screenOn });

/** Relay a WebRTC signaling envelope to a peer device. */
export const sendVoiceSignal = (
  groupId: string,
  targetDevice: string,
  kind: "offer" | "answer" | "ice",
  data: number[]
): Promise<void> => invoke("send_voice_signal", { groupId, targetDevice, kind, data });

/** A voice participant's state change. */
export interface VoiceParticipant {
  serverId: string;
  groupId: string;
  userId: string;
  deviceId: string;
  joined: boolean;
  muted: boolean;
  cameraOn: boolean;
  screenOn: boolean;
}

/** Subscribe to voice participant updates. */
export const onVoiceParticipant = (
  handler: (p: VoiceParticipant) => void
): Promise<UnlistenFn> =>
  listen<VoiceParticipant>("voice-participant", (e) => handler(e.payload));

/** A relayed WebRTC signaling envelope. */
export interface VoiceSignal {
  serverId: string;
  groupId: string;
  fromDevice: string;
  kind: "offer" | "answer" | "ice" | "unknown";
  data: number[];
}

/** Subscribe to relayed voice signaling (consumed by the WebRTC layer). */
export const onVoiceSignal = (handler: (s: VoiceSignal) => void): Promise<UnlistenFn> =>
  listen<VoiceSignal>("voice-signal", (e) => handler(e.payload));
