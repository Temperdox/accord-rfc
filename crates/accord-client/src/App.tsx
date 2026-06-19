/**
 * Accord client - top-level UI (SolidJS).
 *
 * Flow (Discord-like):
 * 1. <AuthScreen/> - log in or sign up. This is account-first: signing up quietly
 *    stands up your own embedded "home" server and registers you there, so you
 *    never deal with server config to get in.
 * 2. <Home/> - land on your home server (channels + DMs). Adding or joining other
 *    servers is an in-app action via the server rail's "+", not part of sign-in.
 *
 * All server interaction goes through the typed helpers in `api.ts`.
 */
import {
  For,
  Match,
  Show,
  Switch,
  createEffect,
  createSignal,
  onCleanup,
  onMount,
  untrack,
} from "solid-js";
import Fa from "solid-fa";
import {
  faChevronDown,
  faChevronRight,
  faComments,
  faHeadphones,
  faVolumeXmark,
  faDesktop,
  faFaceSmile,
  faGear,
  faGlobe,
  faHashtag,
  faLock,
  faMicrophone,
  faMicrophoneSlash,
  faNoteSticky,
  faPaperPlane,
  faPaperclip,
  faPhone,
  faPhoneSlash,
  faPlus,
  faRightToBracket,
  faTrash,
  faTriangleExclamation,
  faUserGroup,
  faVideo,
  faVolumeHigh,
  faUserPlus,
  faXmark,
} from "@fortawesome/free-solid-svg-icons";
import type { IconDefinition } from "@fortawesome/fontawesome-svg-core";
import * as api from "./api";
import type { GroupDto } from "./api";
import * as voice from "./voice";
import * as voicePrefsMod from "./voicePrefs";
import NotificationBar from "./NotificationBar";
import { dismissKey, notify, notifyTransient } from "./notifications";

/** A server the user is signed in to (their home, or one they joined). */
interface ServerSession {
  id: string;
  name: string;
  endpoint: string;
  cert: string | null;
  username: string;
  password: string;
}

/** One row in the custom context menu. `sep` renders a divider above the item. */
interface MenuItem {
  label: string;
  icon?: IconDefinition;
  danger?: boolean;
  sep?: boolean;
  onClick: () => void;
}

/** Options for the custom confirmation dialog (replaces native window.confirm). */
interface ConfirmOpts {
  title: string;
  body: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
}

/** Preset role-color swatches (members use their highest role's color). */
const ROLE_COLORS = [
  "#1abc9c", "#2ecc71", "#3498db", "#9b59b6", "#e91e63", "#f1c40f",
  "#e67e22", "#e74c3c", "#95a5a6", "#607d8b", "#11806a", "#1f8b4c",
  "#206694", "#71368a", "#ad1457", "#c27c0e", "#a84300", "#992d22",
];

export default function App() {
  // Dev tooling lives in the native Dev menu (debug builds only) - there is
  // deliberately no in-app dev banner.
  const [session, setSession] = createSignal<ServerSession | null>(null);
  return (
    <>
      {/* Full-width header bar (sits above the app; #root is a flex column). */}
      <NotificationBar />
      <Show when={session()} fallback={<AuthScreen onAuthed={setSession} />}>
        <Home home={session()!} />
      </Show>
    </>
  );
}

/** Account-first landing: pick a saved account, log in, or sign up. Accounts live
 * on this device's home server (no recovery yet), so known ones show as pills. */
function AuthScreen(props: { onAuthed: (s: ServerSession) => void }) {
  const [accounts, setAccounts] = createSignal<api.AccountPill[]>([]);
  const [mode, setMode] = createSignal<"pick" | "login" | "signup">("pick");
  const [picked, setPicked] = createSignal<string | null>(null);
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  onMount(async () => {
    const list = await api.listAccounts().catch(() => [] as api.AccountPill[]);
    setAccounts(list);
    setMode(list.length > 0 ? "pick" : "signup");
  });

  const authenticate = async (isSignup: boolean) => {
    setError(null);
    if (isSignup && password() !== confirm()) {
      setError("Passwords don't match.");
      return;
    }
    if (!username().trim() || !password()) {
      setError("Enter a username and password.");
      return;
    }
    setBusy(true);
    try {
      // Stand up (or reuse) the embedded home server and auth against it.
      const host = await api.hostPrivateServer();
      await api.connect("home", host.endpoint, host.cert);
      if (isSignup) await api.register(username(), password(), username());
      await api.login(username(), password(), "Desktop");
      props.onAuthed({
        id: "home",
        name: "Home",
        endpoint: host.endpoint,
        cert: host.cert,
        username: username(),
        password: password(),
      });
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const switchMode = (m: "pick" | "login" | "signup") => {
    setMode(m);
    setPicked(null);
    setUsername("");
    setPassword("");
    setConfirm("");
    setError(null);
  };

  const pick = (name: string) => {
    setPicked(name);
    setUsername(name);
    setPassword("");
    setError(null);
  };

  return (
    <div class="auth">
      <div class="auth-card">
        <h1 class="brand">Accord</h1>

        <Switch>
          <Match when={mode() === "pick"}>
            <p class="subtitle">Welcome back. Choose an account.</p>
            <div class="account-pills">
              <For each={accounts()}>
                {(a) => (
                  <button
                    class={`account-pill ${picked() === a.username ? "active" : ""}`}
                    onClick={() => pick(a.username)}
                  >
                    <span class="pill-avatar">
                      <Show when={a.avatar} fallback={(a.username[0] ?? "?").toUpperCase()}>
                        <img src={a.avatar} alt="" />
                      </Show>
                    </span>
                    <span class="pill-name">{a.username}</span>
                    <Show when={a.isMain}>
                      <span class="pill-tag">main</span>
                    </Show>
                  </button>
                )}
              </For>
            </div>
            <Show when={picked()}>
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  authenticate(false);
                }}
              >
                <div class="field">
                  <label class="field-label" for="pick-password">
                    Password
                  </label>
                  <input
                    id="pick-password"
                    type="password"
                    autofocus
                    value={password()}
                    onInput={(e) => setPassword(e.currentTarget.value)}
                  />
                </div>
                <Show when={error()}>
                  <div class="error">{error()}</div>
                </Show>
                <button disabled={busy()}>Log in as {picked()}</button>
              </form>
            </Show>
            <Show when={!picked() && error()}>
              <div class="error">{error()}</div>
            </Show>
            <div class="auth-alt">
              <button class="link" onClick={() => switchMode("signup")}>
                Add an account
              </button>
              <button class="link" onClick={() => switchMode("login")}>
                Use another account
              </button>
            </div>
          </Match>

          <Match when={mode() === "login"}>
            <p class="subtitle">Log in.</p>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                authenticate(false);
              }}
            >
              <div class="field">
                <label class="field-label">Username</label>
                <input
                  autofocus
                  value={username()}
                  onInput={(e) => setUsername(e.currentTarget.value)}
                />
              </div>
              <div class="field">
                <label class="field-label">Password</label>
                <input
                  type="password"
                  value={password()}
                  onInput={(e) => setPassword(e.currentTarget.value)}
                />
              </div>
              <Show when={error()}>
                <div class="error">{error()}</div>
              </Show>
              <button disabled={busy()}>Log in</button>
            </form>
            <div class="auth-alt">
              <Show when={accounts().length > 0}>
                <button class="link" onClick={() => switchMode("pick")}>
                  Back to accounts
                </button>
              </Show>
              <button class="link" onClick={() => switchMode("signup")}>
                Create an account
              </button>
            </div>
          </Match>

          <Match when={mode() === "signup"}>
            <p class="subtitle">Create your account.</p>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                authenticate(true);
              }}
            >
              <div class="field">
                <label class="field-label">Username</label>
                <input
                  autofocus
                  value={username()}
                  onInput={(e) => setUsername(e.currentTarget.value)}
                />
              </div>
              <div class="field">
                <label class="field-label">Password</label>
                <input
                  type="password"
                  value={password()}
                  onInput={(e) => setPassword(e.currentTarget.value)}
                />
              </div>
              <div class="field">
                <label class="field-label">Confirm password</label>
                <input
                  type="password"
                  value={confirm()}
                  onInput={(e) => setConfirm(e.currentTarget.value)}
                />
              </div>
              <Show when={error()}>
                <div class="error">{error()}</div>
              </Show>
              <button disabled={busy()}>Create account</button>
            </form>
            <div class="auth-alt">
              <Show when={accounts().length > 0}>
                <button class="link" onClick={() => switchMode("pick")}>
                  Back to accounts
                </button>
              </Show>
            </div>
          </Match>
        </Switch>

        <p class="hint auth-foot">
          Your account lives on your own device-hosted home server. After signing in
          you can create or join other taverns from the rail.
        </p>
      </div>
    </div>
  );
}

/** A message as rendered in the UI (public or decrypted private). */
interface UiMessage {
  id: string;
  groupId: string;
  author: string;
  /** Sender's user id (public channels), for resolving their avatar; "" if unknown. */
  senderId?: string;
  content: string;
  timestampMs: number;
  /** True while a received (encrypted) history message is still decrypting. */
  pending?: boolean;
}

