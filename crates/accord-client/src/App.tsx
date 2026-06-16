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
  faComments,
  faDesktop,
  faGear,
  faGlobe,
  faHashtag,
  faLock,
  faMicrophone,
  faMicrophoneSlash,
  faPhoneSlash,
  faPlus,
  faRightToBracket,
  faTrash,
  faTriangleExclamation,
  faUserGroup,
  faVideo,
  faVolumeHigh,
  faUserPlus,
} from "@fortawesome/free-solid-svg-icons";
import * as api from "./api";
import type { GroupDto } from "./api";
import * as voice from "./voice";

/** A server the user is signed in to (their home, or one they joined). */
interface ServerSession {
  id: string;
  name: string;
  endpoint: string;
  cert: string | null;
  username: string;
  password: string;
}

export default function App() {
  // Dev tooling lives in the native Dev menu (debug builds only) - there is
  // deliberately no in-app dev banner.
  const [session, setSession] = createSignal<ServerSession | null>(null);
  return (
    <Show when={session()} fallback={<AuthScreen onAuthed={setSession} />}>
      <Home home={session()!} />
    </Show>
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
                    <span class="pill-avatar">{(a.username[0] ?? "?").toUpperCase()}</span>
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
  const [dmName, setDmName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [invite, setInvite] = createSignal<{ key: string; error: string } | null>(null);
  const [encryptAtRest, setEncryptAtRest] = createSignal(false);
  const [friendPolicy, setFriendPolicy] = createSignal<api.FriendRequestPolicy>("everyone");
  const [mesh, setMesh] = createSignal<api.MeshStatus | null>(null);
  const [settingsTab, setSettingsTab] = createSignal<"privacy" | "friends" | "network" | "nodes">(
    "privacy"
  );
  const [yggMode, setYggMode] = createSignal<api.YggPeerMode>("public");
  const [yggPeersText, setYggPeersText] = createSignal("");
  const [meshConn, setMeshConn] = createSignal<api.MeshConnectStatus | null>(null);
  const [rendezvous, setRendezvous] = createSignal<api.RendezvousNode | null>(null);
  const [rdvUrl, setRdvUrl] = createSignal("");
  const [rdvLabel, setRdvLabel] = createSignal("");
  const [rdvMine, setRdvMine] = createSignal(true);
  const [maxTaverns, setMaxTaverns] = createSignal(16);
  const [connected, setConnected] = createSignal(true);
  let wasConnected = true;
  let bottomRef: HTMLDivElement | undefined;

  // --- taverns: permissions, identity, members, voice, mod alerts ---
  const [myPerms, setMyPerms] = createSignal<api.MyPerms | null>(null);
  const [tavern, setTavern] = createSignal<api.TavernDto | null>(null);
  const [members, setMembers] = createSignal<api.MemberDto[]>([]);
  const [showMembers, setShowMembers] = createSignal(false);
  const [createChannelOpen, setCreateChannelOpen] = createSignal(false);
  const [newChannelName, setNewChannelName] = createSignal("");
  const [newChannelKind, setNewChannelKind] = createSignal<"text" | "voice">("text");
  // group_id -> participants currently in that voice channel.
  const [voiceParticipants, setVoiceParticipants] = createSignal<
    Record<string, api.VoiceParticipant[]>
  >({});
  const [activeVoice, setActiveVoice] = createSignal<string | null>(null);
  const [localVoice, setLocalVoice] = createSignal(voice.initialVoiceState());
  const [modAlerts, setModAlerts] = createSignal<api.ModAlert[]>([]);

  const can = (bit: bigint) => api.can(myPerms(), bit);

  const refreshGroups = () => api.listGroups().then(setGroups);
  const refreshPerms = () =>
    api.getMyPermissions().then(setMyPerms).catch(() => setMyPerms(null));
  const refreshTavern = () => api.getTavern().then(setTavern).catch(() => {});
  const refreshMembers = (groupId: string | null) => {
    if (!groupId) return setMembers([]);
    api.listMembers(groupId).then(setMembers).catch(() => setMembers([]));
  };

  const textChannels = () =>
    publicGroups().filter((g) => g.channelKind !== "voice");
  const voiceChannels = () =>
    publicGroups().filter((g) => g.channelKind === "voice");

  async function loadGroupsAndSelect() {
    const gs = await api.listGroups();
    setGroups(gs);
    setActiveId(gs.length > 0 ? gs[0].id : null);
    // Tavern context for the active server (best-effort; ignored on failure).
    void refreshPerms();
    void refreshTavern();
  }

  // --- channel create / delete ---
  async function submitCreateChannel(e: Event) {
    e.preventDefault();
    const name = newChannelName().trim();
    if (!name) return;
    try {
      await api.createChannel(name, newChannelKind());
      setNewChannelName("");
      setCreateChannelOpen(false);
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

  // --- voice channel join/leave + toggles (media stubbed in voice.ts) ---
  async function joinVoiceChannel(groupId: string) {
    try {
      if (activeVoice() && activeVoice() !== groupId) {
        await voice.leave(activeVoice()!);
      }
      await voice.join(groupId);
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
  }

  const toggleMute = async () => {
    const s = localVoice();
    await voice.setMuted(s, !s.muted);
    setLocalVoice({ ...s, muted: !s.muted });
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

  const participantsFor = (groupId: string) => voiceParticipants()[groupId] ?? [];

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
        content: m.content,
        timestampMs: m.timestampMs,
      }))
    );
  }

  createEffect(() => {
    activeId();
    loadHistory();
  });

  createEffect(() => {
    messages();
    bottomRef?.scrollIntoView({ behavior: "smooth" });
  });

  const activeGroup = () => groups().find((g) => g.id === activeId());
  const isPrivate = () => activeGroup()?.kind === "private";
  const publicGroups = () => groups().filter((g) => g.kind !== "private");
  const privateGroups = () => groups().filter((g) => g.kind === "private");

  // Refresh the member list when the member panel is open and the active public
  // channel changes (members are server-scoped; any public channel works).
  createEffect(() => {
    const id = activeId();
    if (showMembers() && view() === "server" && id && !isPrivate()) {
      refreshMembers(id);
    }
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

  /** Join (or connect to) another tavern; it stays connected in the background. */
  async function addServer(s: ServerSession, registerFirst: boolean, inviteToken?: string) {
    await api.connect(s.id, s.endpoint, s.cert ?? undefined);
    if (registerFirst) {
      try {
        await api.register(s.username, s.password, s.username, inviteToken);
      } catch {
        /* already registered - fall through to login */
      }
    }
    await api.login(s.username, s.password, "Desktop");
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
        password: props.home.password,
      },
      true
    );
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
        await api.login(props.home.username, props.home.password, "Desktop");
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

  async function startDm(e: Event) {
    e.preventDefault();
    const name = dmName().trim();
    if (!name) return;
    setDmName("");
    setError(null);
    try {
      const group = await api.startDm(name);
      await refreshGroups();
      setActiveId(group.id);
    } catch (err) {
      setError(String(err));
    }
  }

  const showInvite = () =>
    api
      .createInviteKey(activeServerId())
      .then((key) => setInvite({ key, error: "" }))
      .catch((e) => setInvite({ key: "", error: String(e) }));

  const serverGlyph = (s: ServerSession) => s.name.slice(0, 2).toUpperCase();
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
            <button class="icon-btn ghost" title="Add friends to this DM (group DM)" disabled>
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
                <div class="contact-row">
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
                  <div class="contact-actions">
                    <button class="btn-sm" disabled={isBlocked(c.id)} onClick={() => openDm(c)}>
                      Message
                    </button>
                    <button class="btn-secondary btn-sm" onClick={() => toggleBlocked(c)}>
                      {isBlocked(c.id) ? "Unblock" : "Block"}
                    </button>
                  </div>
                </div>
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
                {(m) => (
                  <Show
                    when={!m.pending}
                    fallback={
                      <div class="message pending" aria-busy="true">
                        <div class="glint glint-author" />
                        <div class="glint glint-body" />
                      </div>
                    }
                  >
                    <div class="message">
                      <span class="author">{m.author}</span>
                      <span class="time">{new Date(m.timestampMs).toLocaleTimeString()}</span>
                      <div class="body">{m.content}</div>
                    </div>
                  </Show>
                )}
              </For>
              <div ref={bottomRef} />
            </div>
            <form class="composer" onSubmit={send}>
              <input
                value={draft()}
                onInput={(e) => setDraft(e.currentTarget.value)}
                placeholder={`Message ${activeConv()?.peerName ?? ""}`}
              />
              <button>Send</button>
            </form>
          </Show>
        </Match>
      </Switch>
    </main>
  );

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
          <div class="sidebar-scroll">
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
              <div class="sidebar-header sidebar-header-row">
                <span>{tavern()?.name || "Channels"}</span>
                <Show when={can(api.PERM.MANAGE_CHANNELS)}>
                  <button
                    class="channel-add-btn"
                    title="Create channel"
                    onClick={() => setCreateChannelOpen(true)}
                  >
                    <Fa icon={faPlus} />
                  </button>
                </Show>
              </div>
              <For each={textChannels()}>
                {(g) => (
                  <div class={`channel-row ${g.id === activeId() ? "active" : ""}`}>
                    <button class="channel" onClick={() => setActiveId(g.id)}>
                      <span class="hash">
                        <Fa icon={faHashtag} />
                      </span>
                      {g.name}
                    </button>
                    <Show when={can(api.PERM.MANAGE_CHANNELS)}>
                      <button
                        class="channel-del"
                        title="Delete channel"
                        onClick={() => deleteChannel(g.id)}
                      >
                        <Fa icon={faTrash} />
                      </button>
                    </Show>
                  </div>
                )}
              </For>

              <Show when={voiceChannels().length > 0}>
                <div class="sidebar-header">Voice channels</div>
              </Show>
              <For each={voiceChannels()}>
                {(g) => (
                  <>
                    <div class={`channel-row ${activeVoice() === g.id ? "active" : ""}`}>
                      <button
                        class="channel voice-channel"
                        onClick={() => joinVoiceChannel(g.id)}
                      >
                        <span class="hash">
                          <Fa icon={faVolumeHigh} />
                        </span>
                        {g.name}
                      </button>
                      <Show when={can(api.PERM.MANAGE_CHANNELS)}>
                        <button
                          class="channel-del"
                          title="Delete channel"
                          onClick={() => deleteChannel(g.id)}
                        >
                          <Fa icon={faTrash} />
                        </button>
                      </Show>
                    </div>
                    <For each={participantsFor(g.id)}>
                      {(p) => (
                        <div class="voice-participant">
                          <span class="vp-name">{p.userId.slice(0, 8)}</span>
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
                        </div>
                      )}
                    </For>
                  </>
                )}
              </For>

              <Show when={activeVoice()}>
                <div class="voice-bar">
                  <span class="voice-bar-label">
                    <Fa icon={faVolumeHigh} /> Voice connected
                  </span>
                  <div class="voice-toggles">
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
                    <button
                      class="voice-toggle danger"
                      title="Disconnect"
                      onClick={leaveVoiceChannel}
                    >
                      <Fa icon={faPhoneSlash} />
                    </button>
                  </div>
                </div>
              </Show>

              <div class="sidebar-header">Direct messages</div>
              <For each={privateGroups()}>
                {(g) => (
                  <button
                    class={`channel ${g.id === activeId() ? "active" : ""}`}
                    onClick={() => setActiveId(g.id)}
                  >
                    <span class="hash" />
                    {g.name}
                  </button>
                )}
              </For>
              <form class="dm-form" onSubmit={startDm}>
                <input
                  value={dmName()}
                  onInput={(e) => setDmName(e.currentTarget.value)}
                  placeholder="username..."
                />
                <button title="Start encrypted DM">+ DM</button>
              </form>
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

          <div class="user-card">
            <div class="user-avatar">{(props.home.username[0] ?? "?").toUpperCase()}</div>
            <div class="user-info">
              <span class="user-name" title={props.home.username}>
                {props.home.username}
              </span>
              <span class="user-status">Online</span>
            </div>
            <button class="user-gear" title="Settings" onClick={() => setSettingsOpen(true)}>
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
              <For each={messages()}>
                {(m) => (
                  <Show
                    when={!m.pending}
                    fallback={
                      <div class="message pending" aria-busy="true">
                        <div class="glint glint-author" />
                        <div class="glint glint-body" />
                      </div>
                    }
                  >
                    <div class="message">
                      <span class="author">{m.author}</span>
                      <span class="time">{new Date(m.timestampMs).toLocaleTimeString()}</span>
                      <div class="body">{m.content}</div>
                    </div>
                  </Show>
                )}
              </For>
              <div ref={bottomRef} />
            </div>

            <form class="composer" onSubmit={send}>
              <input
                value={draft()}
                onInput={(e) => setDraft(e.currentTarget.value)}
                placeholder={
                  activeGroup()
                    ? `Message ${isPrivate() ? " " : "#"}${activeGroup()!.name}`
                    : "Select a channel"
                }
                disabled={!activeId()}
              />
              <button disabled={!activeId()}>Send</button>
            </form>
          </main>
        </Show>

        <Show when={view() === "server" && showMembers() && !isPrivate()}>
          <aside class="members-panel">
            <div class="sidebar-header">Members — {members().length}</div>
            <For each={members()} fallback={<div class="sidebar-empty">No members.</div>}>
              {(m) => (
                <div class="member-row">
                  <span class={`member-dot ${m.online ? "online" : ""}`} />
                  <span class="member-avatar">
                    {(m.displayName[0] ?? "?").toUpperCase()}
                  </span>
                  <span class="member-name" title={m.username}>
                    {m.displayName}
                  </span>
                  <Show when={m.isOwner}>
                    <span class="role-badge owner">owner</span>
                  </Show>
                  <span class="member-actions">
                    <Show when={can(api.PERM.KICK_MEMBERS) && !m.isOwner}>
                      <button
                        class="member-action"
                        title="Kick from channel"
                        onClick={() => activeId() && api.kickMember(activeId()!, m.userId).then(() => refreshMembers(activeId())).catch((e) => setError(String(e)))}
                      >
                        kick
                      </button>
                    </Show>
                    <Show when={can(api.PERM.BAN_MEMBERS) && !m.isOwner}>
                      <button
                        class="member-action danger"
                        title="Ban from server"
                        onClick={() => api.banMember(m.userId).then(() => refreshMembers(activeId())).catch((e) => setError(String(e)))}
                      >
                        ban
                      </button>
                    </Show>
                  </span>
                </div>
              )}
            </For>
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
                  {a.action} on {a.target || "server"} — {a.reason}
                </span>
              </div>
            )}
          </For>
          <button class="mod-alert-clear" onClick={() => setModAlerts([])}>
            dismiss
          </button>
        </div>
      </Show>

      <Show when={createChannelOpen()}>
        <div class="modal-backdrop" onClick={() => setCreateChannelOpen(false)}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Create channel</h3>
            <form onSubmit={submitCreateChannel}>
              <div class="field">
                <label class="field-label">Channel name</label>
                <input
                  value={newChannelName()}
                  onInput={(e) => setNewChannelName(e.currentTarget.value)}
                  placeholder="new-channel"
                  autofocus
                />
              </div>
              <div class="field">
                <label class="field-label">Type</label>
                <div class="channel-kind-pick">
                  <button
                    type="button"
                    class={`btn-secondary ${newChannelKind() === "text" ? "active" : ""}`}
                    onClick={() => setNewChannelKind("text")}
                  >
                    <Fa icon={faHashtag} /> Text
                  </button>
                  <button
                    type="button"
                    class={`btn-secondary ${newChannelKind() === "voice" ? "active" : ""}`}
                    onClick={() => setNewChannelKind("voice")}
                  >
                    <Fa icon={faVolumeHigh} /> Voice
                  </button>
                </div>
              </div>
              <div class="modal-footer">
                <button type="button" class="btn-secondary" onClick={() => setCreateChannelOpen(false)}>
                  Cancel
                </button>
                <button type="submit" disabled={!newChannelName().trim()}>
                  Create
                </button>
              </div>
            </form>
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
        <div class="modal-backdrop" onClick={() => setSettingsOpen(false)}>
          <div class="modal settings-modal" onClick={(e) => e.stopPropagation()}>
            <h3>Settings</h3>

            <div class="settings-body">
              <nav class="settings-nav">
                <For
                  each={
                    [
                      ["privacy", "Privacy"],
                      ["friends", "Friends"],
                      ["network", "Yggdrasil"],
                      ["nodes", "Nodes"],
                    ] as const
                  }
                >
                  {([key, label]) => (
                    <button
                      class={`settings-nav-item ${settingsTab() === key ? "active" : ""}`}
                      onClick={() => setSettingsTab(key)}
                    >
                      {label}
                    </button>
                  )}
                </For>
              </nav>

              <div class="settings-content">
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
              <button class="btn-secondary" onClick={() => setSettingsOpen(false)}>
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
  const [password, setPassword] = createSignal("");
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
          password: password(),
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
          password: password(),
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

          {/* Step 2a: create — pick private (real) or public (scaffold). */}
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