/** The signed-in app: server rail + channel sidebar + message view. */
function Home(props: { home: ServerSession }) {
  const [servers, setServers] = createSignal<ServerSession[]>([props.home]);
  const [activeServerId, setActiveServerId] = createSignal(props.home.id);
  const [addOpen, setAddOpen] = createSignal(false);
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  // "dms" shows the Direct Messages home (the embedded home server is the hidden
  // backbone); "server" shows a joined/created server's channels.
  const [view, setView] = createSignal<"dms" | "server">("dms");
  // Within DMs: "friends", "requests", or a contact id (a conversation).
  const [dmSel, setDmSel] = createSignal<string>("friends");
  const [contacts, setContacts] = createSignal<api.ContactDto[]>([]);
  const [blocks, setBlocks] = createSignal<api.BlockDto[]>([]);
  const [myCode, setMyCode] = createSignal("");
  const [codePaste, setCodePaste] = createSignal("");
  const [frIncoming, setFrIncoming] = createSignal<api.IncomingFriendRequest[]>([]);
  const [frPending, setFrPending] = createSignal<api.PendingSentRequest[]>([]);
  const [frNotice, setFrNotice] = createSignal<string | null>(null);
  // Send-button lifecycle: idle -> sending -> a 1s "Sent" flash -> idle. While
  // the pasted code's peer already has a pending request, the button grays out.
  const [frSendState, setFrSendState] = createSignal<"idle" | "sending" | "sent">("idle");
  // Who the pasted code identifies (decoded locally as the user types/pastes).
  const [pasteId, setPasteId] = createSignal<string | null>(null);
  const pastePending = () => {
    const id = pasteId();
    return !!id && frPending().some((p) => p.peerId === id);
  };
  // contactId -> the opened DM (its backend session + group), once established.
  const [dmRoutes, setDmRoutes] = createSignal<Record<string, api.OpenedDm>>({});
  const [dmOpening, setDmOpening] = createSignal(false);
  const [dmConversations, setDmConversations] = createSignal<api.DmConversation[]>([]);
  const [activeConv, setActiveConv] = createSignal<api.DmConversation | null>(null);
  const isBlocked = (id: string) => blocks().some((b) => b.id === id);
  // Unread message count per server, shown as a badge on inactive rail servers.
  const [unread, setUnread] = createSignal<Record<string, number>>({});
  const bumpUnread = (serverId: string) =>
    setUnread((u) => ({ ...u, [serverId]: (u[serverId] ?? 0) + 1 }));

  const [groups, setGroups] = createSignal<GroupDto[]>([]);
  const [activeId, setActiveId] = createSignal<string | null>(null);
  const [messages, setMessages] = createSignal<UiMessage[]>([]);
  const [draft, setDraft] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [invite, setInvite] = createSignal<{ key: string; error: string } | null>(null);
  const [encryptAtRest, setEncryptAtRest] = createSignal(false);
  const [friendPolicy, setFriendPolicy] = createSignal<api.FriendRequestPolicy>("everyone");
  const [mesh, setMesh] = createSignal<api.MeshStatus | null>(null);
  const [settingsTab, setSettingsTab] = createSignal<
    "profile" | "privacy" | "voice" | "friends" | "network" | "nodes"
  >("profile");
  // Voice & Video prefs (client-only; persisted in localStorage) + device lists.
  const [voicePrefs, setVoicePrefsSig] = createSignal<voicePrefsMod.VoicePrefs>(
    voicePrefsMod.loadVoicePrefs()
  );
  const [audioInputs, setAudioInputs] = createSignal<MediaDeviceInfo[]>([]);
  const [audioOutputs, setAudioOutputs] = createSignal<MediaDeviceInfo[]>([]);
  // Mic test (loopback + meter): the running test's stop fn + its live level.
  const [micTestLevel, setMicTestLevel] = createSignal(0);
  let micTestStop: (() => void) | null = null;
  const micTesting = () => micTestStop !== null;
  async function toggleMicTest() {
    if (micTestStop) {
      micTestStop();
      micTestStop = null;
      setMicTestLevel(0);
      return;
    }
    try {
      micTestStop = await voice.startMicTest(setMicTestLevel);
    } catch (e) {
      setError(String(e));
    }
  }
  /** Stop the mic test if running (on settings close / tab switch). */
  function stopMicTest() {
    if (micTestStop) {
      micTestStop();
      micTestStop = null;
      setMicTestLevel(0);
    }
  }
  void voice.setAudioPrefs(voicePrefs()); // apply saved prefs to the voice layer
  /** Patch + persist voice prefs and push them to the live voice layer. */
  function updateVoicePrefs(patch: Partial<voicePrefsMod.VoicePrefs>) {
    const next = { ...voicePrefs(), ...patch };
    setVoicePrefsSig(next);
    voicePrefsMod.saveVoicePrefs(next);
    void voice.setAudioPrefs(next);
  }
  /** Enumerate audio input/output devices (labels appear after mic permission). */
  async function loadAudioDevices() {
    try {
      const devs = await navigator.mediaDevices.enumerateDevices();
      setAudioInputs(devs.filter((d) => d.kind === "audioinput"));
      setAudioOutputs(devs.filter((d) => d.kind === "audiooutput"));
    } catch {
      /* enumeration unavailable */
    }
  }
  const [yggMode, setYggMode] = createSignal<api.YggPeerMode>("public");
  const [yggPeersText, setYggPeersText] = createSignal("");
  const [meshConn, setMeshConn] = createSignal<api.MeshConnectStatus | null>(null);
  const [rendezvous, setRendezvous] = createSignal<api.RendezvousNode | null>(null);
  const [rdvUrl, setRdvUrl] = createSignal("");
  const [rdvLabel, setRdvLabel] = createSignal("");
  const [rdvMine, setRdvMine] = createSignal(true);
  const [maxTaverns, setMaxTaverns] = createSignal(16);
  // Contact popout profile card + a generic custom context menu (Discord-style;
  // the native webview menu is suppressed app-wide in onMount).
  const [profileCard, setProfileCard] = createSignal<{
    c: api.ContactDto;
    x: number;
    y: number;
  } | null>(null);
  const [menu, setMenu] = createSignal<{ x: number; y: number; items: MenuItem[] } | null>(null);
  // Full-screen Server (tavern) settings page, gated by MANAGE_SERVER.
  type SettingsSection =
    | "overview"
    | "roles"
    | "members"
    | "invites"
    | "audit"
    | "bans"
    | "automod"
    | "delete";
  const [tavernSettingsOpen, setTavernSettingsOpen] = createSignal(false);
  const [settingsSection, setSettingsSection] = createSignal<SettingsSection>("overview");
  const [tavName, setTavName] = createSignal("");
  const [tavDesc, setTavDesc] = createSignal("");
  const [tavIcon, setTavIcon] = createSignal(""); // base64 data URL or ""
  const [tavBanner, setTavBanner] = createSignal(""); // base64 data URL or ""
  const [settingsBans, setSettingsBans] = createSignal<api.BanDto[]>([]);
  const [settingsAudit, setSettingsAudit] = createSignal<api.AuditEntry[]>([]);
  // Roles editor: the role list, the currently-edited role, and its draft state.
  const [roles, setRoles] = createSignal<api.RoleDto[]>([]);
  // editingRole holds the ORIGINAL (last-saved) role; null = editor closed.
  // "" id = a brand-new unsaved role.
  const [editingRole, setEditingRole] = createSignal<api.RoleDto | null>(null);
  const [roleTab, setRoleTab] = createSignal<"display" | "permissions">("display");
  const [roleDraftName, setRoleDraftName] = createSignal("");
  const [roleDraftPerms, setRoleDraftPerms] = createSignal(0n);
  const [roleDraftColor, setRoleDraftColor] = createSignal("");
  const [roleDraftIcon, setRoleDraftIcon] = createSignal("");
  const [roleDraftHoist, setRoleDraftHoist] = createSignal(false);
  const [roleDraftMentionable, setRoleDraftMentionable] = createSignal(false);
  const [roleBusy, setRoleBusy] = createSignal(false);
  // Drag-to-reorder: id of the role being dragged.
  const [dragRoleId, setDragRoleId] = createSignal<string | null>(null);
  // The role id the dragged role would be inserted ABOVE (drop indicator). The
  // @everyone id here means "drop at the end of the normal roles".
  const [dragOverId, setDragOverId] = createSignal<string | null>(null);
  // Custom confirmation dialog (replaces the native window.confirm box).
  const [confirmDialog, setConfirmDialog] = createSignal<ConfirmOpts | null>(null);
  let confirmResolver: ((ok: boolean) => void) | null = null;
  const [connected, setConnected] = createSignal(true);
  let wasConnected = true;
  let bottomRef: HTMLDivElement | undefined;

  // --- taverns: permissions, identity, members, voice, mod alerts ---
  const [myPerms, setMyPerms] = createSignal<api.MyPerms | null>(null);
  const [tavern, setTavern] = createSignal<api.TavernDto | null>(null);
  // serverId -> icon data URL, so the rail shows every tavern's icon (not just
  // the active one). Populated in the background per connected server.
  const [tavernIcons, setTavernIcons] = createSignal<Record<string, string>>({});
  const [members, setMembers] = createSignal<api.MemberDto[]>([]);
  const [showMembers, setShowMembers] = createSignal(false);
  // Create-channel side panel: the target category id (null = closed; "" = uncategorized).
  const [createChannelCat, setCreateChannelCat] = createSignal<string | null>(null);
  const [newChannelName, setNewChannelName] = createSignal("");
  const [newChannelKind, setNewChannelKind] = createSignal<"text" | "voice">("text");
  // Channel categories + which are collapsed.
  const [categories, setCategories] = createSignal<api.CategoryDto[]>([]);
  const [collapsedCats, setCollapsedCats] = createSignal<Record<string, boolean>>({});
  // Drag-and-drop state for channels + categories.
  const [dragChannelId, setDragChannelId] = createSignal<string | null>(null);
  const [dragOverChannel, setDragOverChannel] = createSignal<string | null>(null);
  const [dragCatId, setDragCatId] = createSignal<string | null>(null);
  const [dragOverCat, setDragOverCat] = createSignal<string | null>(null);
  // group_id -> participants currently in that voice channel.
  const [voiceParticipants, setVoiceParticipants] = createSignal<
    Record<string, api.VoiceParticipant[]>
  >({});
  const [activeVoice, setActiveVoice] = createSignal<string | null>(null);
  const [localVoice, setLocalVoice] = createSignal(voice.initialVoiceState());
  // Local mic level (0..1) for the speaking indicator under the user pill.
  const [voiceLevel, setVoiceLevel] = createSignal(0);
  voice.onLevel(setVoiceLevel);
  // Remote peers' levels keyed by device id (0..1), for each participant tile.
  // Only populated for devices we're connected to (i.e. only when in the VC).
  const [peerLevels, setPeerLevels] = createSignal<Record<string, number>>({});
  voice.onLevels(setPeerLevels);
  // This device's id on the active server (to exclude our own echoed
  // participant entry, since the server fans our join back to us too).
  const [myDeviceId, setMyDeviceId] = createSignal("");
  const [deafened, setDeafened] = createSignal(false);
  const [modAlerts, setModAlerts] = createSignal<api.ModAlert[]>([]);
  // My editable profile on the active server (display name + avatar) + the
  // Profile-settings editor draft.
  const [myProfile, setMyProfile] = createSignal<api.ProfileDto | null>(null);
  const [profName, setProfName] = createSignal("");
  const [profAvatar, setProfAvatar] = createSignal("");
  const refreshProfile = () =>
    api
      .getMyProfile()
      .then((p) => {
        setMyProfile(p);
        cacheHomeAvatar(p.avatarUrl);
      })
      .catch(() => {});
  /** Cache the home account's avatar for the login picker (home only). */
  const cacheHomeAvatar = (avatarUrl: string) => {
    if (activeServerId() === "home") {
      void api.setAccountAvatar(props.home.username, avatarUrl).catch(() => {});
    }
  };
  /** A member's avatar data-URL by user id (for messages, voice tiles, etc.). */
  const avatarOf = (userId?: string) =>
    (userId ? members().find((m) => m.userId === userId)?.avatarUrl : "") ?? "";

  const can = (bit: bigint) => api.can(myPerms(), bit);

  const refreshGroups = () => api.listGroups().then(setGroups);
  const refreshPerms = () =>
    api.getMyPermissions().then(setMyPerms).catch(() => setMyPerms(null));
  const refreshTavern = () =>
    api
      .getTavern()
      .then((t) => {
        setTavern(t);
        // Cache the active server's icon so the rail keeps showing it after you
        // navigate away from the tavern.
        const id = activeServerId();
        setTavernIcons((prev) => ({ ...prev, [id]: t.iconUrl }));
      })
      .catch(() => {});
  const refreshMembers = (groupId: string | null) => {
    if (!groupId) return setMembers([]);
    api.listMembers(groupId).then(setMembers).catch(() => setMembers([]));
  };

  const textChannels = () =>
    publicGroups().filter((g) => g.channelKind !== "voice");
  const voiceChannels = () =>
    publicGroups().filter((g) => g.channelKind === "voice");
  /** Public channels in a given category (or "" for uncategorized), ordered. */
  const channelsIn = (catId: string) =>
    publicGroups()
      .filter((g) => (g.categoryId || "") === catId)
      .sort((a, b) => a.position - b.position);
  const refreshCategories = () =>
    api.listCategories().then(setCategories).catch(() => setCategories([]));
  const toggleCategory = (id: string) =>
    setCollapsedCats((prev) => ({ ...prev, [id]: !prev[id] }));

  async function loadGroupsAndSelect() {
    const [gs] = await Promise.all([api.listGroups(), refreshCategories()]);
    setGroups(gs);
    // Pick the first TEXT channel as the landing channel (voice channels are
    // join-on-click, not a default view).
    const firstText = gs.find((g) => g.kind !== "private" && g.channelKind !== "voice");
    setActiveId(firstText?.id ?? (gs.length > 0 ? gs[0].id : null));
    // Tavern context for the active server (best-effort; ignored on failure).
    void refreshPerms();
    void refreshTavern();
    void refreshProfile();
    // Load members so messages/voice tiles can resolve avatars by sender id.
    if (firstText) refreshMembers(firstText.id);
  }

  /** Open the Profile settings tab, prefilled from the active server's profile. */
  async function openProfileSettings() {
    const p = await api.getMyProfile().catch(() => null);
    if (p) {
      setMyProfile(p);
      setProfName(p.displayName);
      setProfAvatar(p.avatarUrl);
    }
  }
  /** Save the profile draft (display name + avatar) to the active server. */
  async function saveProfile(e: Event) {
    e.preventDefault();
    const name = profName().trim();
    if (!name) {
      setError("Display name can't be empty.");
      return;
    }
    try {
      const p = await api.updateProfile(name, profAvatar());
      setMyProfile(p);
      cacheHomeAvatar(p.avatarUrl); // so the login picker shows it next launch
      refreshMembers(activeId()); // reflect new name/avatar in the member list
    } catch (err) {
      setError(String(err));
    }
  }

  /** Render one chat message, Discord-style: avatar + name/time + body. The
   * avatar is the sender's profile pic if set, else their initial. */
  function renderMsg(m: UiMessage) {
    if (m.pending) {
      return (
        <div class="message pending" aria-busy="true">
          <span class="msg-avatar glint" />
          <div class="msg-main">
            <div class="glint glint-author" />
            <div class="glint glint-body" />
          </div>
        </div>
      );
    }
    // A reactive accessor (not a captured const) so the avatar updates when the
    // member list / profile changes after the row was first rendered.
    const avatar = () => avatarOf(m.senderId);
    return (
      <div class="message">
        <span class="msg-avatar">
          <Show when={avatar()} fallback={<>{(m.author[0] ?? "?").toUpperCase()}</>}>
            <img src={avatar()} alt="" />
          </Show>
        </span>
        <div class="msg-main">
          <div class="msg-head">
            <span class="author">{m.author}</span>
            <span class="time">{new Date(m.timestampMs).toLocaleTimeString()}</span>
          </div>
          <div class="body">{m.content}</div>
        </div>
      </div>
    );
  }

  // --- channel create / delete ---
  /** Open the create-channel side panel targeting a category ("" = uncategorized). */
  function openCreateChannel(categoryId: string, kind: "text" | "voice" = "text") {
    setCreateChannelCat(categoryId);
    setNewChannelKind(kind);
    setNewChannelName("");
  }
  async function submitCreateChannel(e: Event) {
    e.preventDefault();
    const name = newChannelName().trim();
    if (!name) return;
    try {
      await api.createChannel(name, newChannelKind(), createChannelCat() ?? "");
      setNewChannelName("");
      setCreateChannelCat(null);
      await refreshGroups();
    } catch (err) {
      setError(String(err));
    }
  }

  async function deleteChannel(groupId: string) {
    try {
      await api.deleteChannel(groupId);
      await refreshGroups();
    } catch (err) {
      setError(String(err));
    }
  }

  // --- categories: create / delete / drag-reorder ---
  async function createCategoryFlow() {
    closeMenu();
    const name = window.prompt("New category name");
    if (!name || !name.trim()) return;
    try {
      await api.createCategory(name.trim());
      await refreshCategories();
    } catch (err) {
      setError(String(err));
    }
  }
  async function deleteCategoryFlow(cat: api.CategoryDto) {
    const ok = await askConfirm({
      title: "Delete category",
      body: `Delete the "${cat.name}" category? Its channels become uncategorized; they are not deleted.`,
      confirmLabel: "Delete category",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteCategory(cat.id);
      await Promise.all([refreshCategories(), refreshGroups()]);
    } catch (err) {
      setError(String(err));
    }
  }
  /** Drop the dragged channel into `catId` before `beforeId` (null = append). */
  async function dropChannel(catId: string, beforeId: string | null) {
    const dragged = dragChannelId();
    setDragChannelId(null);
    setDragOverChannel(null);
    if (!dragged) return;
    const ids = channelsIn(catId)
      .map((g) => g.id)
      .filter((id) => id !== dragged);
    const at = beforeId ? ids.indexOf(beforeId) : ids.length;
    ids.splice(at < 0 ? ids.length : at, 0, dragged);
    // Optimistic: update local positions/category before the round-trip.
    setGroups((prev) =>
      prev.map((g) =>
        g.id === dragged
          ? { ...g, categoryId: catId, position: ids.indexOf(dragged) }
          : ids.includes(g.id)
            ? { ...g, position: ids.indexOf(g.id) }
            : g
      )
    );
    try {
      await api.reorderChannels(catId, ids);
      await refreshGroups();
    } catch (e) {
      setError(String(e));
      await refreshGroups();
    }
  }
  /** Render one channel row (text or voice) with drag-reorder + delete. `catId`
   * is the category the row lives in (drop target for in-category reorder). */
  function channelRow(g: api.GroupDto, catId: string) {
    const isVoice = () => g.channelKind === "voice";
    const isActive = () => (isVoice() ? activeVoice() === g.id : g.id === activeId());
    return (
      <>
        <div
          class={`channel-row ${isActive() ? "active" : ""} ${dragChannelId() === g.id ? "dragging" : ""} ${dragChannelId() && dragChannelId() !== g.id && dragOverChannel() === g.id ? "drop-above" : ""}`}
          draggable={can(api.PERM.MANAGE_CHANNELS)}
          onDragStart={(e) => {
            setDragChannelId(g.id);
            if (e.dataTransfer) {
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", g.id);
            }
          }}
          onDragEnd={() => {
            setDragChannelId(null);
            setDragOverChannel(null);
          }}
          onDragOver={(e) => {
            if (!dragChannelId()) return;
            e.preventDefault();
            e.stopPropagation();
            if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
            setDragOverChannel(g.id);
          }}
          onDrop={(e) => {
            if (!dragChannelId()) return;
            e.preventDefault();
            e.stopPropagation();
            dropChannel(catId, g.id);
          }}
        >
          <button
            class={`channel ${isVoice() ? "voice-channel" : ""}`}
            onClick={() => (isVoice() ? joinVoiceChannel(g.id) : setActiveId(g.id))}
          >
            <span class="hash">
              <Fa icon={isVoice() ? faVolumeHigh : faHashtag} />
            </span>
            {g.name}
          </button>
          <Show when={can(api.PERM.MANAGE_CHANNELS)}>
            <button class="channel-del" title="Delete channel" onClick={() => deleteChannel(g.id)}>
              <Fa icon={faTrash} />
            </button>
          </Show>
        </div>
        <Show when={isVoice()}>
          {/* Your own tile (the server only sends OTHER participants), driven by
              your local mic level so the indicator works even alone in the call. */}
          <Show when={activeVoice() === g.id}>
            <div class="voice-participant">
              <span class="vp-avatar">
                <Show
                  when={myProfile()?.avatarUrl}
                  fallback={(props.home.username[0] ?? "?").toUpperCase()}
                >
                  <img src={myProfile()!.avatarUrl} alt="" />
                </Show>
              </span>
              <span class="vp-name">{myProfile()?.displayName || props.home.username}</span>
              <span class="vp-icons">
                <Show when={localVoice().muted}>
                  <Fa icon={faMicrophoneSlash} />
                </Show>
                <Show when={localVoice().cameraOn}>
                  <Fa icon={faVideo} />
                </Show>
                <Show when={localVoice().screenOn}>
                  <Fa icon={faDesktop} />
                </Show>
              </span>
              <div class="vp-voice-bar">
                <div
                  class="vp-voice-level"
                  style={{ width: `${Math.round(voiceLevel() * 100)}%` }}
                />
              </div>
            </div>
          </Show>
          <For each={participantsFor(g.id)}>
            {(p) => {
              const label = () => p.displayName || p.username || p.userId.slice(0, 8);
              // Only present when WE are in the VC (we have this peer's audio).
              const level = () => peerLevels()[p.deviceId] ?? 0;
              const avatar = () => avatarOf(p.userId);
              return (
                <div class="voice-participant">
                  <span class="vp-avatar">
                    <Show when={avatar()} fallback={(label()[0] ?? "?").toUpperCase()}>
                      <img src={avatar()} alt="" />
                    </Show>
                  </span>
                  <span class="vp-name">{label()}</span>
                  <span class="vp-icons">
                    <Show when={p.muted}>
                      <Fa icon={faMicrophoneSlash} />
                    </Show>
                    <Show when={p.cameraOn}>
                      <Fa icon={faVideo} />
                    </Show>
                    <Show when={p.screenOn}>
                      <Fa icon={faDesktop} />
                    </Show>
                  </span>
                  <Show when={activeVoice() === g.id}>
                    <div class="vp-voice-bar">
                      <div
                        class="vp-voice-level"
                        style={{ width: `${Math.round(level() * 100)}%` }}
                      />
                    </div>
                  </Show>
                </div>
              );
            }}
          </For>
        </Show>
      </>
    );
  }

  /** Drop the dragged category before `beforeId` (null = end). */
  async function dropCategory(beforeId: string | null) {
    const dragged = dragCatId();
    setDragCatId(null);
    setDragOverCat(null);
    if (!dragged || dragged === beforeId) return;
    const ids = categories()
      .map((c) => c.id)
      .filter((id) => id !== dragged);
    const at = beforeId ? ids.indexOf(beforeId) : ids.length;
    ids.splice(at < 0 ? ids.length : at, 0, dragged);
    setCategories((prev) =>
      [...prev].sort((a, b) => ids.indexOf(a.id) - ids.indexOf(b.id))
    );
    try {
      await api.reorderCategories(ids);
      await refreshCategories();
    } catch (e) {
      setError(String(e));
      await refreshCategories();
    }
  }

  // --- voice channel join/leave + toggles (media stubbed in voice.ts) ---
  async function joinVoiceChannel(groupId: string) {
    try {
      if (activeVoice() && activeVoice() !== groupId) {
        await voice.leave(activeVoice()!);
      }
      const deviceId = await api.getMyDeviceId().catch(() => "");
      setMyDeviceId(deviceId);
      await voice.join(groupId, deviceId);
      setActiveVoice(groupId);
      setLocalVoice({ ...voice.initialVoiceState(), groupId });
    } catch (err) {
      setError(String(err));
    }
  }

  async function leaveVoiceChannel() {
    const g = activeVoice();
    if (!g) return;
    try {
      await voice.leave(g);
    } catch (err) {
      setError(String(err));
    }
    setActiveVoice(null);
    setLocalVoice(voice.initialVoiceState());
    setDeafened(false);
  }

  const toggleMute = async () => {
    const s = localVoice();
    // Unmuting while deafened also un-deafens (matches Discord).
    if (s.muted && deafened()) {
      voice.setDeafened(false);
      setDeafened(false);
    }
    await voice.setMuted(s, !s.muted);
    setLocalVoice({ ...s, muted: !s.muted });
  };
  /** Deafen = silence everyone else's audio; per convention it also mutes you. */
  const toggleDeafen = async () => {
    const next = !deafened();
    setDeafened(next);
    voice.setDeafened(next);
    const s = localVoice();
    if (next && !s.muted) {
      await voice.setMuted(s, true);
      setLocalVoice({ ...s, muted: true });
    } else if (!next && s.muted) {
      await voice.setMuted(s, false);
      setLocalVoice({ ...s, muted: false });
    }
  };
  const toggleCamera = async () => {
    const s = localVoice();
    await voice.setCamera(s, !s.cameraOn);
    setLocalVoice({ ...s, cameraOn: !s.cameraOn });
  };
  const toggleScreen = async () => {
    const s = localVoice();
    await voice.setScreen(s, !s.screenOn);
    setLocalVoice({ ...s, screenOn: !s.screenOn });
  };

  const participantsFor = (groupId: string) =>
    (voiceParticipants()[groupId] ?? []).filter((p) => p.deviceId !== myDeviceId());
  /** Label for the voice panel: the voice channel's name, or the DM peer. */
  const voiceLabel = () => {
    const id = activeVoice();
    if (!id) return "";
    const g = groups().find((x) => x.id === id);
    if (g) return g.name;
    if (activeConv()?.groupId === id) return activeConv()!.peerName;
    return "Voice";
  };

  function appendIfActive(msg: UiMessage) {
    if (msg.groupId !== activeId()) return;
    setMessages((prev) => (prev.some((m) => m.id === msg.id) ? prev : [...prev, msg]));
  }

  const toggleEncryptAtRest = async () => {
    const next = !encryptAtRest();
    try {
      await api.setEncryptAtRest(next);
      setEncryptAtRest(next);
    } catch (e) {
      setError(String(e));
    }
  };

  const changeFriendPolicy = async (policy: api.FriendRequestPolicy) => {
    try {
      await api.setFriendRequestPolicy(policy);
      setFriendPolicy(policy);
    } catch (e) {
      setError(String(e));
    }
  };

  const connectMesh = async () => {
    setMeshConn({ state: "connecting", message: "Starting..." });
    try {
      setMesh(await api.meshConnect(yggMode(), yggPeersText().split("\n")));
    } catch {
      // The mesh-connect-status event already carries the error line.
      api.getMeshStatus().then(setMesh).catch(() => {});
    }
  };

  const disconnectMesh = async () => {
    try {
      setMesh(await api.meshDisconnect());
    } catch (e) {
      setMeshConn({ state: "error", message: String(e) });
    }
  };

  const saveRendezvous = async () => {
    const url = rdvUrl().trim();
    if (!url) return;
    const node = { url, label: rdvLabel().trim() || url, mine: rdvMine() };
    try {
      await api.setRendezvousNode(node);
      setRendezvous(node);
      setRdvUrl("");
      setRdvLabel("");
    } catch (e) {
      setError(String(e));
    }
  };

  const clearRendezvous = async () => {
    try {
      await api.setRendezvousNode(null);
      setRendezvous(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const unlisteners: Array<() => void> = [];
  onCleanup(() => unlisteners.forEach((u) => u()));

  const refreshContacts = () => api.listContacts().then(setContacts).catch(() => {});
  const refreshBlocks = () => api.listBlocks().then(setBlocks).catch(() => {});
  const refreshDms = () => api.listDms().then(setDmConversations).catch(() => {});

  /** Display name for a private message: "You" for own messages, the DM peer's
   * name when the group is a known conversation, else a short id fallback. */
  const authorFor = (m: api.PrivateMessageDto): string => {
    if (m.mine) return "You";
    const conv = dmConversations().find((c) => c.groupId === m.groupId);
    return conv?.peerName ?? shortId(m.senderId);
  };

  async function toggleBlocked(c: api.ContactDto) {
    try {
      if (isBlocked(c.id)) await api.unblockContact(c.id);
      else await api.blockContact(c.id, c.name);
      await refreshBlocks();
    } catch (e) {
      setError(String(e));
    }
  }

  onMount(async () => {
    // Suppress the native webview right-click menu app-wide so our custom menus
    // are the only context menus. (Ctrl+V etc. still work in inputs.)
    const blockNativeMenu = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", blockNativeMenu);
    onCleanup(() => document.removeEventListener("contextmenu", blockNativeMenu));

    // Esc closes any open custom menu / the full-screen tavern settings page.
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      closeMenu();
      if (confirmDialog()) return resolveConfirm(false);
      if (tavernSettingsOpen() && guardUnsaved()) setTavernSettingsOpen(false);
    };
    document.addEventListener("keydown", onEsc);
    onCleanup(() => document.removeEventListener("keydown", onEsc));

    api
      .getSettings()
      .then((s) => {
        setEncryptAtRest(s.encryptAtRest);
        setFriendPolicy(s.friendRequestPolicy);
        setRendezvous(s.rendezvousNode);
        setYggMode(s.yggPeerMode);
        setYggPeersText(s.yggPrivatePeers.join("\n"));
        setMaxTaverns(s.maxHostedTaverns);
      })
      .catch(() => {});
    // DMs is the landing view; the home server is the hidden backbone, so we do
    // not load/show its channels until the user opens an actual server.
    refreshContacts();
    refreshBlocks();
    refreshDms();
    // Load the home profile so the pill shows your avatar/name and the login
    // picker's cached avatar stays fresh (home is the active session at launch).
    void refreshProfile();
    // Re-spawn any taverns this client hosts and attach them to the rail.
    attachResumedTaverns();
    api
      .getMeshStatus()
      .then((m) => {
        setMesh(m);
        // Seed the status line from the live state.
        if (m.running && m.address) {
          setMeshConn({ state: "connected", message: `Connected - mesh address ${m.address}` });
        } else if (m.enabled && !m.running) {
          setMeshConn({
            state: "error",
            message: "Mesh is enabled but not running (admin privileges needed?)",
          });
        }
      })
      .catch(() => {});
    unlisteners.push(
      await api.onMeshConnectStatus((s) => {
        setMeshConn(s.state === "idle" ? null : s);
        if (s.state === "connected" || s.state === "idle") {
          api.getMeshStatus().then(setMesh).catch(() => {});
          // The fr code embeds the current addresses (incl. the mesh address),
          // so regenerate it whenever connectivity changes.
          api.myContactCode(props.home.username).then(setMyCode).catch(() => {});
        }
      })
    );
    api.myContactCode(props.home.username).then(setMyCode).catch(() => {});

    unlisteners.push(
      await api.onIncomingMessage((m) => {
        if (m.serverId !== activeServerId()) {
          bumpUnread(m.serverId); // background server; archived + badged, shown on switch
          return;
        }
        appendIfActive({
          id: m.id,
          groupId: m.groupId,
          author: m.senderDisplayName,
          senderId: m.senderId,
          content: m.content,
          timestampMs: m.timestampMs,
        });
      })
    );
    unlisteners.push(
      await api.onIncomingPrivateMessage((m) => {
        if (m.serverId !== activeServerId()) {
          bumpUnread(m.serverId);
          return;
        }
        appendIfActive({
          id: crypto.randomUUID(),
          groupId: m.groupId,
          author: authorFor(m),
          content: m.content,
          timestampMs: m.timestampMs,
        });
      })
    );
    unlisteners.push(
      await api.onPrivateHistoryDecrypted(({ id, message }) => {
        if (message.serverId !== activeServerId()) return;
        setMessages((prev) =>
          prev.map((m) =>
            m.id === id
              ? {
                  ...m,
                  author: authorFor(message),
                  content: message.content,
                  timestampMs: message.timestampMs,
                  pending: false,
                }
              : m
          )
        );
      })
    );
    unlisteners.push(
      await api.onJoinedGroup(({ serverId }) => {
        if (serverId === activeServerId()) refreshGroups();
        // A new private group from a Welcome may be an inbound DM - surface it.
        refreshDms();
      })
    );
    unlisteners.push(
      await api.onVoiceParticipant((p) => {
        if (p.serverId !== activeServerId()) return;
        setVoiceParticipants((prev) => {
          const list = (prev[p.groupId] ?? []).filter((x) => x.deviceId !== p.deviceId);
          if (p.joined) list.push(p);
          return { ...prev, [p.groupId]: list };
        });
        // Drive the WebRTC mesh: connect to joiners, drop leavers.
        voice.onParticipant(p);
      })
    );
    unlisteners.push(
      // Relayed WebRTC signaling -> the (stubbed) media layer.
      await api.onVoiceSignal((s) => {
        if (s.serverId !== activeServerId()) return;
        voice.handleSignal(s);
      })
    );
    unlisteners.push(
      // Guardrail/auto-mod alerts (only owner/admins receive these).
      await api.onModAlert((a) => {
        if (a.serverId !== activeServerId()) return;
        setModAlerts((prev) => [...prev.slice(-4), a]);
      })
    );
    unlisteners.push(
      // A friend request was accepted (either direction): refresh everything
      // friend-shaped.
      await api.onFriendsChanged(() => {
        refreshContacts();
        refreshDms();
        syncFr();
      })
    );
    // Keep the requests/friends views fresh (and retry queued deliveries +
    // profile backfills) while either is open - pending placeholders show in
    // the Friends list too.
    const frTimer = setInterval(() => {
      if (dmSel() === "requests" || dmSel() === "friends") syncFr();
    }, 45_000);
    unlisteners.push(() => clearInterval(frTimer));
    unlisteners.push(
      // Persisted DMs reconnect in the background after login; refresh the list
      // as each one comes up.
      await api.onDmsChanged(() => refreshDms())
    );
    unlisteners.push(
      await api.onConnection(({ serverId, connected }) => {
        // Cache any connected (non-home) server's icon for the rail, even if it
        // isn't the active one.
        if (connected && serverId !== "home") {
          api
            .getTavernFor(serverId)
            .then((t) => setTavernIcons((prev) => ({ ...prev, [serverId]: t.iconUrl })))
            .catch(() => {});
        }
        if (serverId !== activeServerId()) return;
        if (connected && !wasConnected) loadHistory();
        wasConnected = connected;
        setConnected(connected);
      })
    );
  });

  // Load the active channel's history (untracked reads so it can run on demand).
  async function loadHistory() {
    const id = untrack(activeId);
    if (!id) return;
    const group = untrack(groups).find((g) => g.id === id);
    // A group not in the active server's channel list is a DM group (on a
    // contact's host session), which is always private.
    if (group ? group.kind === "private" : true) {
      const history = await api.fetchPrivateHistory(id);
      setMessages(
        history.map((e) =>
          e.message
            ? {
                id: e.id,
                groupId: e.message.groupId,
                author: authorFor(e.message),
                content: e.message.content,
                timestampMs: e.message.timestampMs,
                pending: false,
              }
            : { id: e.id, groupId: id, author: "", content: "", timestampMs: 0, pending: true }
        )
      );
      return;
    }
    const history = await api.fetchPublicHistory(id);
    setMessages(
      [...history].reverse().map((m) => ({
        id: m.id,
        groupId: m.groupId,
        author: m.senderDisplayName,
        senderId: m.senderId,
        content: m.content,
        timestampMs: m.timestampMs,
      }))
    );
  }

  createEffect(() => {
    activeId();
    loadHistory();
  });

  // Backstop: when the rail's server list changes, (re)fetch icons so every
  // tavern glyph shows its icon. The connection-status hook covers the common
  // case; this catches launch races. Depends only on the server list (not the
  // icon cache) to avoid a write-read reactive loop.
  createEffect(() => {
    railServers();
    refreshRailIcons();
  });

  createEffect(() => {
    messages();
    bottomRef?.scrollIntoView({ behavior: "smooth" });
  });

  const activeGroup = () => groups().find((g) => g.id === activeId());
  const isPrivate = () => activeGroup()?.kind === "private";
  const publicGroups = () => groups().filter((g) => g.kind !== "private");

  // Refresh the member list when the member panel is open and the active public
  // channel changes (members are server-scoped; any public channel works).
  createEffect(() => {
    const id = activeId();
    if (showMembers() && view() === "server" && id && !isPrivate()) {
      refreshMembers(id);
      refreshRoles(); // needed to group hoisted roles + color member names
    }
  });

  // Header prompt: Yggdrasil underpins almost everything (taverns, internet DMs),
  // so nudge the user to set it up until it's connected, then clear it.
  createEffect(() => {
    const m = mesh();
    if (!m) return; // status not loaded yet
    if (m.running) {
      dismissKey("ygg-setup");
      return;
    }
    // Yellow (warn): without Yggdrasil connected you effectively can't use the
    // app's cross-device features, so it's an action-needed prompt with a link
    // straight to the network settings.
    notify({
      key: "ygg-setup",
      severity: "warn",
      message: m.available
        ? "Yggdrasil networking isn't connected - most features need it."
        : "Yggdrasil networking isn't available in this build - most cross-device features need it.",
      actionLabel: "Click here to access settings",
      onAction: () => {
        setSettingsTab("network");
        setSettingsOpen(true);
      },
    });
  });

  /** Open (or re-select) a DM with a contact: ensure the cross-server DM exists,
   * make its host session active, and show the conversation. */
  /** Activate a DM conversation: make its host session active and show it. */
  async function openConversation(conv: api.DmConversation) {
    setDmSel(conv.peerId);
    setActiveConv(conv);
    setError(null);
    try {
      await api.setActiveServer(conv.serverId);
      setActiveServerId(conv.serverId);
      setMessages([]);
      setActiveId(conv.groupId); // triggers loadHistory (treated as private)
    } catch (e) {
      setError(String(e));
    }
  }

  /** Start (or re-open) a DM with a contact, then show the conversation. */
  async function openDm(c: api.ContactDto) {
    setDmSel(c.id);
    setError(null);
    let route = dmRoutes()[c.id];
    if (!route) {
      setDmOpening(true);
      try {
        const opened = await api.openContactDm(c.id, props.home.username);
        setDmRoutes((prev) => ({ ...prev, [c.id]: opened }));
        route = opened;
      } catch (e) {
        setError(String(e));
        setDmOpening(false);
        return;
      }
      setDmOpening(false);
    }
    const r = route;
    if (!r) return;
    await refreshDms();
    await openConversation({
      serverId: r.serverId,
      groupId: r.group.id,
      peerId: c.id,
      peerName: c.name,
      fingerprint: c.fingerprint,
    });
  }

  // --- custom context menu + contact/member actions ---
  /** Left-click a contact: open their profile card near the cursor. */
  function openProfileCard(c: api.ContactDto, e: MouseEvent) {
    e.preventDefault();
    setMenu(null);
    setProfileCard({ c, x: e.clientX, y: e.clientY });
  }
  /** Open the custom context menu at the cursor with the given items. */
  function showMenu(e: MouseEvent, items: MenuItem[]) {
    e.preventDefault();
    setProfileCard(null);
    setMenu({ x: e.clientX, y: e.clientY, items });
  }
  const closeMenu = () => setMenu(null);

  /** Message a contact: dismiss overlays and open the DM conversation. */
  async function messageContact(c: api.ContactDto) {
    setProfileCard(null);
    closeMenu();
    await openDm(c);
  }
  /** Call a contact: open the DM, then start a voice call on its group. */
  async function callContact(c: api.ContactDto) {
    setProfileCard(null);
    closeMenu();
    await openDm(c);
    const conv = activeConv();
    if (conv) await joinVoiceChannel(conv.groupId);
  }
  async function removeFriend(c: api.ContactDto) {
    closeMenu();
    try {
      await api.removeContact(c.id);
      await refreshContacts();
    } catch (e) {
      setError(String(e));
    }
  }
  const copyText = (t: string) => navigator.clipboard.writeText(t).catch(() => {});

  /** Right-click menu for a contact (DM/friends list). */
  function contactMenuItems(c: api.ContactDto): MenuItem[] {
    return [
      { label: "Profile", icon: faUserGroup, onClick: () => setProfileCard({ c, x: menu()!.x, y: menu()!.y }) },
      { label: "Message", icon: faComments, onClick: () => void messageContact(c) },
      { label: "Start a Call", icon: faPhone, onClick: () => void callContact(c) },
      { label: "Copy User ID", icon: faPlus, sep: true, onClick: () => copyText(c.id) },
      { label: "Remove Friend", danger: true, onClick: () => void removeFriend(c) },
      {
        label: isBlocked(c.id) ? "Unblock" : "Block",
        danger: !isBlocked(c.id),
        onClick: () => {
          closeMenu();
          void toggleBlocked(c);
        },
      },
    ];
  }

  /** Right-click menu for a tavern member, gated by the viewer's permissions.
   * (Profile/Message/Call to arbitrary members need the contact-identity link;
   * that arrives with federation - for now this covers Mention + moderation.) */
  function memberMenuItems(m: api.MemberDto): MenuItem[] {
    const items: MenuItem[] = [
      {
        label: "Mention",
        onClick: () => {
          closeMenu();
          setDraft(`${draft()}@${m.displayName} `);
        },
      },
      { label: "Copy User ID", icon: faPlus, onClick: () => copyText(m.userId) },
    ];
    // Can't moderate the server owner (can() already grants owner/admin).
    if (!m.isOwner) {
      if (activeId() && can(api.PERM.KICK_MEMBERS)) {
        items.push({
          label: "Kick",
          danger: true,
          sep: true,
          onClick: () => {
            closeMenu();
            api
              .kickMember(activeId()!, m.userId)
              .then(() => refreshMembers(activeId()))
              .catch((e) => setError(String(e)));
          },
        });
      }
      if (can(api.PERM.BAN_MEMBERS)) {
        items.push({
          label: "Ban",
          danger: true,
          sep: !can(api.PERM.KICK_MEMBERS),
          onClick: () => {
            closeMenu();
            api
              .banMember(m.userId)
              .then(() => refreshMembers(activeId()))
              .catch((e) => setError(String(e)));
          },
        });
      }
    }
    return items;
  }

  /** Open the full-screen tavern settings page, prefilled with current values. */
  function openTavernSettings() {
    const t = tavern();
    setTavName(t?.name ?? "");
    setTavDesc(t?.description ?? "");
    setTavIcon(t?.iconUrl ?? "");
    setTavBanner(t?.bannerUrl ?? "");
    setSettingsSection("overview");
    setTavernSettingsOpen(true);
  }
  /** Switch settings section, lazily loading that section's data. Blocked while
   * a role has unsaved changes (see guardUnsaved). */
  function selectSettingsSection(s: SettingsSection) {
    if (!guardUnsaved()) return;
    setSettingsSection(s);
    if (s === "members") {
      refreshMembers(activeId());
      refreshRoles();
    }
    if (s === "bans") api.listBans().then(setSettingsBans).catch(() => setSettingsBans([]));
    if (s === "audit") api.listAudit(200).then(setSettingsAudit).catch(() => setSettingsAudit([]));
    if (s === "roles") refreshRoles();
  }
  async function unbanUser(userId: string) {
    try {
      await api.unbanMember(userId);
      setSettingsBans(await api.listBans());
    } catch (e) {
      setError(String(e));
    }
  }

  // --- Roles editor -----------------------------------------------------------
  function refreshRoles() {
    api.listRoles().then(setRoles).catch(() => setRoles([]));
  }
  /** Seed the draft fields from a role (original state). */
  function seedRoleDraft(r: api.RoleDto) {
    setRoleDraftName(r.name);
    setRoleDraftPerms(BigInt(r.permissions || "0"));
    setRoleDraftColor(r.color);
    setRoleDraftIcon(r.icon);
    setRoleDraftHoist(r.hoist);
    setRoleDraftMentionable(r.mentionable);
  }
  /** True while the open editor's draft differs from the saved original. A new
   * (unsaved) role is always considered dirty until it is created. */
  function roleDirty(): boolean {
    const r = editingRole();
    if (!r) return false;
    if (!r.id) return true; // brand-new, never saved
    return (
      roleDraftName() !== r.name ||
      roleDraftPerms().toString() !== r.permissions ||
      roleDraftColor() !== r.color ||
      roleDraftIcon() !== r.icon ||
      roleDraftHoist() !== r.hoist ||
      roleDraftMentionable() !== r.mentionable
    );
  }
  /** Block an action if there are unsaved role changes; toast if so. */
  function guardUnsaved(): boolean {
    if (roleDirty()) {
      notifyTransient({
        key: "role-unsaved",
        severity: "warn",
        message: "You have unsaved role changes - save or reset them first.",
      });
      return false;
    }
    return true;
  }
  /** Open the editor for a role (guarded), seeding the draft from its state. */
  function editRole(r: api.RoleDto) {
    if (editingRole()?.id === r.id && !!r.id) return; // already editing it
    if (!guardUnsaved()) return;
    setRoleTab("display");
    setEditingRole(r);
    seedRoleDraft(r);
  }
  /** Begin creating a brand-new role (draft only; saved on "Create"). */
  function startNewRole() {
    if (!guardUnsaved()) return;
    const blank: api.RoleDto = {
      id: "",
      name: "new role",
      permissions: "0",
      position: 0,
      isDefault: false,
      color: "",
      icon: "",
      hoist: false,
      mentionable: false,
    };
    setRoleTab("display");
    setEditingRole(blank);
    seedRoleDraft(blank);
  }
  /** Reset the draft to the saved original; for a new role, discard + close. */
  function resetRoleDraft() {
    const r = editingRole();
    if (!r) return;
    if (!r.id) {
      setEditingRole(null);
      return;
    }
    seedRoleDraft(r);
  }
  /** Toggle one permission bit in the draft. */
  function toggleRolePerm(bit: bigint, on: boolean) {
    setRoleDraftPerms((p) => (on ? p | bit : p & ~bit));
  }
  /** Encode a canvas to the most compact alpha-preserving format the webview can
   * produce: AVIF first (best compression), then WebP, then PNG. `toDataURL`
   * silently returns PNG when a codec is unsupported, so we verify the MIME of
   * the result and fall through. All three keep the alpha channel. */
  function encodeImage(canvas: HTMLCanvasElement): string {
    for (const type of ["image/avif", "image/webp"]) {
      const url = canvas.toDataURL(type, 0.9);
      if (url.startsWith(`data:${type}`)) return url;
    }
    return canvas.toDataURL("image/png");
  }
  /** Read a picked image, downscale it to fit `w`x`h` (cover-cropped, centered),
   * and hand back a base64 data URL. Preserves transparency. Rejects non-images
   * and oversized results. */
  function downscaleImage(
    file: File | undefined,
    w: number,
    h: number,
    onDone: (dataUrl: string) => void
  ) {
    if (!file) return;
    if (!file.type.startsWith("image/")) {
      setError("That file is not an image.");
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const img = new Image();
      img.onload = () => {
        const canvas = document.createElement("canvas");
        canvas.width = w;
        canvas.height = h;
        // Transparent backing (no fill) so source alpha is preserved.
        const ctx = canvas.getContext("2d");
        if (!ctx) return;
        ctx.clearRect(0, 0, w, h);
        // Cover-fit: scale so the image fills the box, center-cropped.
        const scale = Math.max(w / img.width, h / img.height);
        const dw = img.width * scale;
        const dh = img.height * scale;
        ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
        const dataUrl = encodeImage(canvas);
        if (dataUrl.length > 512 * 1024) {
          setError("Image is too large even after resizing.");
          return;
        }
        onDone(dataUrl);
      };
      img.src = String(reader.result);
    };
    reader.readAsDataURL(file);
  }
  /** Role icon: a small 64x64 square. */
  function pickRoleIcon(file: File | undefined) {
    downscaleImage(file, 64, 64, setRoleDraftIcon);
  }
  /** Persist the draft, then close the editor (per the "save = done" UX). */
  async function saveRole() {
    const r = editingRole();
    if (!r) return;
    const name = roleDraftName().trim();
    if (!name) {
      setError("Role name can't be empty.");
      return;
    }
    setRoleBusy(true);
    try {
      const write: api.RoleWrite = {
        name,
        permissions: roleDraftPerms().toString(),
        color: roleDraftColor(),
        icon: roleDraftIcon(),
        hoist: roleDraftHoist(),
        mentionable: roleDraftMentionable(),
      };
      if (r.id) await api.updateRole(r.id, write);
      else await api.createRole(write);
      setEditingRole(null); // close the control box as completion feedback
      refreshRoles();
      refreshPerms();
    } catch (e) {
      setError(String(e));
    } finally {
      setRoleBusy(false);
    }
  }
  /** The list shown in the Roles editor: highest power first, @everyone always
   * pinned to the very bottom. Reflects the live draft name for the open role. */
  function roleListOrdered(): api.RoleDto[] {
    const live = (r: api.RoleDto) =>
      editingRole()?.id === r.id && r.id ? { ...r, name: roleDraftName() } : r;
    const rs = roles().map(live);
    const normal = rs.filter((r) => !r.isDefault);
    const everyone = rs.filter((r) => r.isDefault);
    return [...normal, ...everyone];
  }
  /** Look up a role's name by id (for member role chips). */
  function roleName(id: string): string {
    return roles().find((r) => r.id === id)?.name ?? "role";
  }
  /** A member's highest (most powerful) assigned non-default role, or null. */
  function memberTopRole(m: api.MemberDto): api.RoleDto | null {
    let best: api.RoleDto | null = null;
    for (const rid of m.roleIds) {
      const role = roles().find((r) => r.id === rid && !r.isDefault);
      if (role && (!best || role.position > best.position)) best = role;
    }
    return best;
  }
  /** Group members into hoisted-role sections (highest power first), with a
   * catch-all "Members" section for everyone whose roles aren't hoisted. */
  function memberSections(): { id: string; title: string; color: string; members: api.MemberDto[] }[] {
    const hoisted = roles().filter((r) => r.hoist && !r.isDefault); // already pos desc
    const sections = hoisted.map((r) => ({
      id: r.id,
      title: r.name,
      color: r.color,
      members: [] as api.MemberDto[],
    }));
    const rest: api.MemberDto[] = [];
    for (const m of members()) {
      let best: api.RoleDto | null = null;
      for (const rid of m.roleIds) {
        const role = hoisted.find((r) => r.id === rid);
        if (role && (!best || role.position > best.position)) best = role;
      }
      if (best) sections.find((s) => s.id === best!.id)!.members.push(m);
      else rest.push(m);
    }
    const out = sections.filter((s) => s.members.length);
    if (rest.length) out.push({ id: "__rest", title: "Members", color: "", members: rest });
    return out;
  }
  /** Assign a role to a member, then refresh the member list. */
  async function assignRoleToMember(userId: string, roleId: string) {
    if (!roleId) return;
    try {
      await api.assignRole(userId, roleId);
      refreshMembers(activeId());
    } catch (e) {
      setError(String(e));
    }
  }
  /** Remove a role from a member, then refresh the member list. */
  async function unassignRoleFromMember(userId: string, roleId: string) {
    try {
      await api.unassignRole(userId, roleId);
      refreshMembers(activeId());
    } catch (e) {
      setError(String(e));
    }
  }
  /** Drop the dragged role before `targetId`, then persist the new order. A
   * targetId that isn't a normal role (e.g. the @everyone id) lands at the end. */
  async function dropRoleBefore(targetId: string | null) {
    const dragged = dragRoleId();
    setDragRoleId(null);
    setDragOverId(null);
    if (!dragged || dragged === targetId) return;
    const normal = roles().filter((r) => !r.isDefault);
    const ids = normal.map((r) => r.id).filter((id) => id !== dragged);
    const at = targetId ? ids.indexOf(targetId) : ids.length;
    ids.splice(at < 0 ? ids.length : at, 0, dragged);
    // Optimistic local reorder so the list updates immediately.
    const byId = new Map(roles().map((r) => [r.id, r]));
    const reordered = [
      ...ids.map((id) => byId.get(id)!).filter(Boolean),
      ...roles().filter((r) => r.isDefault),
    ];
    setRoles(reordered);
    try {
      await api.reorderRoles(ids);
      refreshRoles();
      refreshPerms();
    } catch (e) {
      setError(String(e));
      refreshRoles();
    }
  }

  /** Delete the edited role after confirming. */
  async function deleteRoleFlow(r: api.RoleDto) {
    const ok = await askConfirm({
      title: "Delete role",
      body: `Delete the "${r.name}" role? Members keep their other roles; this can't be undone.`,
      confirmLabel: "Delete role",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteRole(r.id);
      if (editingRole()?.id === r.id) setEditingRole(null);
      refreshRoles();
    } catch (e) {
      setError(String(e));
    }
  }
  async function saveTavernSettings(e: Event) {
    e.preventDefault();
    try {
      const t = await api.updateTavern(
        tavName().trim(),
        tavIcon(),
        tavDesc().trim(),
        tavBanner()
      );
      setTavern(t);
      setTavernIcons((prev) => ({ ...prev, [activeServerId()]: t.iconUrl }));
    } catch (err) {
      setError(String(err));
    }
  }

  /** Show the custom confirm dialog; resolves true if the user confirms. */
  function askConfirm(opts: ConfirmOpts): Promise<boolean> {
    return new Promise((resolve) => {
      confirmResolver = resolve;
      setConfirmDialog(opts);
    });
  }
  /** Resolve and dismiss the active confirm dialog. */
  function resolveConfirm(ok: boolean) {
    setConfirmDialog(null);
    const r = confirmResolver;
    confirmResolver = null;
    r?.(ok);
  }

  /** Delete the active tavern (host-only; stops it + wipes its data). */
  async function deleteTavernFlow() {
    closeMenu();
    const id = activeServerId();
    if (id === "home") return;
    const ok = await askConfirm({
      title: "Delete tavern",
      body: "Delete this tavern? This stops it and permanently erases its data. This can't be undone.",
      confirmLabel: "Delete tavern",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteTavern(id);
      setServers((prev) => prev.filter((s) => s.id !== id));
      await enterDms();
    } catch (err) {
      setError(String(err));
    }
  }

  /** The tavern (server) management menu - opened by clicking the tavern name or
   * right-clicking the channel list. Items are gated by the viewer's perms. */
  function tavernMenuItems(): MenuItem[] {
    const items: MenuItem[] = [];
    if (can(api.PERM.MANAGE_CHANNELS)) {
      items.push({
        label: "Create Channel",
        icon: faPlus,
        onClick: () => {
          closeMenu();
          // Context-menu channels land in the first category (or uncategorized).
          openCreateChannel(categories()[0]?.id ?? "");
        },
      });
      items.push({
        label: "Create Category",
        icon: faPlus,
        onClick: createCategoryFlow,
      });
    }
    items.push({
      label: "Invite People",
      icon: faUserPlus,
      sep: items.length > 0,
      onClick: () => {
        closeMenu();
        showInvite();
      },
    });
    if (can(api.PERM.MANAGE_SERVER)) {
      items.push({
        label: "Server Settings",
        icon: faGear,
        sep: true,
        onClick: () => {
          closeMenu();
          openTavernSettings();
        },
      });
    }
    if (myPerms()?.isOwner) {
      items.push({ label: "Delete Tavern", danger: true, sep: true, onClick: deleteTavernFlow });
    }
    return items;
  }

  /** Right-click menu for a category header (gated by MANAGE_CHANNELS). */
  function categoryMenuItems(cat: api.CategoryDto): MenuItem[] {
    if (!can(api.PERM.MANAGE_CHANNELS)) return tavernMenuItems();
    return [
      {
        label: "Create Channel",
        icon: faPlus,
        onClick: () => {
          closeMenu();
          openCreateChannel(cat.id);
        },
      },
      { label: "Create Category", icon: faPlus, onClick: createCategoryFlow },
      {
        label: "Delete Category",
        danger: true,
        sep: true,
        onClick: () => {
          closeMenu();
          deleteCategoryFlow(cat);
        },
      },
    ];
  }

  /** Show the Direct Messages home (backed by the hidden home server). */
  async function enterDms() {
    setView("dms");
    setDmSel("friends");
    setActiveConv(null);
    setError(null);
    try {
      await api.setActiveServer("home");
      setActiveServerId("home");
    } catch (e) {
      setError(String(e));
    }
    refreshContacts();
  }

  /** Switch to an already-connected server. Instant - no reconnect or re-login. */
  async function selectServer(s: ServerSession) {
    setError(null);
    try {
      if (s.id !== activeServerId()) {
        await api.setActiveServer(s.id);
        setActiveServerId(s.id);
      }
      setView("server");
      setUnread((u) => ({ ...u, [s.id]: 0 }));
      // Assume healthy on switch; a connection-status event corrects it if not.
      wasConnected = true;
      setConnected(true);
      setMessages([]);
      await loadGroupsAndSelect();
    } catch (e) {
      setError(String(e));
    }
  }

  /** Join (or connect to) another tavern; it stays connected in the background.
   * Taverns use password-less KEY auth: register binds this server's derived
   * identity key (empty password = key-only account), then login signs a
   * challenge. The user never types a per-tavern password. */
  async function addServer(s: ServerSession, registerFirst: boolean, inviteToken?: string) {
    await api.connect(s.id, s.endpoint, s.cert ?? undefined);
    if (registerFirst) {
      try {
        await api.register(s.username, "", s.username, inviteToken);
      } catch {
        /* already registered - fall through to login */
      }
    }
    await api.loginWithKey(s.username, "Desktop");
    setServers((prev) => [...prev.filter((p) => p.id !== s.id), s]);
    setActiveServerId(s.id);
    setView("server");
    setAddOpen(false);
    setMessages([]);
    await loadGroupsAndSelect();
  }

  /** Create + host a new private tavern, then connect to it as owner and switch
   * to it. Reuses the same account credentials as the home node (each tavern has
   * its own DB, so the first account registered on it becomes its owner). */
  async function createTavernFlow(name: string) {
    const t = await api.createTavern(name);
    await addServer(
      {
        id: t.id,
        name: t.name,
        endpoint: t.endpoint,
        cert: t.cert,
        username: props.home.username,
        password: "", // taverns use key auth; no password
      },
      true
    );
    // Persist the name into the tavern's identity (we're now its owner), so the
    // sidebar header shows it instead of the generic "Channels".
    try {
      setTavern(await api.updateTavern(name));
    } catch {
      /* best-effort; the rail glyph still shows the name */
    }
  }

  /** On launch, re-spawn any taverns this client hosts and attach them to the
   * rail in the background (does not change the active view). Best-effort. */
  async function attachResumedTaverns() {
    let list: api.TavernConnect[] = [];
    try {
      list = await api.resumeHostedTaverns();
    } catch {
      return;
    }
    for (const t of list) {
      try {
        await api.connect(t.id, t.endpoint, t.cert ?? undefined);
        await api.loginWithKey(props.home.username, "Desktop");
        setServers((prev) =>
          prev.some((p) => p.id === t.id)
            ? prev
            : [
                ...prev,
                {
                  id: t.id,
                  name: t.name,
                  endpoint: t.endpoint,
                  cert: t.cert,
                  username: props.home.username,
                  password: props.home.password,
                },
              ]
        );
      } catch {
        /* one tavern failing to resume must not block the others */
      }
    }
  }

  /** Refresh the Friend Requests view (retries queued deliveries too). */
  async function syncFr() {
    try {
      const s = await api.syncFriends(props.home.username);
      setFrIncoming(s.incoming);
      setFrPending(s.pending);
    } catch {
      // Not signed in yet / home session briefly down; the next sync catches up.
    }
  }

  // Decode the pasted code as it changes, so the button can reflect "this peer
  // is already pending" before anything is sent.
  createEffect(() => {
    const code = codePaste().trim();
    if (!code) {
      setPasteId(null);
      return;
    }
    api
      .peekContactCode(code)
      .then((p) => {
        if (codePaste().trim() === code) setPasteId(p.peerId);
      })
      .catch(() => setPasteId(null));
  });

  /** Send a friend request from a pasted fr code. The code stays in the box:
   * after the "Sent" flash the button grays out as long as it's still there. */
  async function sendFr() {
    const code = codePaste().trim();
    if (!code || frSendState() !== "idle" || pastePending()) return;
    setError(null);
    setFrNotice(null);
    setFrSendState("sending");
    try {
      const sent = await api.sendFriendRequest(code, props.home.username);
      setFrSendState("sent");
      setTimeout(() => setFrSendState("idle"), 1000);
      setFrNotice(
        sent.delivered
          ? `Request sent to ${sent.displayName ?? sent.name}.`
          : `Request saved - ${sent.name} isn't reachable right now, it will deliver automatically.`
      );
      await syncFr();
    } catch (e) {
      setFrSendState("idle");
      setError(String(e));
    }
  }

  /** Re-attempt delivery of a pending request right now. */
  async function resendFr(p: api.PendingSentRequest) {
    setError(null);
    setFrNotice(null);
    try {
      const sent = await api.resendFriendRequest(p.peerId, props.home.username);
      setFrNotice(`Request re-sent to ${sent.displayName ?? sent.name}.`);
    } catch (e) {
      // "Still unreachable" is expected, not an error state.
      setFrNotice(String(e));
    }
    await syncFr();
  }

  async function respondFr(r: api.IncomingFriendRequest, accept: boolean) {
    setError(null);
    try {
      await api.respondFriendRequest(r.id, r.code, accept, props.home.username);
      await syncFr();
      await refreshContacts();
    } catch (e) {
      setError(String(e));
    }
  }

  async function cancelFr(peerId: string) {
    try {
      await api.cancelFriendRequest(peerId);
      await syncFr();
    } catch (e) {
      setError(String(e));
    }
  }

  async function send(e: Event) {
    e.preventDefault();
    const text = draft().trim();
    const id = activeId();
    if (!text || !id) return;
    setDraft("");
    // A group not in the channel list is a DM group (private).
    const group = groups().find((g) => g.id === id);
    const priv = group ? group.kind === "private" : true;
    if (priv) {
      await api.sendPrivateMessage(id, text);
      appendIfActive({
        id: crypto.randomUUID(),
        groupId: id,
        author: "You",
        content: text,
        timestampMs: Date.now(),
      });
    } else {
      await api.sendPublicMessage(id, text);
    }
  }

  const showInvite = () =>
    api
      .createInviteKey(activeServerId())
      .then((key) => setInvite({ key, error: "" }))
      .catch((e) => setInvite({ key: "", error: String(e) }));

  const serverGlyph = (s: ServerSession) => {
    // Prefer the live active-tavern icon, else the cached per-server icon, else
    // fall back to initials. The cache keeps the icon on the rail after you
    // navigate away from the tavern.
    const icon =
      s.id === activeServerId() && tavern()?.iconUrl
        ? tavern()!.iconUrl
        : tavernIcons()[s.id];
    if (icon) return <img class="rail-icon" src={icon} alt={s.name} />;
    return s.name.slice(0, 2).toUpperCase();
  };
  /** Fetch every connected server's tavern icon into the rail cache (background;
   * does not switch the active session). Best-effort per server. */
  function refreshRailIcons() {
    for (const s of railServers()) {
      api
        .getTavernFor(s.id)
        .then((t) =>
          setTavernIcons((prev) => ({ ...prev, [s.id]: t.iconUrl }))
        )
        .catch(() => {});
    }
  }
  const railServers = () => servers().filter((s) => s.id !== "home");

  /** The Direct Messages main pane: friends list, friend requests, or a (scaffold)
   * conversation. Cross-user delivery is wired in the federation phases. */
  const DmMain = () => (
    <main class="main">
      <header class="main-header">
        <span>
          {dmSel() === "friends"
            ? "Friends"
            : dmSel() === "requests"
              ? "Friend Requests"
              : (activeConv()?.peerName ?? "Direct Message")}
        </span>
        <span class="header-right">
          <Show when={activeConv()}>
            <Show when={activeVoice() !== activeConv()!.groupId}>
              <button
                class="header-icon-btn"
                title="Start a call"
                onClick={() => joinVoiceChannel(activeConv()!.groupId)}
              >
                <Fa icon={faPhone} />
              </button>
            </Show>
            <button class="header-icon-btn" title="Add friends to this DM (group DM)" disabled>
              <Fa icon={faUserPlus} />
            </button>
          </Show>
        </span>
      </header>

      <Switch>
        <Match when={dmSel() === "friends"}>
          <div class="dm-body">
            <Show when={contacts().length === 0 && frPending().length === 0}>
              <p class="empty-note">No friends yet. Add one from Friend Requests.</p>
            </Show>
            <For each={contacts()}>
              {(c) => (
                // Click opens the profile card; right-click the quick menu.
                <button
                  class="contact-row contact-row-click"
                  onClick={(e) => openProfileCard(c, e)}
                  onContextMenu={(e) => showMenu(e, contactMenuItems(c))}
                >
                  <span class="contact-avatar">{(c.name[0] ?? "?").toUpperCase()}</span>
                  <div class="contact-meta">
                    <span class="contact-name">
                      {c.name}
                      <Show when={c.verified}>
                        <span class="verified-badge"> verified</span>
                      </Show>
                      <Show when={isBlocked(c.id)}>
                        <span class="blocked-badge"> blocked</span>
                      </Show>
                    </span>
                    <span class="contact-fp">{c.fingerprint}</span>
                  </div>
                </button>
              )}
            </For>
            {/* Outbound requests appear as placeholder friends until accepted;
                their name/handle refresh from the peer's node once delivered. */}
            <For each={frPending()}>
              {(p) => (
                <div class="contact-row pending-contact">
                  <div class="contact-meta">
                    <span class="contact-name">
                      {p.displayName ?? p.name}
                      <span class="blocked-badge"> pending</span>
                    </span>
                    <span class="contact-fp">
                      {p.username ? `@${p.username} ` : ""}
                      {p.fingerprint}
                    </span>
                  </div>
                  <div class="contact-actions">
                    <button class="btn-secondary btn-sm" onClick={() => setDmSel("requests")}>
                      View request
                    </button>
                  </div>
                </div>
              )}
            </For>
          </div>
        </Match>

        <Match when={dmSel() === "requests"}>
          <div class="dm-body">
            <div class="field">
              <label class="field-label">Your friend code</label>
              <textarea class="invite-input" readOnly value={myCode()} rows={3} />
              <p class="field-help">
                Generate and share this while the mesh is connected so it carries your
                internet-reachable address.
              </p>
              <div class="actions">
                <button class="btn-sm" onClick={() => navigator.clipboard.writeText(myCode())}>
                  Copy my code
                </button>
              </div>
            </div>

            <div class="field">
              <label class="field-label">Send a friend request</label>
              <textarea
                class="invite-input"
                value={codePaste()}
                onInput={(e) => setCodePaste(e.currentTarget.value)}
                placeholder="Paste a friend code (accordc:...)"
                rows={2}
              />
              <div class="actions">
                <button
                  class={pastePending() ? "btn-secondary btn-sm" : "btn-sm"}
                  disabled={!codePaste().trim() || frSendState() !== "idle" || pastePending()}
                  onClick={sendFr}
                >
                  {pastePending()
                    ? "Request pending"
                    : frSendState() === "sent"
                      ? "Sent"
                      : frSendState() === "sending"
                        ? "Sending..."
                        : "Send request"}
                </button>
              </div>
              <Show when={frNotice()}>
                <div class="note note-ok">{frNotice()}</div>
              </Show>
            </div>
            <Show when={error()}>
              <div class="error">{error()}</div>
            </Show>

            <div class="divider" />
            <h4 class="dm-subhead">Incoming requests</h4>
            <For
              each={frIncoming()}
              fallback={<p class="empty-note">No incoming requests.</p>}
            >
              {(r) => (
                <div class="contact-row">
                  <div class="contact-meta">
                    <span class="contact-name">{r.name}</span>
                    <span class="contact-fp">{r.fingerprint}</span>
                  </div>
                  <div class="contact-actions">
                    <button class="btn-sm" onClick={() => respondFr(r, true)}>
                      Accept
                    </button>
                    <button class="btn-secondary btn-sm" onClick={() => respondFr(r, false)}>
                      Decline
                    </button>
                  </div>
                </div>
              )}
            </For>

            <div class="divider" />
            <h4 class="dm-subhead">Pending sent</h4>
            <For
              each={frPending()}
              fallback={<p class="empty-note">No pending requests you've sent.</p>}
            >
              {(p) => (
                <div class="contact-row">
                  <div class="contact-meta">
                    <span class="contact-name">
                      {p.displayName ?? p.name}
                      <Show when={!p.delivered}>
                        <span class="blocked-badge"> not delivered yet</span>
                      </Show>
                      <Show when={p.delivered}>
                        <span class="verified-badge"> awaiting their reply</span>
                      </Show>
                    </span>
                    <span class="contact-fp">
                      {p.username ? `@${p.username} ` : ""}
                      {p.fingerprint}
                    </span>
                  </div>
                  <div class="contact-actions">
                    <button class="btn-sm" onClick={() => resendFr(p)}>
                      Resend
                    </button>
                    <button class="btn-secondary btn-sm" onClick={() => cancelFr(p.peerId)}>
                      Cancel
                    </button>
                  </div>
                </div>
              )}
            </For>
          </div>
        </Match>

        <Match when={true}>
          <Show
            when={!dmOpening()}
            fallback={
              <p class="empty-note center">Opening a secure DM with {activeConv()?.peerName}...</p>
            }
          >
            {/* Call header (when a voice call is active in this DM): both
                avatars + mic/cam/screen/hangup. Media is the WebRTC TODO. */}
            <Show when={activeConv() && activeVoice() === activeConv()!.groupId}>
              <div class="call-stage">
                <div class="call-avatars">
                  <div class="call-avatar">
                    {(props.home.username[0] ?? "?").toUpperCase()}
                  </div>
                  <div class="call-avatar">
                    {(activeConv()?.peerName[0] ?? "?").toUpperCase()}
                  </div>
                </div>
                <div class="call-controls">
                  <button
                    class={`voice-toggle ${localVoice().muted ? "off" : ""}`}
                    title={localVoice().muted ? "Unmute" : "Mute"}
                    onClick={toggleMute}
                  >
                    <Fa icon={localVoice().muted ? faMicrophoneSlash : faMicrophone} />
                  </button>
                  <button
                    class={`voice-toggle ${localVoice().cameraOn ? "on" : ""}`}
                    title="Toggle camera"
                    onClick={toggleCamera}
                  >
                    <Fa icon={faVideo} />
                  </button>
                  <button
                    class={`voice-toggle ${localVoice().screenOn ? "on" : ""}`}
                    title="Share screen"
                    onClick={toggleScreen}
                  >
                    <Fa icon={faDesktop} />
                  </button>
                  <button class="voice-toggle danger" title="Leave call" onClick={leaveVoiceChannel}>
                    <Fa icon={faPhoneSlash} />
                  </button>
                </div>
              </div>
            </Show>
            <div class="messages">
              <For
                each={messages()}
                fallback={
                  <p class="empty-note center">
                    No messages yet. Say hi to {activeConv()?.peerName}.
                    <br />
                    Verify their fingerprint: <code>{activeConv()?.fingerprint}</code>
                  </p>
                }
              >
                {(m) => renderMsg(m)}
              </For>
              <div ref={bottomRef} />
            </div>
            <Composer placeholder={`Message ${activeConv()?.peerName ?? ""}`} />
          </Show>
        </Match>
      </Switch>
    </main>
  );

  /** The message composer: one rounded box with a paperclip attach, the input,
   * and an actions cluster (3 ascending bars that morph into emoji/gif/sticker/
   * send on hover). Enter or the send button submits. Attach/emoji/gif/stickers
   * are scaffolded (the features aren't built yet). */
  const Composer = (cp: { placeholder: string; disabled?: boolean }) => {
    const soon = (what: string) =>
      notifyTransient({ severity: "info", message: `${what} are coming soon.` }, 2500);
    return (
      <form class="composer" onSubmit={send}>
        <button
          type="button"
          class="composer-attach"
          title="Attach a file"
          disabled={cp.disabled}
          onClick={() => soon("File attachments")}
        >
          <Fa icon={faPaperclip} />
        </button>
        <input
          class="composer-input"
          value={draft()}
          onInput={(e) => setDraft(e.currentTarget.value)}
          placeholder={cp.placeholder}
          disabled={cp.disabled}
        />
        <div class="composer-actions">
          <div class="composer-toolbar">
            <button
              type="button"
              class="composer-tool"
              title="Emoji"
              disabled={cp.disabled}
              onClick={() => soon("Emoji")}
            >
              <Fa icon={faFaceSmile} />
            </button>
            <button
              type="button"
              class="composer-tool"
              title="GIF"
              disabled={cp.disabled}
              onClick={() => soon("GIFs")}
            >
              GIF
            </button>
            <button
              type="button"
              class="composer-tool"
              title="Stickers"
              disabled={cp.disabled}
              onClick={() => soon("Stickers")}
            >
              <Fa icon={faNoteSticky} />
            </button>
            <button type="submit" class="composer-send" title="Send" disabled={cp.disabled}>
              <Fa icon={faPaperPlane} />
            </button>
          </div>
          <button type="submit" class="composer-bars" title="Send" disabled={cp.disabled} aria-label="Send">
            <i />
            <i />
            <i />
          </button>
        </div>
      </form>
    );
  };

  return (
    <div class="app">
      <nav class="server-rail">
        <button
          class={`rail-server ${view() === "dms" ? "active" : ""}`}
          title="Direct Messages"
          onClick={enterDms}
        >
          <Fa icon={faComments} />
        </button>
        <div class="rail-divider" />
        <For each={railServers()}>
          {(s) => (
            <button
              class={`rail-server ${view() === "server" && s.id === activeServerId() ? "active" : ""}`}
              title={s.name}
              onClick={() => selectServer(s)}
            >
              {serverGlyph(s)}
              <Show when={(unread()[s.id] ?? 0) > 0}>
                <span class="rail-badge">{(unread()[s.id] ?? 0) > 9 ? "9+" : unread()[s.id]}</span>
              </Show>
            </button>
          )}
        </For>
        <button class="rail-add" title="Add a tavern" onClick={() => setAddOpen(true)}>
          <Fa icon={faPlus} />
        </button>
      </nav>

      <div class="chat">
        <aside class="sidebar">
          <div
            class="sidebar-scroll"
            onContextMenu={(e) => {
              // Right-click anywhere in a tavern's channel list → management menu.
              if (view() === "server") showMenu(e, tavernMenuItems());
            }}
          >
            <Show
              when={view() === "server"}
              fallback={
                <>
                  <div class="sidebar-header">Direct Messages</div>
                  <button
                    class={`channel ${dmSel() === "friends" ? "active" : ""}`}
                    onClick={() => {
                      setDmSel("friends");
                      syncFr(); // pending placeholders render here too
                    }}
                  >
                    <span class="hash">
                      <Fa icon={faUserGroup} />
                    </span>
                    Friends
                  </button>
                  <button
                    class={`channel ${dmSel() === "requests" ? "active" : ""}`}
                    onClick={() => {
                      setDmSel("requests");
                      // Always show a code with the freshest addresses, and pull
                      // in anything parked while we were elsewhere.
                      api.myContactCode(props.home.username).then(setMyCode).catch(() => {});
                      syncFr();
                    }}
                  >
                    <span class="hash">
                      <Fa icon={faUserPlus} />
                    </span>
                    Friend Requests
                  </button>
                  <div class="divider" />
                  <For
                    each={dmConversations()}
                    fallback={<div class="sidebar-empty">No conversations yet.</div>}
                  >
                    {(conv) => (
                      <button
                        class={`channel ${dmSel() === conv.peerId ? "active" : ""}`}
                        onClick={() => openConversation(conv)}
                      >
                        <span class="dm-avatar">{(conv.peerName[0] ?? "?").toUpperCase()}</span>
                        {conv.peerName}
                      </button>
                    )}
                  </For>
                </>
              }
            >
              {/* Banner + clickable header. The header is overlaid on the bottom
                  of the banner (gradient flush to the banner's bottom edge); with
                  no banner it sits in normal flow. The + button is gone - channels
                  are created from a category's + or the context menu. */}
              <div class={`tavern-head ${tavern()?.bannerUrl ? "has-banner" : ""}`}>
                <Show when={tavern()?.bannerUrl}>
                  <div class="tavern-banner">
                    <img src={tavern()!.bannerUrl} alt="" />
                  </div>
                </Show>
                <div class="tavern-header-bar">
                  <button
                    class="tavern-header"
                    title="Tavern menu"
                    onClick={(e) => showMenu(e, tavernMenuItems())}
                  >
                    <Show when={tavern()?.iconUrl}>
                      <img class="tavern-header-icon" src={tavern()!.iconUrl} alt="" />
                    </Show>
                    <span class="tavern-header-name">{tavern()?.name || "Tavern"}</span>
                    <Fa icon={faChevronDown} />
                  </button>
                </div>
              </div>

              {/* Uncategorized channels (rendered above the categories). */}
              <For each={channelsIn("")}>{(g) => channelRow(g, "")}</For>

              {/* Categories, each collapsible, with its channels + a create '+'. */}
              <For each={categories()}>
                {(cat) => (
                  <div
                    class={`category ${dragCatId() && dragCatId() !== cat.id && dragOverCat() === cat.id ? "drop-above" : ""}`}
                    onDragOver={(e) => {
                      if (dragCatId() && dragCatId() !== cat.id) {
                        e.preventDefault();
                        setDragOverCat(cat.id);
                      }
                    }}
                    onDrop={(e) => {
                      if (dragCatId()) {
                        e.preventDefault();
                        dropCategory(cat.id);
                      }
                    }}
                  >
                    <div
                      class="category-header"
                      draggable={can(api.PERM.MANAGE_CHANNELS)}
                      onClick={() => toggleCategory(cat.id)}
                      onContextMenu={(e) => {
                        e.stopPropagation();
                        showMenu(e, categoryMenuItems(cat));
                      }}
                      onDragStart={(e) => {
                        setDragCatId(cat.id);
                        if (e.dataTransfer) {
                          e.dataTransfer.effectAllowed = "move";
                          e.dataTransfer.setData("text/plain", cat.id);
                        }
                      }}
                      onDragEnd={() => {
                        setDragCatId(null);
                        setDragOverCat(null);
                      }}
                    >
                      <Fa
                        icon={collapsedCats()[cat.id] ? faChevronRight : faChevronDown}
                        class="category-caret"
                      />
                      <span class="category-name">{cat.name}</span>
                      <Show when={can(api.PERM.MANAGE_CHANNELS)}>
                        <button
                          class="cat-add"
                          title="Create channel"
                          onClick={(e) => {
                            e.stopPropagation();
                            openCreateChannel(cat.id);
                          }}
                        >
                          <Fa icon={faPlus} />
                        </button>
                      </Show>
                    </div>
                    <Show when={!collapsedCats()[cat.id]}>
                      {/* The channel area is itself a drop zone so channels can be
                          dropped into (the bottom of) this category, incl. empty. */}
                      <div
                        class="category-channels"
                        onDragOver={(e) => {
                          if (dragChannelId()) {
                            e.preventDefault();
                            setDragOverChannel(null);
                          }
                        }}
                        onDrop={(e) => {
                          if (dragChannelId()) {
                            e.preventDefault();
                            dropChannel(cat.id, null);
                          }
                        }}
                      >
                        <For each={channelsIn(cat.id)}>{(g) => channelRow(g, cat.id)}</For>
                      </div>
                    </Show>
                  </div>
                )}
              </For>

              {/* Empty-state CTA so creating the first channel is obvious. DMs do
                  NOT live in a tavern's sidebar - they're in the Direct Messages
                  view (the home rail button); you start one from a contact. */}
              <Show
                when={
                  textChannels().length === 0 &&
                  voiceChannels().length === 0 &&
                  can(api.PERM.MANAGE_CHANNELS)
                }
              >
                <button
                  class="invite-btn"
                  onClick={() => openCreateChannel(categories()[0]?.id ?? "")}
                >
                  <Fa icon={faPlus} />
                  Create a channel
                </button>
              </Show>
              <button
                class="invite-btn"
                title="Create a shareable invite key (tavern owner only)"
                onClick={showInvite}
              >
                <Fa icon={faUserPlus} />
                Invite people
              </button>
              <Show when={error()}>
                <div class="error small">{error()}</div>
              </Show>
            </Show>
          </div>

          {/* Voice/call panel sits directly above the user pill (both views),
              like Discord's "Voice Connected" box. */}
          <Show when={activeVoice()}>
            <div class="voice-panel">
              <div class="voice-panel-row">
                <div class="voice-panel-info">
                  <span class="voice-panel-status">
                    <Fa icon={faVolumeHigh} /> Voice Connected
                  </span>
                  <span class="voice-panel-where">{voiceLabel()}</span>
                </div>
                <button class="voice-panel-hangup" title="Disconnect" onClick={leaveVoiceChannel}>
                  <Fa icon={faPhoneSlash} />
                </button>
              </div>
              <div class="voice-panel-actions">
                <button
                  class={`voice-toggle ${localVoice().muted ? "off" : ""}`}
                  title={localVoice().muted ? "Unmute" : "Mute"}
                  onClick={toggleMute}
                >
                  <Fa icon={localVoice().muted ? faMicrophoneSlash : faMicrophone} />
                </button>
                <button
                  class={`voice-toggle ${deafened() ? "off" : ""}`}
                  title={deafened() ? "Undeafen" : "Deafen"}
                  onClick={toggleDeafen}
                >
                  <Fa icon={deafened() ? faVolumeXmark : faHeadphones} />
                </button>
                <button
                  class={`voice-toggle ${localVoice().cameraOn ? "on" : ""}`}
                  title="Toggle camera"
                  onClick={toggleCamera}
                >
                  <Fa icon={faVideo} />
                </button>
                <button
                  class={`voice-toggle ${localVoice().screenOn ? "on" : ""}`}
                  title="Share screen"
                  onClick={toggleScreen}
                >
                  <Fa icon={faDesktop} />
                </button>
              </div>
            </div>
          </Show>

          <div class="user-card">
            <div class="user-avatar">
              <Show
                when={myProfile()?.avatarUrl}
                fallback={(props.home.username[0] ?? "?").toUpperCase()}
              >
                <img src={myProfile()!.avatarUrl} alt="" />
              </Show>
            </div>
            <div class="user-info">
              <span class="user-name" title={props.home.username}>
                {myProfile()?.displayName || props.home.username}
              </span>
              <span class="user-status">Online</span>
            </div>
            <button
              class="user-gear"
              title="Settings"
              onClick={() => {
                setSettingsTab("profile");
                openProfileSettings();
                setSettingsOpen(true);
              }}
            >
              <Fa icon={faGear} />
            </button>
          </div>
        </aside>

        <Show when={view() === "server"} fallback={<DmMain />}>
          <main class="main">
            <header class="main-header">
              <span>
                {activeGroup()
                  ? `${isPrivate() ? " " : "# "}${activeGroup()!.name}`
                  : "Select a channel"}
              </span>
              <span class="header-right">
                <Show when={!connected()}>
                  <span class="reconnecting" title="The connection dropped; retrying...">
                    Reconnecting...
                  </span>
                </Show>
                <Show when={!isPrivate()}>
                  <button
                    class={`header-icon-btn ${showMembers() ? "active" : ""}`}
                    title="Toggle member list"
                    onClick={() => setShowMembers((v) => !v)}
                  >
                    <Fa icon={faUserGroup} />
                  </button>
                </Show>
              </span>
            </header>

            <div class="messages">
              <For each={messages()}>{(m) => renderMsg(m)}</For>
              <div ref={bottomRef} />
            </div>

            <Composer
              placeholder={
                activeGroup()
                  ? `Message ${isPrivate() ? "" : "#"}${activeGroup()!.name}`
                  : "Select a channel"
              }
              disabled={!activeId()}
            />
          </main>
        </Show>

        <Show when={view() === "server" && showMembers() && !isPrivate()}>
          <aside class="members-panel">
            <Show
              when={members().length}
              fallback={<div class="sidebar-empty">No members.</div>}
            >
              <For each={memberSections()}>
                {(section) => (
                  <div class="member-section">
                    <div class="member-section-header" style={section.color ? { color: section.color } : undefined}>
                      {section.title} - {section.members.length}
                    </div>
                    <For each={section.members}>
                      {(m) => {
                        const top = memberTopRole(m);
                        return (
                          // Right-click (or click) opens the perm-gated member menu.
                          <button
                            class="member-row contact-row-click"
                            onContextMenu={(e) => showMenu(e, memberMenuItems(m))}
                            onClick={(e) => showMenu(e, memberMenuItems(m))}
                          >
                            <span class={`member-dot ${m.online ? "online" : ""}`} />
                            <span class="member-avatar">
                              <Show
                                when={m.avatarUrl}
                                fallback={(m.displayName[0] ?? "?").toUpperCase()}
                              >
                                <img src={m.avatarUrl} alt="" />
                              </Show>
                            </span>
                            <span
                              class="member-name"
                              title={m.username}
                              style={top?.color ? { color: top.color } : undefined}
                            >
                              {m.displayName}
                            </span>
                            <Show when={top?.icon}>
                              <img class="member-role-icon" src={top!.icon} alt="" />
                            </Show>
                            <Show when={m.isOwner}>
                              <span class="role-badge owner">owner</span>
                            </Show>
                          </button>
                        );
                      }}
                    </For>
                  </div>
                )}
              </For>
            </Show>
          </aside>
        </Show>
      </div>

      <Show when={modAlerts().length > 0}>
        <div class="mod-alert-stack">
          <For each={modAlerts()}>
            {(a) => (
              <div class={`mod-alert-toast ${a.severity}`}>
                <Fa icon={faTriangleExclamation} />
                <span>
                  {a.action} on {a.target || "server"} - {a.reason}
                </span>
              </div>
            )}
          </For>
          <button class="mod-alert-clear" onClick={() => setModAlerts([])}>
            dismiss
          </button>
        </div>
      </Show>

      {/* Create-channel side panel (Discord-style), scoped to a category. */}
      <Show when={createChannelCat() !== null}>
        <div class="modal-backdrop" onClick={() => setCreateChannelCat(null)}>
          <div class="create-channel-panel" onClick={(e) => e.stopPropagation()}>
            <button class="settings-close" title="Close" onClick={() => setCreateChannelCat(null)}>
              <Fa icon={faXmark} />
            </button>
            <h3 class="ccp-title">Create Channel</h3>
            <div class="ccp-sub">
              in {categories().find((c) => c.id === createChannelCat())?.name ?? "this tavern"}
            </div>
            <form onSubmit={submitCreateChannel}>
              <label class="field-label">Channel Type</label>
              <button
                type="button"
                class={`ccp-type ${newChannelKind() === "text" ? "active" : ""}`}
                onClick={() => setNewChannelKind("text")}
              >
                <Fa icon={faHashtag} />
                <span class="ccp-type-text">
                  <span class="ccp-type-name">Text</span>
                  <span class="ccp-type-desc">Send messages, images, GIFs, emoji, and more</span>
                </span>
              </button>
              <button
                type="button"
                class={`ccp-type ${newChannelKind() === "voice" ? "active" : ""}`}
                onClick={() => setNewChannelKind("voice")}
              >
                <Fa icon={faVolumeHigh} />
                <span class="ccp-type-text">
                  <span class="ccp-type-name">Voice</span>
                  <span class="ccp-type-desc">Hang out with voice, video, and screen share</span>
                </span>
              </button>

              <label class="field-label ccp-name-label">Channel Name</label>
              <div class="ccp-name-input">
                <Fa icon={newChannelKind() === "voice" ? faVolumeHigh : faHashtag} />
                <input
                  value={newChannelName()}
                  onInput={(e) => setNewChannelName(e.currentTarget.value)}
                  placeholder="new-channel"
                  autofocus
                />
              </div>

              <div class="modal-footer">
                <button type="button" class="btn-secondary" onClick={() => setCreateChannelCat(null)}>
                  Cancel
                </button>
                <button type="submit" disabled={!newChannelName().trim()}>
                  Create Channel
                </button>
              </div>
            </form>
          </div>
        </div>
      </Show>

      {/* Contact popout profile card (click a contact). */}
      <Show when={profileCard()}>
        <div class="overlay-dismiss" onClick={() => setProfileCard(null)} onContextMenu={(e) => { e.preventDefault(); setProfileCard(null); }}>
          <div
            class="profile-card"
            style={{
              left: `${Math.min(profileCard()!.x, window.innerWidth - 280)}px`,
              top: `${Math.min(profileCard()!.y, window.innerHeight - 220)}px`,
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div class="profile-card-head">
              <div class="profile-card-avatar">
                {(profileCard()!.c.name[0] ?? "?").toUpperCase()}
              </div>
              <div class="profile-card-name">
                {profileCard()!.c.name}
                <Show when={profileCard()!.c.verified}>
                  <span class="verified-badge"> verified</span>
                </Show>
              </div>
            </div>
            <div class="profile-card-fp">{profileCard()!.c.fingerprint}</div>
            <div class="profile-card-actions">
              <button
                disabled={isBlocked(profileCard()!.c.id)}
                onClick={() => messageContact(profileCard()!.c)}
              >
                <Fa icon={faComments} /> Message
              </button>
              <button
                class="btn-secondary"
                disabled={isBlocked(profileCard()!.c.id)}
                onClick={() => callContact(profileCard()!.c)}
              >
                <Fa icon={faPhone} /> Call
              </button>
              <button class="btn-secondary" onClick={() => toggleBlocked(profileCard()!.c)}>
                {isBlocked(profileCard()!.c.id) ? "Unblock" : "Block"}
              </button>
            </div>
          </div>
        </div>
      </Show>

      {/* Generic custom context menu (contacts + tavern members). */}
      <Show when={menu()}>
        <div
          class="overlay-dismiss"
          onClick={closeMenu}
          onContextMenu={(e) => {
            e.preventDefault();
            closeMenu();
          }}
        >
          <div
            class="context-menu"
            style={{
              left: `${Math.min(menu()!.x, window.innerWidth - 200)}px`,
              top: `${Math.min(menu()!.y, window.innerHeight - (menu()!.items.length * 34 + 16))}px`,
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <For each={menu()!.items}>
              {(it) => (
                <>
                  <Show when={it.sep}>
                    <div class="context-sep" />
                  </Show>
                  <button
                    class={`context-item ${it.danger ? "danger" : ""}`}
                    onClick={() => it.onClick()}
                  >
                    <Show when={it.icon}>
                      <Fa icon={it.icon!} />
                    </Show>
                    {it.label}
                  </button>
                </>
              )}
            </For>
          </div>
        </div>
      </Show>

      {/* Tavern (server) settings. */}
      {/* Full-screen Server (tavern) settings, Discord-style left nav + sections. */}
      <Show when={tavernSettingsOpen()}>
        <div class="settings-page">
          <nav class="settings-nav-col">
            <div class="settings-nav-title">{tavern()?.name || "Tavern"}</div>
            <button
              class={`settings-nav-item ${settingsSection() === "overview" ? "active" : ""}`}
              onClick={() => selectSettingsSection("overview")}
            >
              Overview
            </button>
            <div class="settings-nav-group">People</div>
            <button
              class={`settings-nav-item ${settingsSection() === "members" ? "active" : ""}`}
              onClick={() => selectSettingsSection("members")}
            >
              Members
            </button>
            <button
              class={`settings-nav-item ${settingsSection() === "roles" ? "active" : ""}`}
              onClick={() => selectSettingsSection("roles")}
            >
              Roles
            </button>
            <button
              class={`settings-nav-item ${settingsSection() === "invites" ? "active" : ""}`}
              onClick={() => selectSettingsSection("invites")}
            >
              Invites
            </button>
            <div class="settings-nav-group">Moderation</div>
            <button
              class={`settings-nav-item ${settingsSection() === "audit" ? "active" : ""}`}
              onClick={() => selectSettingsSection("audit")}
            >
              Audit Log
            </button>
            <button
              class={`settings-nav-item ${settingsSection() === "bans" ? "active" : ""}`}
              onClick={() => selectSettingsSection("bans")}
            >
              Bans
            </button>
            <button
              class={`settings-nav-item ${settingsSection() === "automod" ? "active" : ""}`}
              onClick={() => selectSettingsSection("automod")}
            >
              AutoMod
            </button>
            <div class="settings-nav-sep" />
            <Show when={myPerms()?.isOwner}>
              <button class="settings-nav-item danger" onClick={deleteTavernFlow}>
                Delete Tavern
              </button>
            </Show>
          </nav>

          <button
            class="settings-close"
            title="Close (Esc)"
            onClick={() => guardUnsaved() && setTavernSettingsOpen(false)}
          >
            <Fa icon={faXmark} />
            <span class="settings-close-label">ESC</span>
          </button>

          <div class="settings-content">
            <Switch>
              <Match when={settingsSection() === "overview"}>
                {(() => {
                  const ro = () => !can(api.PERM.MANAGE_SERVER);
                  return (
                    <>
                      <h2 class="settings-h">Tavern Profile</h2>
                      <form onSubmit={saveTavernSettings} class="settings-form">
                        <div class="field">
                          <label class="field-label">Tavern name</label>
                          <input
                            value={tavName()}
                            disabled={ro()}
                            onInput={(e) => setTavName(e.currentTarget.value)}
                          />
                        </div>

                        <label class="field-label">Tavern icon</label>
                        <div class="field-help">
                          Shown in the server rail and the sidebar header. Resized to 128x128 and
                          stored with the tavern.
                        </div>
                        <div class="role-icon-row">
                          <span class="tavern-icon-preview">
                            <Show
                              when={tavIcon()}
                              fallback={<span>{(tavName()[0] ?? "?").toUpperCase()}</span>}
                            >
                              <img src={tavIcon()} alt="tavern icon" />
                            </Show>
                          </span>
                          <Show when={!ro()}>
                            <label class="btn-secondary btn-sm file-btn">
                              {tavIcon() ? "Change icon" : "Choose image"}
                              <input
                                type="file"
                                accept="image/*"
                                onChange={(e) => {
                                  downscaleImage(e.currentTarget.files?.[0], 128, 128, setTavIcon);
                                  e.currentTarget.value = "";
                                }}
                              />
                            </label>
                            <Show when={tavIcon()}>
                              <button
                                type="button"
                                class="btn-secondary btn-sm btn-danger-text"
                                onClick={() => setTavIcon("")}
                              >
                                Remove icon
                              </button>
                            </Show>
                          </Show>
                        </div>

                        <label class="field-label">Tavern banner</label>
                        <div class="field-help">
                          A wide image shown at the top of the channel list. Resized to 640x240.
                        </div>
                        <div class="tavern-banner-edit">
                          <span class="tavern-banner-preview">
                            <Show
                              when={tavBanner()}
                              fallback={<span class="tavern-banner-empty">No banner</span>}
                            >
                              <img src={tavBanner()} alt="tavern banner" />
                            </Show>
                          </span>
                          <Show when={!ro()}>
                            <div class="role-icon-row">
                              <label class="btn-secondary btn-sm file-btn">
                                {tavBanner() ? "Change banner" : "Choose image"}
                                <input
                                  type="file"
                                  accept="image/*"
                                  onChange={(e) => {
                                    downscaleImage(
                                      e.currentTarget.files?.[0],
                                      640,
                                      240,
                                      setTavBanner
                                    );
                                    e.currentTarget.value = "";
                                  }}
                                />
                              </label>
                              <Show when={tavBanner()}>
                                <button
                                  type="button"
                                  class="btn-secondary btn-sm btn-danger-text"
                                  onClick={() => setTavBanner("")}
                                >
                                  Remove banner
                                </button>
                              </Show>
                            </div>
                          </Show>
                        </div>

                        <div class="field">
                          <label class="field-label">Description</label>
                          <input
                            value={tavDesc()}
                            disabled={ro()}
                            onInput={(e) => setTavDesc(e.currentTarget.value)}
                          />
                        </div>
                        <Show when={!ro()}>
                          <div class="actions">
                            <button type="submit">Save changes</button>
                          </div>
                        </Show>
                      </form>
                    </>
                  );
                })()}
              </Match>

              <Match when={settingsSection() === "members"}>
                <h2 class="settings-h">Members - {members().length}</h2>
                <div class="settings-table">
                  <For each={members()} fallback={<div class="empty-note">No members.</div>}>
                    {(m) => (
                      <div class="settings-row">
                        <span class="member-avatar">
                          <Show
                            when={m.avatarUrl}
                            fallback={(m.displayName[0] ?? "?").toUpperCase()}
                          >
                            <img src={m.avatarUrl} alt="" />
                          </Show>
                        </span>
                        <span class="settings-row-main">
                          <span class="member-name">{m.displayName}</span>
                          <span class="member-roles">
                            <span class="contact-fp">@{m.username}</span>
                            <For each={m.roleIds}>
                              {(rid) => (
                                <span class="member-role-chip">
                                  {roleName(rid)}
                                  <Show when={can(api.PERM.MANAGE_ROLES)}>
                                    <button
                                      class="chip-x"
                                      title="Remove role"
                                      onClick={() => unassignRoleFromMember(m.userId, rid)}
                                    >
                                      <Fa icon={faXmark} />
                                    </button>
                                  </Show>
                                </span>
                              )}
                            </For>
                          </span>
                        </span>
                        <Show when={m.isOwner}>
                          <span class="role-badge owner">owner</span>
                        </Show>
                        <span class="settings-row-actions">
                          <Show when={can(api.PERM.MANAGE_ROLES) && !m.isOwner}>
                            <select
                              class="role-assign-select"
                              value=""
                              onChange={(e) => {
                                assignRoleToMember(m.userId, e.currentTarget.value);
                                e.currentTarget.value = "";
                              }}
                            >
                              <option value="">+ Role</option>
                              <For each={roles().filter((r) => !r.isDefault && !m.roleIds.includes(r.id))}>
                                {(r) => <option value={r.id}>{r.name}</option>}
                              </For>
                            </select>
                          </Show>
                          <Show when={can(api.PERM.KICK_MEMBERS) && !m.isOwner}>
                            <button
                              class="btn-secondary btn-sm"
                              onClick={() =>
                                activeId() &&
                                api
                                  .kickMember(activeId()!, m.userId)
                                  .then(() => refreshMembers(activeId()))
                                  .catch((e) => setError(String(e)))
                              }
                            >
                              Kick
                            </button>
                          </Show>
                          <Show when={can(api.PERM.BAN_MEMBERS) && !m.isOwner}>
                            <button
                              class="btn-secondary btn-sm btn-danger-text"
                              onClick={async () => {
                                const ok = await askConfirm({
                                  title: "Ban member",
                                  body: `Ban ${m.displayName} (@${m.username})? They will be removed and blocked from rejoining this tavern.`,
                                  confirmLabel: "Ban member",
                                  danger: true,
                                });
                                if (!ok) return;
                                api
                                  .banMember(m.userId)
                                  .then(() => refreshMembers(activeId()))
                                  .catch((e) => setError(String(e)));
                              }}
                            >
                              Ban
                            </button>
                          </Show>
                        </span>
                      </div>
                    )}
                  </For>
                </div>
              </Match>

              <Match when={settingsSection() === "bans"}>
                <h2 class="settings-h">Bans</h2>
                <div class="settings-table">
                  <For each={settingsBans()} fallback={<div class="empty-note">No bans.</div>}>
                    {(b) => (
                      <div class="settings-row">
                        <span class="settings-row-main">
                          <span class="member-name">{b.userId.slice(0, 12)}</span>
                          <span class="contact-fp">{b.reason || "(no reason)"}</span>
                        </span>
                        <Show when={can(api.PERM.BAN_MEMBERS)}>
                          <button class="btn-secondary btn-sm" onClick={() => unbanUser(b.userId)}>
                            Unban
                          </button>
                        </Show>
                      </div>
                    )}
                  </For>
                </div>
              </Match>

              <Match when={settingsSection() === "audit"}>
                <h2 class="settings-h">Audit Log</h2>
                <div class="settings-table">
                  <For
                    each={settingsAudit()}
                    fallback={<div class="empty-note">No audit entries yet.</div>}
                  >
                    {(a) => (
                      <div class="settings-row">
                        <span class={`audit-verdict ${a.verdict}`}>{a.verdict}</span>
                        <span class="settings-row-main">
                          <span class="member-name">
                            {a.action} {a.target ? `→ ${a.target}` : ""}
                          </span>
                          <span class="contact-fp">
                            {a.actorId.slice(0, 8)} · {new Date(a.createdAtMs).toLocaleString()}
                            {a.reason ? ` · ${a.reason}` : ""}
                          </span>
                        </span>
                      </div>
                    )}
                  </For>
                </div>
              </Match>

              <Match when={settingsSection() === "roles"}>
                <div class="settings-head-row">
                  <h2 class="settings-h">Roles</h2>
                  <Show when={can(api.PERM.MANAGE_ROLES)}>
                    <button class="btn-sm" onClick={startNewRole}>
                      <Fa icon={faPlus} /> New role
                    </button>
                  </Show>
                </div>
                <p class="field-help">
                  Members use the color of their highest role. Drag roles to reorder them -
                  roles higher in the list outrank lower ones, so a higher role can manage,
                  kick, and ban members whose top role is lower. @everyone is always at the
                  bottom.
                </p>
                <div class="roles-layout">
                  <div
                    class="settings-table roles-list"
                    onDragOver={(e) => {
                      if (!dragRoleId()) return;
                      e.preventDefault();
                      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
                      // Empty space below the list = drop at the end (above
                      // @everyone). Item handlers stop propagation, so reaching
                      // here means the cursor is over the container's padding.
                      setDragOverId(roles().find((r) => r.isDefault)?.id ?? null);
                    }}
                    onDrop={(e) => {
                      e.preventDefault();
                      dropRoleBefore(null);
                    }}
                  >
                    <For
                      each={roleListOrdered()}
                      fallback={<div class="empty-note">No roles yet.</div>}
                    >
                      {(r) => (
                        <button
                          class={`role-list-item ${editingRole()?.id === r.id && r.id ? "active" : ""} ${dragRoleId() === r.id ? "dragging" : ""} ${dragRoleId() && dragRoleId() !== r.id && dragOverId() === r.id ? "drop-above" : ""}`}
                          draggable={can(api.PERM.MANAGE_ROLES) && !r.isDefault}
                          onClick={() => editRole(r)}
                          onDragStart={(e) => {
                            if (r.isDefault) return;
                            setDragRoleId(r.id);
                            if (e.dataTransfer) {
                              e.dataTransfer.effectAllowed = "move";
                              // Some engines require data to be set for a valid drag.
                              e.dataTransfer.setData("text/plain", r.id);
                            }
                          }}
                          onDragEnd={() => {
                            setDragRoleId(null);
                            setDragOverId(null);
                          }}
                          onDragOver={(e) => {
                            if (!dragRoleId()) return;
                            e.preventDefault();
                            e.stopPropagation();
                            if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
                            // Hovering any row (incl. @everyone) sets the insert
                            // point ABOVE it; @everyone means "end of normal roles".
                            setDragOverId(r.id);
                          }}
                          onDrop={(e) => {
                            e.preventDefault();
                            e.stopPropagation();
                            dropRoleBefore(r.isDefault ? null : r.id);
                          }}
                        >
                          <span
                            class="role-dot"
                            style={{ background: r.color || "var(--text-dim)" }}
                          >
                            <Show when={r.icon}>
                              <img class="role-dot-icon" src={r.icon} alt="" />
                            </Show>
                          </span>
                          <span
                            class="role-list-name"
                            style={r.color ? { color: r.color } : undefined}
                          >
                            {r.name}
                          </span>
                          <Show when={r.isDefault}>
                            <span class="role-badge">default</span>
                          </Show>
                        </button>
                      )}
                    </For>
                  </div>

                  <Show
                    when={editingRole()}
                    fallback={
                      <div class="role-editor empty">
                        <div class="empty-note">
                          Select a role to edit, or create a new one.
                        </div>
                      </div>
                    }
                  >
                    {(role) => {
                      const readOnly = () => !can(api.PERM.MANAGE_ROLES);
                      return (
                        <div class="role-editor">
                          <div class="role-editor-head">
                            <span class="role-editor-title">
                              {role().id ? "Edit role" : "New role"} -{" "}
                              <span class="role-editor-rolename">{roleDraftName()}</span>
                            </span>
                            <Show when={role().id && !role().isDefault && can(api.PERM.MANAGE_ROLES)}>
                              <button
                                class="icon-btn danger"
                                title="Delete role"
                                onClick={() => deleteRoleFlow(role())}
                              >
                                <Fa icon={faTrash} />
                              </button>
                            </Show>
                          </div>

                          <div class="role-tabs">
                            <button
                              class={`role-tab ${roleTab() === "display" ? "active" : ""}`}
                              onClick={() => setRoleTab("display")}
                            >
                              Display
                            </button>
                            <button
                              class={`role-tab ${roleTab() === "permissions" ? "active" : ""}`}
                              onClick={() => setRoleTab("permissions")}
                            >
                              Permissions
                            </button>
                          </div>

                          <Switch>
                            <Match when={roleTab() === "display"}>
                              <div class="field">
                                <label class="field-label">Role name</label>
                                <input
                                  value={roleDraftName()}
                                  disabled={readOnly() || role().isDefault}
                                  onInput={(e) => setRoleDraftName(e.currentTarget.value)}
                                />
                                <Show when={role().isDefault}>
                                  <div class="field-help">The @everyone role can't be renamed.</div>
                                </Show>
                              </div>

                              <label class="field-label">Role color</label>
                              <div class="field-help">
                                Members use the color of the highest role they have.
                              </div>
                              <div class="color-swatches">
                                <button
                                  class={`color-swatch none ${!roleDraftColor() ? "active" : ""}`}
                                  title="No color"
                                  disabled={readOnly()}
                                  onClick={() => setRoleDraftColor("")}
                                >
                                  <Fa icon={faXmark} />
                                </button>
                                <For each={ROLE_COLORS}>
                                  {(c) => (
                                    <button
                                      class={`color-swatch ${roleDraftColor() === c ? "active" : ""}`}
                                      style={{ background: c }}
                                      disabled={readOnly()}
                                      onClick={() => setRoleDraftColor(c)}
                                    />
                                  )}
                                </For>
                                <label class="color-swatch custom" title="Custom color">
                                  <input
                                    type="color"
                                    disabled={readOnly()}
                                    value={roleDraftColor() || "#5865f2"}
                                    onInput={(e) => setRoleDraftColor(e.currentTarget.value)}
                                  />
                                </label>
                              </div>

                              <label class="field-label">Role icon</label>
                              <div class="field-help">
                                Shown next to the names of members whose highest role is this
                                one. Upload a small image (resized to 64x64, stored inline).
                              </div>
                              <div class="role-icon-row">
                                <span class="role-icon-preview">
                                  <Show
                                    when={roleDraftIcon()}
                                    fallback={<Fa icon={faUserGroup} />}
                                  >
                                    <img src={roleDraftIcon()} alt="role icon" />
                                  </Show>
                                </span>
                                <label class="btn-secondary btn-sm file-btn">
                                  Choose image
                                  <input
                                    type="file"
                                    accept="image/*"
                                    disabled={readOnly()}
                                    onChange={(e) => {
                                      pickRoleIcon(e.currentTarget.files?.[0]);
                                      e.currentTarget.value = "";
                                    }}
                                  />
                                </label>
                                <Show when={roleDraftIcon()}>
                                  <button
                                    class="btn-secondary btn-sm"
                                    disabled={readOnly()}
                                    onClick={() => setRoleDraftIcon("")}
                                  >
                                    Remove
                                  </button>
                                </Show>
                              </div>

                              <Show when={!role().isDefault}>
                                <label class="toggle-row">
                                  <span class="toggle-text">
                                    <span class="toggle-title">
                                      Display role members separately from online members
                                    </span>
                                    <span class="toggle-desc">
                                      Members with this role get their own section in the member
                                      list, with a count.
                                    </span>
                                  </span>
                                  <input
                                    type="checkbox"
                                    class="switch"
                                    disabled={readOnly()}
                                    checked={roleDraftHoist()}
                                    onChange={(e) => setRoleDraftHoist(e.currentTarget.checked)}
                                  />
                                </label>
                              </Show>

                              <label class="toggle-row">
                                <span class="toggle-text">
                                  <span class="toggle-title">Allow anyone to @mention this role</span>
                                  <span class="toggle-desc">
                                    When on, any member can ping everyone with this role.
                                  </span>
                                </span>
                                <input
                                  type="checkbox"
                                  class="switch"
                                  disabled={readOnly()}
                                  checked={roleDraftMentionable()}
                                  onChange={(e) =>
                                    setRoleDraftMentionable(e.currentTarget.checked)
                                  }
                                />
                              </label>
                            </Match>

                            <Match when={roleTab() === "permissions"}>
                              <For each={api.PERM_GROUPS}>
                                {(group) => (
                                  <div class="perm-group">
                                    <h3 class="perm-group-title">{group.category}</h3>
                                    <For each={group.perms}>
                                      {(p) => (
                                        <div
                                          class={`perm-row ${p.key === "ADMINISTRATOR" ? "perm-admin" : ""}`}
                                        >
                                          <span class="perm-text">
                                            <span class="perm-label">{p.label}</span>
                                            <span class="perm-desc">{p.desc}</span>
                                          </span>
                                          <input
                                            type="checkbox"
                                            class="switch"
                                            disabled={
                                              readOnly() ||
                                              (p.key === "ADMINISTRATOR" &&
                                                !can(api.PERM.ADMINISTRATOR))
                                            }
                                            checked={(roleDraftPerms() & p.bit) !== 0n}
                                            onChange={(e) =>
                                              toggleRolePerm(p.bit, e.currentTarget.checked)
                                            }
                                          />
                                        </div>
                                      )}
                                    </For>
                                  </div>
                                )}
                              </For>
                            </Match>
                          </Switch>
                        </div>
                      );
                    }}
                  </Show>
                </div>
              </Match>

              <Match when={settingsSection() === "invites"}>
                <h2 class="settings-h">Invites</h2>
                <p class="field-help">Generate a shareable invite key for this tavern.</p>
                <div class="actions">
                  <button onClick={showInvite}>Create invite key</button>
                </div>
                <Show when={invite()}>
                  <textarea class="invite-input" readOnly rows={3} value={invite()!.key} />
                </Show>
              </Match>

              <Match when={settingsSection() === "automod"}>
                <h2 class="settings-h">AutoMod</h2>
                <div class="note">
                  Rate-limit + name-heuristic guardrails already run server-side and write the
                  Audit Log. A configurable AutoMod rules UI is coming soon.
                </div>
              </Match>
            </Switch>
          </div>

          {/* Sticky unsaved-changes bar (Discord-style), shown while a role draft
              differs from its saved state. Navigation is blocked until resolved. */}
          <Show when={editingRole() && roleDirty()}>
            <div class="unsaved-bar">
              <span class="unsaved-text">Careful - you have unsaved changes!</span>
              <button class="link-btn" onClick={resetRoleDraft}>
                Reset
              </button>
              <button class="btn-save" disabled={roleBusy()} onClick={saveRole}>
                Save Changes
              </button>
            </div>
          </Show>
        </div>
      </Show>

      {/* Custom confirmation dialog - replaces the native window.confirm box. */}
      <Show when={confirmDialog()}>
        <div class="modal-backdrop confirm-backdrop" onClick={() => resolveConfirm(false)}>
          <div class="modal confirm-modal" onClick={(e) => e.stopPropagation()}>
            <h3>{confirmDialog()!.title}</h3>
            <p class="confirm-body">{confirmDialog()!.body}</p>
            <div class="modal-footer">
              <button class="btn-secondary" onClick={() => resolveConfirm(false)}>
                {confirmDialog()!.cancelLabel ?? "Cancel"}
              </button>
              <button
                class={confirmDialog()!.danger ? "btn-danger" : ""}
                onClick={() => resolveConfirm(true)}
              >
                {confirmDialog()!.confirmLabel ?? "Confirm"}
              </button>
            </div>
          </div>
        </div>
      </Show>

      <Show when={addOpen()}>
        <AddServerModal
          defaultUsername={props.home.username}
          onClose={() => setAddOpen(false)}
          onJoin={addServer}
          onCreatePrivate={createTavernFlow}
        />
      </Show>

      <Show when={settingsOpen()}>
        <div class="modal-backdrop" onClick={() => { stopMicTest(); setSettingsOpen(false); }}>
          <div class="modal settings-modal" onClick={(e) => e.stopPropagation()}>
            <h3>Settings</h3>

            <div class="settings-body">
              <nav class="settings-nav">
                <For
                  each={
                    [
                      ["profile", "Profile"],
                      ["privacy", "Privacy"],
                      ["voice", "Voice & Video"],
                      ["friends", "Friends"],
                      ["network", "Yggdrasil"],
                      ["nodes", "Nodes"],
                    ] as const
                  }
                >
                  {([key, label]) => (
                    <button
                      class={`settings-nav-item ${settingsTab() === key ? "active" : ""}`}
                      onClick={() => {
                        if (key !== "voice") stopMicTest();
                        setSettingsTab(key);
                        if (key === "voice") loadAudioDevices();
                        if (key === "profile") openProfileSettings();
                      }}
                    >
                      {label}
                    </button>
                  )}
                </For>
              </nav>

              <div class="settings-content">
                <Show when={settingsTab() === "profile"}>
                  <form class="settings-section" onSubmit={saveProfile}>
                    <h4>Profile</h4>
                    <p class="field-help">
                      This is your profile on <b>{tavern()?.name || "this server"}</b>. Each tavern
                      has its own profile (name + avatar); your home account is your main one.
                    </p>
                    <div class="field">
                      <label class="field-label">Avatar</label>
                      <div class="role-icon-row">
                        <span class="profile-avatar-preview">
                          <Show
                            when={profAvatar()}
                            fallback={(profName()[0] ?? "?").toUpperCase()}
                          >
                            <img src={profAvatar()} alt="avatar" />
                          </Show>
                        </span>
                        <label class="btn-secondary btn-sm file-btn">
                          {profAvatar() ? "Change avatar" : "Choose image"}
                          <input
                            type="file"
                            accept="image/*"
                            onChange={(e) => {
                              downscaleImage(e.currentTarget.files?.[0], 128, 128, setProfAvatar);
                              e.currentTarget.value = "";
                            }}
                          />
                        </label>
                        <Show when={profAvatar()}>
                          <button
                            type="button"
                            class="btn-secondary btn-sm btn-danger-text"
                            onClick={() => setProfAvatar("")}
                          >
                            Remove
                          </button>
                        </Show>
                      </div>
                    </div>
                    <div class="field">
                      <label class="field-label">Display name</label>
                      <input
                        value={profName()}
                        onInput={(e) => setProfName(e.currentTarget.value)}
                        placeholder="Your name on this server"
                      />
                      <p class="field-help">
                        Your username is <code>{myProfile()?.username || props.home.username}</code>.
                      </p>
                    </div>
                    <div class="actions">
                      <button type="submit">Save profile</button>
                    </div>
                  </form>
                </Show>

                <Show when={settingsTab() === "privacy"}>
                  <div class="settings-section">
                    <h4>Privacy</h4>
                    <div class="field">
                      <label class="check">
                        <input
                          type="checkbox"
                          checked={encryptAtRest()}
                          onChange={toggleEncryptAtRest}
                        />
                        Encrypt my messages at rest
                      </label>
                      <p class="field-help">
                        Messages you receive are always encrypted on disk. This also encrypts the
                        messages you send, at a small cost to load speed.
                      </p>
                    </div>
                  </div>
                </Show>

                <Show when={settingsTab() === "voice"}>
                  <div class="settings-section">
                    <h4>Voice &amp; Video</h4>
                    <div class="field">
                      <label class="field-label" for="mic-device">Microphone</label>
                      <select
                        id="mic-device"
                        value={voicePrefs().micDeviceId}
                        onChange={(e) => updateVoicePrefs({ micDeviceId: e.currentTarget.value })}
                      >
                        <option value="">System default</option>
                        <For each={audioInputs()}>
                          {(d) => (
                            <option value={d.deviceId}>
                              {d.label || `Microphone ${d.deviceId.slice(0, 6)}`}
                            </option>
                          )}
                        </For>
                      </select>
                    </div>
                    <div class="field">
                      <label class="field-label" for="spk-device">Output device</label>
                      <select
                        id="spk-device"
                        value={voicePrefs().speakerDeviceId}
                        onChange={(e) => updateVoicePrefs({ speakerDeviceId: e.currentTarget.value })}
                      >
                        <option value="">System default</option>
                        <For each={audioOutputs()}>
                          {(d) => (
                            <option value={d.deviceId}>
                              {d.label || `Speaker ${d.deviceId.slice(0, 6)}`}
                            </option>
                          )}
                        </For>
                      </select>
                      <p class="field-help">Device names appear after you've allowed mic access once.</p>
                    </div>

                    <div class="field">
                      <label class="field-label">
                        Microphone volume - {voicePrefs().micGain}%
                      </label>
                      <input
                        type="range"
                        min="0"
                        max="200"
                        value={voicePrefs().micGain}
                        onInput={(e) =>
                          updateVoicePrefs({ micGain: Number(e.currentTarget.value) })
                        }
                      />
                    </div>
                    <div class="field">
                      <label class="field-label">
                        Output volume - {voicePrefs().outputVolume}%
                      </label>
                      <input
                        type="range"
                        min="0"
                        max="200"
                        value={voicePrefs().outputVolume}
                        onInput={(e) =>
                          updateVoicePrefs({ outputVolume: Number(e.currentTarget.value) })
                        }
                      />
                    </div>

                    <div class="field">
                      <label class="field-label">Mic test</label>
                      <div class="mic-test-row">
                        <button
                          type="button"
                          class={micTesting() ? "btn-danger" : ""}
                          onClick={toggleMicTest}
                        >
                          {micTesting() ? "Stop test" : "Mic test"}
                        </button>
                        <div class="mic-test-meter">
                          <div
                            class="mic-test-fill"
                            style={{ width: `${Math.round(micTestLevel() * 100)}%` }}
                          />
                        </div>
                      </div>
                      <p class="field-help">
                        Routes your mic through a local WebRTC loopback and plays the result back,
                        so you can confirm WebRTC carries your audio and tune the volume sliders
                        live - all before joining a call. Use headphones to avoid echo.
                      </p>
                    </div>

                    <h4>Input processing</h4>
                    <div class="field">
                      <label class="field-label" for="ns-mode">Noise suppression</label>
                      <select
                        id="ns-mode"
                        value={voicePrefs().noiseSuppression}
                        onChange={(e) =>
                          updateVoicePrefs({
                            noiseSuppression: e.currentTarget
                              .value as voicePrefsMod.NoiseSuppression,
                          })
                        }
                      >
                        <option value="none">None</option>
                        <option value="standard">Standard (built-in)</option>
                        <option value="rnnoise">RNNoise (recommended)</option>
                      </select>
                      <p class="field-help">
                        Reduces background noise from your mic. "Standard" uses the built-in WebRTC
                        suppression; "RNNoise" is a stronger AI model (the free, open-source
                        equivalent of Discord's Krisp) that runs locally.
                      </p>
                    </div>
                    <div class="field">
                      <label class="check">
                        <input
                          type="checkbox"
                          checked={voicePrefs().echoCancellation}
                          onChange={(e) =>
                            updateVoicePrefs({ echoCancellation: e.currentTarget.checked })
                          }
                        />
                        Echo cancellation
                      </label>
                    </div>
                    <div class="field">
                      <label class="check">
                        <input
                          type="checkbox"
                          checked={voicePrefs().autoGain}
                          onChange={(e) => updateVoicePrefs({ autoGain: e.currentTarget.checked })}
                        />
                        Automatic gain control
                      </label>
                      <p class="field-help">
                        Changes apply immediately if you're in a call, otherwise on your next join.
                      </p>
                    </div>
                  </div>
                </Show>

                <Show when={settingsTab() === "friends"}>
                  <div class="settings-section">
                    <h4>Friend requests</h4>
                    <div class="field">
                      <label class="field-label" for="friend-policy">
                        Who can add me
                      </label>
                      <select
                        id="friend-policy"
                        value={friendPolicy()}
                        onChange={(e) =>
                          changeFriendPolicy(e.currentTarget.value as api.FriendRequestPolicy)
                        }
                      >
                        <option value="everyone">Anyone with my code</option>
                        <option value="tavern_members">People in my taverns</option>
                        <option value="friends_of_friends">Friends of friends</option>
                        <option value="no_one">No one (I add others)</option>
                      </select>
                      <p class="field-help">
                        Gates who may send you a friend request. Enforced once cross-user request
                        delivery lands.
                      </p>
                    </div>
                  </div>
                </Show>

                <Show when={settingsTab() === "network"}>
                  <div class="settings-section">
                    <h4>Yggdrasil peers</h4>
                    <Show when={!mesh()?.available}>
                      <div class="note note-warn">
                        Mesh networking is not compiled into this build, so connecting will fail.
                        Internet DMs need a mesh-enabled build run with admin privileges.
                      </div>
                    </Show>

                    <div class="seg-pill">
                      <button
                        class={yggMode() === "authorized" ? "active" : ""}
                        onClick={() => setYggMode("authorized")}
                      >
                        Authorized
                      </button>
                      <button
                        class={yggMode() === "private" ? "active" : ""}
                        onClick={() => setYggMode("private")}
                      >
                        Private
                      </button>
                      <button
                        class={yggMode() === "public" ? "active" : ""}
                        onClick={() => setYggMode("public")}
                      >
                        Public
                      </button>
                    </div>

                    <Show when={yggMode() === "authorized"}>
                      <div class="note note-warn">
                        Authorized peers are hosted by Accord. They are trustworthy, but
                        connection metadata may be logged in accordance with our policies - never
                        message content.
                      </div>
                    </Show>

                    <Show when={yggMode() === "private"}>
                      <div class="ygg-instructions">
                        <b>Host your own Yggdrasil peer</b>
                        <ol>
                          <li>
                            Install Yggdrasil on an always-on machine (packages for
                            Windows/Linux/macOS at yggdrasil-network.github.io).
                          </li>
                          <li>
                            Generate a config: <code>yggdrasil -genconf &gt; yggdrasil.conf</code>
                          </li>
                          <li>
                            In the config, set <code>Listen: ["tls://0.0.0.0:PORT"]</code> and
                            forward that port on your router.
                          </li>
                          <li>Run it as a service so it stays online.</li>
                          <li>
                            Your peer URI is <code>tls://your-host-or-ip:PORT</code> - paste it
                            below (one per line; tcp:// and tls:// are supported, IPv6 hosts in
                            brackets).
                          </li>
                        </ol>
                      </div>
                      <textarea
                        class="invite-input"
                        rows={4}
                        value={yggPeersText()}
                        onInput={(e) => setYggPeersText(e.currentTarget.value)}
                        placeholder={"tls://a.b.c.d:e\ntcp://[a:b:c::d]:e"}
                      />
                    </Show>

                    <Show when={yggMode() === "public"}>
                      <div class="note note-warn">
                        Public peers are community-run. Their operators may observe or collect
                        connection metadata (your IP and traffic timing - never message content).
                        The app picks the fastest reachable peers for your region from the
                        official yggdrasil-network lists and migrates automatically if they go
                        offline.
                      </div>
                    </Show>

                    <div class="actions">
                      <button onClick={connectMesh} disabled={meshConn()?.state === "connecting"}>
                        Connect
                      </button>
                      <Show when={mesh()?.running}>
                        <button class="btn-secondary" onClick={disconnectMesh}>
                          Disconnect
                        </button>
                      </Show>
                    </div>

                    <Show when={meshConn()}>
                      <div class="conn-row">
                        <Show when={meshConn()!.state === "connecting"}>
                          <span class="throbber" />
                        </Show>
                        <span
                          class={`conn-status ${
                            meshConn()!.state === "connecting"
                              ? "connecting"
                              : meshConn()!.state === "error"
                                ? "error"
                                : "ok"
                          }`}
                        >
                          {meshConn()!.message}
                        </span>
                      </div>
                    </Show>

                    <div class="settings-subsection">
                      <h4>Hosting</h4>
                      <div class="field">
                        <label class="field-label" for="max-taverns">
                          Max taverns hosted at once
                        </label>
                        <input
                          id="max-taverns"
                          type="number"
                          min="1"
                          max="200"
                          value={maxTaverns()}
                          onInput={(e) =>
                            setMaxTaverns(
                              Math.max(1, Math.min(200, Number(e.currentTarget.value) || 1))
                            )
                          }
                          onChange={() =>
                            api
                              .setMaxHostedTaverns(maxTaverns())
                              .catch((er) => setError(String(er)))
                          }
                        />
                        <div class="field-help">
                          Each tavern you host runs its own local server on a port starting at
                          50052. The default is 16.
                        </div>
                        <Show when={maxTaverns() > 16}>
                          <div class="note note-warn">
                            Hosting more than 16 taverns uses more ports (50052 and up). A high
                            count can collide with other software using those ports and uses more
                            memory and CPU.
                          </div>
                        </Show>
                      </div>
                    </div>
                  </div>
                </Show>

                <Show when={settingsTab() === "nodes"}>
                  <div class="settings-section">
                    <h4>Rendezvous node</h4>
                    <p class="field-help" style={{ "margin-bottom": "12px" }}>
                      Relays DMs for offline delivery and reaches peers behind NAT. Routing
                      through it arrives with the presence step; this saves your choice and
                      shows the tradeoff.
                    </p>
                    <Show
                      when={rendezvous()}
                      fallback={
                        <>
                          <div class="field">
                            <label class="field-label" for="rdv-url">
                              Node URL
                            </label>
                            <input
                              id="rdv-url"
                              value={rdvUrl()}
                              onInput={(e) => setRdvUrl(e.currentTarget.value)}
                              placeholder="https://node.example:50051"
                            />
                          </div>
                          <div class="field">
                            <label class="field-label" for="rdv-label">
                              Label
                            </label>
                            <input
                              id="rdv-label"
                              value={rdvLabel()}
                              onInput={(e) => setRdvLabel(e.currentTarget.value)}
                              placeholder="e.g. My VPS"
                            />
                          </div>
                          <label class="check">
                            <input
                              type="checkbox"
                              checked={rdvMine()}
                              onChange={(e) => setRdvMine(e.currentTarget.checked)}
                            />
                            This is my own node (trusted)
                          </label>
                          <Show when={!rdvMine()}>
                            <div class="note note-warn">
                              A public node's operator may log metadata (who you message and
                              when) - never message content. Run your own node to avoid this.
                            </div>
                          </Show>
                          <div class="actions">
                            <button onClick={saveRendezvous}>Save node</button>
                          </div>
                        </>
                      }
                    >
                      <p>
                        Using <b>{rendezvous()!.label}</b>{" "}
                        {rendezvous()!.mine ? "(your node)" : "(public)"}
                      </p>
                      <Show when={!rendezvous()!.mine}>
                        <div class="note note-warn">
                          This is a public node: its operator may log metadata (who you message
                          and when), never message content.
                        </div>
                      </Show>
                      <div class="actions">
                        <button class="btn-secondary" onClick={clearRendezvous}>
                          Remove node
                        </button>
                      </div>
                    </Show>
                  </div>
                </Show>
              </div>
            </div>

            <div class="modal-footer">
              <button class="btn-secondary" onClick={() => { stopMicTest(); setSettingsOpen(false); }}>
                Close
              </button>
            </div>
          </div>
        </div>
      </Show>

      <Show when={invite()}>
        <div class="modal-backdrop" onClick={() => setInvite(null)}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Invite people</h3>
            <Show when={!invite()!.error} fallback={<div class="error">{invite()!.error}</div>}>
              <p class="field-help" style={{ "margin-bottom": "8px" }}>
                Share this key. Anyone can paste it into <b>Add a server</b> to connect - no
                setup needed. Regenerating mints a new key.
              </p>
              <textarea class="invite-input" readOnly value={invite()!.key} rows={3} />
              <div class="actions">
                <button class="btn-sm" onClick={() => navigator.clipboard.writeText(invite()!.key)}>
                  Copy
                </button>
                <button class="btn-secondary btn-sm" onClick={showInvite}>
                  Regenerate
                </button>
              </div>
            </Show>
            <div class="modal-footer">
              <button class="btn-secondary" onClick={() => setInvite(null)}>
                Close
              </button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}

/** Modal to join another server by invite key, or connect to one by URL. */
function AddServerModal(props: {
  defaultUsername: string;
  onClose: () => void;
  onJoin: (s: ServerSession, registerFirst: boolean, inviteToken?: string) => Promise<void>;
  onCreatePrivate: (name: string) => Promise<void>;
}) {
  // Step 1 is a Create-vs-Join choice; we never drop the user straight into Join.
  const [step, setStep] = createSignal<"choice" | "create" | "join">("choice");
  const [createKind, setCreateKind] = createSignal<"private" | null>(null);
  const [tavernName, setTavernName] = createSignal("");
  const [tab, setTab] = createSignal<"key" | "url">("key");
  const [inviteKey, setInviteKey] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [username, setUsername] = createSignal(props.defaultUsername);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const createPrivate = () =>
    run(async () => {
      const name = tavernName().trim();
      if (!name) {
        setError("Enter a tavern name.");
        return;
      }
      await props.onCreatePrivate(name);
    });

  const joinByKey = () =>
    run(async () => {
      const info = await api.decodeInvite(inviteKey().trim());
      if (info.transport === "mesh") await api.prepareMesh(info.peers);
      await props.onJoin(
        {
          id: crypto.randomUUID(),
          name: info.name || "Tavern",
          endpoint: info.endpoint,
          cert: info.cert,
          username: username(),
          password: "", // taverns use key auth (no password)
        },
        true,
        info.token
      );
    });

  const connectByUrl = () =>
    run(async () => {
      await props.onJoin(
        {
          id: crypto.randomUUID(),
          name: url(),
          endpoint: url().trim(),
          cert: null,
          username: username(),
          password: "", // taverns use key auth (no password)
        },
        true
      );
    });

  return (
    <div class="modal-backdrop" onClick={props.onClose}>
      <div class="modal" onClick={(e) => e.stopPropagation()}>
        <Switch>
          {/* Step 1: choose to create or join. */}
          <Match when={step() === "choice"}>
            <h3>Add a tavern</h3>
            <div class="choice-cards">
              <button class="choice-card" onClick={() => setStep("create")}>
                <span class="choice-card-icon">
                  <Fa icon={faPlus} />
                </span>
                <span class="choice-card-title">Create a tavern</span>
                <span class="choice-card-sub">Host your own server</span>
              </button>
              <button class="choice-card" onClick={() => setStep("join")}>
                <span class="choice-card-icon">
                  <Fa icon={faRightToBracket} />
                </span>
                <span class="choice-card-title">Join a tavern</span>
                <span class="choice-card-sub">Use an invite key or URL</span>
              </button>
            </div>
            <div class="modal-footer">
              <button class="btn-secondary" onClick={props.onClose}>
                Cancel
              </button>
            </div>
          </Match>

          {/* Step 2a: create - pick private (real) or public (scaffold). */}
          <Match when={step() === "create"}>
            <h3>Create a tavern</h3>
            <div class="choice-cards">
              <button
                class={`choice-card ${createKind() === "private" ? "active" : ""}`}
                onClick={() => setCreateKind("private")}
              >
                <span class="choice-card-icon">
                  <Fa icon={faLock} />
                </span>
                <span class="choice-card-title">Private tavern</span>
                <span class="choice-card-sub">Invite-only; you host it</span>
              </button>
              <button class="choice-card disabled" disabled title="Coming soon">
                <span class="choice-card-badge">Soon</span>
                <span class="choice-card-icon">
                  <Fa icon={faGlobe} />
                </span>
                <span class="choice-card-title">Public tavern</span>
                <span class="choice-card-sub">Needs public hosting nodes (not available yet)</span>
              </button>
            </div>

            <Show when={createKind() === "private"}>
              <div class="field">
                <label class="field-label">Tavern name</label>
                <input
                  autofocus
                  value={tavernName()}
                  onInput={(e) => setTavernName(e.currentTarget.value)}
                  placeholder="My Tavern"
                />
              </div>
            </Show>

            <Show when={error()}>
              <div class="error">{error()}</div>
            </Show>
            <div class="modal-footer">
              <button class="btn-secondary" onClick={() => setStep("choice")}>
                Back
              </button>
              <button
                disabled={busy() || createKind() !== "private" || !tavernName().trim()}
                onClick={createPrivate}
              >
                {busy() ? "Creating..." : "Create tavern"}
              </button>
            </div>
          </Match>

          {/* Step 2b: join by invite key or URL. */}
          <Match when={step() === "join"}>
            <h3>Join a tavern</h3>
            <div class="tabs">
              <button class={tab() === "key" ? "tab active" : "tab"} onClick={() => setTab("key")}>
                Invite key
              </button>
              <button class={tab() === "url" ? "tab active" : "tab"} onClick={() => setTab("url")}>
                Tavern URL
              </button>
            </div>

            <Show when={tab() === "key"}>
              <textarea
                class="invite-input"
                value={inviteKey()}
                onInput={(e) => setInviteKey(e.currentTarget.value)}
                placeholder="Paste an invite key (accord1:...)"
                rows={3}
              />
            </Show>
            <Show when={tab() === "url"}>
              <div class="field">
                <label class="field-label">Tavern URL</label>
                <input
                  value={url()}
                  onInput={(e) => setUrl(e.currentTarget.value)}
                  placeholder="http://host:50051"
                />
              </div>
            </Show>

            <div class="field">
              <label class="field-label">Username</label>
              <input value={username()} onInput={(e) => setUsername(e.currentTarget.value)} />
              <div class="field-help">
                No password - you join with your identity key. Defaults to your name; change it
                to use a different name on this tavern.
              </div>
            </div>

            <Show when={error()}>
              <div class="error">{error()}</div>
            </Show>
            <div class="modal-footer">
              <button class="btn-secondary" onClick={() => setStep("choice")}>
                Back
              </button>
              <button
                disabled={busy()}
                onClick={() => (tab() === "key" ? joinByKey() : connectByUrl())}
              >
                {busy() ? "Connecting..." : "Join tavern"}
              </button>
            </div>
          </Match>
        </Switch>
      </div>
    </div>
  );
}

/** Short, readable form of a UUID for display. */
function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}
