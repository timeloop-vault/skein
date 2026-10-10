# skeind — running harnesses away from the UI

Status: design draft (not yet implemented). Recon: docs/skeind-recon.md.
Issue drafts: docs/skeind-issues-draft.md.

CLAUDE.md's standing decisions still hold. In particular, Skein performs
no git mutations in any mode: a daemon, a remote host or a sandbox does
not change that.

## Goal

Harnesses run somewhere other than the machine showing the UI, and keep
running with no client attached. Later, desktop and mobile clients
attach to the same rooms.

## Modes

Three modes, one code path. Only the lifecycle differs:

- **Attached local** — today's behaviour. The daemon lives and dies with
  the app.
- **Detached local** — the daemon survives the app quitting; the app
  reattaches on the next launch.
- **Remote** — an always-on home server or VM. Clients attach over the
  network.

A client may connect to several daemons. A room belongs to exactly one
daemon.

## Non-goals

- Harnesses surviving a daemon crash. Durability comes from session
  resume, not from PTY survival.
- Multi-user shared rooms.
- Changing CLAUDE.md's standing decisions (Skein still performs no git
  mutations in any mode).
- Building VM or sandbox infrastructure. That is a separate project.

## Why it's a split

The UI and the harnesses currently share a machine through five
couplings:

1. PTYs are spawned in-process.
2. Transcript tailing uses local filesystem watchers (Claude JSONL),
   plus opencode events.
3. The agent API and MCP endpoint listen on 127.0.0.1, and harness hooks
   post back to it.
4. The worktree watcher, git reads, the files harness and the review
   surface all read local disk.
5. The sqlite state lives beside the app.

All of these move to skeind, and the Tauri app becomes a client. The
recon refines this list; where it differs, the recon wins (see
"Corrections from recon" at the end).

## Architecture

    client (desktop / mobile)              skeind
    ┌──────────────────────────┐          ┌─────────────────────────────┐
    │ UI, xterm.js, CodeMirror │          │ rooms + sqlite (authority)  │
    │ review UI                │          │ harness supervisor          │
    │ connection manager:      │  one     │ PTY host + headless emulator│
    │  N daemons, reconnect,   │ multi-   │ transcript tails + parsers  │
    │  last-seen seq           │ plexed   │ watcher, skein-git reads    │
    │                          │<-------->│ files backend               │
    └──────────────────────────┘ conn per │ agent API + MCP (localhost) │
                                 daemon   │ approval queue + notifier   │
                                          │ Runtime: Host | sandbox     │
                                          │                             │
                                          └──────────────┬──────────────┘
                                                         │ localhost
                                                    harnesses (PTYs)

**Client.** The UI, xterm.js, CodeMirror, the review UI, and a
connection manager for N daemons with reconnect and the last-seen seq
per daemon.

**skeind.** Rooms and sqlite (authoritative), the harness supervisor,
the PTY host with a headless terminal emulator per PTY, transcript tails
and parsers, the watcher, skein-git reads, the files backend, the agent
API and MCP (still localhost from the harness's point of view), the
approval queue and notifier, and a Runtime trait (Host | sandbox).

**Why skeind does not use ssh + tmux.** skeind on the host already gives remote
harnesses: it spawns ordinary local PTYs (HostRuntime). PTY survival
across a daemon restart is a non-goal; durability is resume plus terminal
snapshots. And a harness in tmux reached over ssh from the UI machine
cannot reach the agent API and MCP on its 127.0.0.1 without a reverse
tunnel, which this design avoids. The spike itself shipped separately
as the `remote` harness kind (#568), and its findings fed this design;
they are listed under "Evidence from the ssh + tmux spike (#568)"
below. The `remote` kind stays a separate, simpler path for plain
remote ssh use: skeind neither replaces nor depends on it, and the two
can coexist. A `remote`-kind harness keeps its own limits, e.g. no
agent-API reachability unless the remote host can reach it.

**Transport.** One multiplexed connection per daemon: an in-process
channel, a local socket, or a WebSocket over a private overlay network
(e.g. Tailscale).

**Key property.** Harnesses always talk to a daemon next to them, so no
reverse tunnels are needed.

### Evidence from the ssh + tmux spike (#568)

- Keeping a process alive is easy; the value is telemetry. Transcripts
  and SSE live on the remote host, so skeind tails them there and ships
  events, not just bytes (transcript tails in skeind).
- The remote env breaks first: a non-login ssh command found no tmux or
  opencode, and claude was on no PATH. The spawn env probe runs on the
  daemon's host (#565 login-shell capture).
- tmux adds a resize hop and visible jank. A daemon-owned PTY removes it
  (Terminal reattach, headless emulator, last-active-client sizing).
- Review needs the worktree where the agent edits, so a remote room's
  worktree, watcher and git reads live on the daemon's host (the
  Room/workspace contract).
- Review tools, mail and nudges need the agent API reachable from the
  harness: a harness talks to the daemon next to it (Harness to skeind
  communication).

## Harness ↔ skeind communication

All harness traffic to Skein goes to the agent API, which lives in
skeind: MCP at `/mcp`, the plain-JSON `/api/*` mirror, and the three
Claude hooks (`curl` entries in the #215 plugin bundle's `hooks.json`,
posting to `/api/harness/{permission,session-start,session-end}`).
opencode reaches the same server through its MCP config. The harness
sees it on 127.0.0.1 because skeind runs on the harness's host.

skeind mints and injects `SKEIN_REVIEW_URL`, `SKEIN_REVIEW_TOKEN`,
`SKEIN_ROOM_ID` and `SKEIN_HARNESS_ID`, and ships its own harness-config
bundle (`--plugin-dir` / `OPENCODE_CONFIG`), because today's paths point
into the app's resource dir.

Remote mode therefore means "skeind on the harness host". A runtime
where the PTY is not on skeind's host (a sandbox that does not share
skeind's loopback) must itself provide reachability of the agent API URL: a forwarded port or an
explicit egress rule. That is the runtime's job, behind the `Runtime`
trait, and never a reverse tunnel to a client.

## Cross-daemon routing

**Problem.** The agent API's cross-room verbs assume one install and one
sqlite:

- `send_message`, `read_messages` and `message_history` use one
  `harness_messages` table, and a room id resolves to its lead harness
  at send time.
- `create_room` stamps `createdBy`.
- `close_room` is creator-only and needs the target's sign-off for its
  HEAD.
- `open_harness` and `close_harness` act on your own room or rooms you
  created.
- `list_rooms`, `get_room`, `list_harnesses` and `find_rooms_for_path`
  are install-wide.

With rooms on several daemons (say a director room local and worker rooms
remote), these must route between daemons.

**Design.** A daemon-to-daemon link. The harness side is unchanged: a
harness only ever talks to its own daemon, and the daemon routes. The
local daemon dials out to each remote daemon, as the client does, and the
link carries traffic both ways. A remote daemon never dials in to a
laptop, which keeps "no reverse tunnels". The link reuses the Phase 3
transport and the per-device token model, the local daemon being one more
device. Its peer credential is scoped to the routed agent-API verbs and
never to sign-off.

**Availability.** The local daemon is available only while its machine
is up (in attached mode it dies with the app, and laptops sleep); the
remote daemon is the always-on one. Routing is therefore store-and-forward:
mail to a room whose daemon is unreachable queues on the sender's daemon
and is delivered on reconnect (the mailbox is already a queue). Verbs
that cannot wait (`create_room`, `close_room`, `open_harness`,
`close_harness`) fail with an explicit "daemon offline" error rather
than queue.

**Identity and authority.** Room and harness ids are qualified by daemon
id, and `createdBy` names a room on a specific daemon. The owning daemon
is authoritative for its rooms' mailbox, sign-off and creator checks; a
forwarded `close_room` is checked there against the qualified
`createdBy`. The listing verbs (`list_rooms`, `find_rooms_for_path`,
`get_room`, `list_harnesses`) aggregate across reachable peers and flag
each peer's results as live or stale/offline.

**Scope line.** The client still connects to every daemon directly for
PTYs, review and the event stream. Daemon-to-daemon carries routed
agent-API traffic only, never terminal bytes.

**Rejected alternative.** The client relays between daemons. It is
cheaper, but it fails exactly when it matters: the director's machine is
closed and a remote worker sends mail.

## Crates

- `skein-daemon` — no Tauri dependency.
- `skein-proto` — versioned wire types.

`app/src-tauri` becomes a thin client; in attached mode it embeds the
daemon with an in-memory transport. Extract behind the transport trait
with the in-process transport first, prove no regression, then add real
transports.

## Protocol

1. **Event stream.** A monotonic per-daemon seq covers every state
   change. Reconnect with `since=<seq>`, or receive a snapshot plus tail
   if the gap is too old.
2. **PTY channel.** Bytes, input, resize, attach/detach.

Requests map 1:1 onto existing agent API verbs where possible (see
`docs/agent-api.md`).

Versioned from day one:
`hello { proto_version, client_kind, capabilities }`.

## Auth

- In-process: none.
- Local socket: filesystem permissions.
- Remote: the overlay network provides reachability, plus per-device
  revocable tokens issued by the daemon. Never rely on the network
  alone.

## Terminal reattach

Do not replay a raw byte ring into a fresh xterm.js: Claude Code's TUI
renders wrong that way, and there is history of xterm fragility here.

Instead, run a headless emulator per PTY (e.g. alacritty_terminal or
vt100). On attach, send a screen and scrollback snapshot, then live
bytes.

Resize: the last active client sets the size, not smallest-wins, and the
size is re-asserted on reactivation.

## Approvals while unattended

The permission hook (the #215 plugin bundle's `PermissionRequest` hook)
goes to a daemon approval record, visible in the event stream, and the
hook blocks. A notifier pushes it; start with a self-hosted push service
(e.g. ntfy), with APNs/FCM later.

Any client can answer, and the first answer wins.

Optional per-room timeout policy: keep waiting (default), auto-deny
after N minutes, or notify again.

## Durability

The daemon owns which harnesses should run and their `sessionId`. On
start it re-spawns them with resume (as `resume.rs` does today) and
marks the resume in the event stream. In-flight PTY state is lost on
restart, and that is accepted.

## Runtime trait

Methods: `ensure_room_env`, `spawn_pty`, `watch_paths`, `teardown`.

`HostRuntime` comes first. A sandbox runtime follows, with these
constraints:

- One sandbox per room, since harnesses share a worktree.
- The daemon runs outside the sandbox and execs PTYs into it.
- The worktree and the harness state dir must be readable by the daemon.
- Egress must reach the agent API.

The candidate sandbox (e.g. OpenShell) is at 0.1.0, so keep it strictly
behind the trait.

## Room/workspace contract

A remote room is created against a main checkout that already exists on
the host. Provisioning it is out of scope; separate workspace tooling
owns that.

`list_workspaces` returns the main checkouts under a configured root
(`SKEIND_WORKSPACE_ROOT`). An optional later `create_workspace` shells
out. Skein stays free of clone and credential logic.

## Phasing

Each phase is usable on its own.

0. **ssh + tmux spike.** Ran as #568 and shipped standalone as the
   `remote` harness kind; its findings fed this design. skeind itself
   uses neither ssh nor tmux.
1. **Extract skein-daemon** with the in-process transport and zero
   behaviour change. This is the bulk of the work.
2. **Detached local.** Socket, a daemon that outlives the app, seq
   resume plus terminal snapshots, and a Settings choice of attached vs
   detached.
3. **Remote.** WebSocket over the overlay network, device tokens, a host
   picker at room creation, `list_workspaces`, and the daemon-to-daemon
   link for cross-daemon routing (see "Cross-daemon routing").
4. **Approval queue and push notifier.**
5. **Sandbox runtime.**
6. **Mobile client.** Feed, approvals, `send_message`, and a read-only
   terminal peek.

## Open questions

- Daemon discovery and config UX: a manual list, MagicDNS, or mDNS?
- Cross-daemon routing: id qualification and migration of existing room
  ids, store-and-forward semantics (ordering, dedupe, expiry), and peer
  trust bootstrap. Detail in `docs/skeind-recon.md` §8.
- Review state for a room whose daemon is offline: a read-only cache, or
  hide it?
- Should detached become the default?
- The updater for a remote daemon, and the client/daemon version-skew
  policy.
- Claude Code auth on a remote host (subscription vs API key). This
  affects `spawn_env`: since #565 a harness inherits the daemon host's
  full login-shell env, so an API key exported there reaches it.

## Corrections from recon

Verified against commit `4d785fe` (spawn-env citations re-verified at
`30cd4b8`). Where this design and the recon
differ, the recon wins. Detail is in `docs/skeind-recon.md` (§7
corrections, §8 new open questions).

1. The five couplings hold but are incomplete: the recon lists 46.
2. Coupling 3 is bidirectional. The agent API calls into the webview
   (`request_frontend`), so `create_room`, `open_harness`,
   `close_harness`, `close_room`, `list_rooms` and the design verbs
   need a client today.
3. The phase machine, nudges, mail delivery and `harness_events` writes
   live in the TypeScript frontend; so do argv building and opencode
   port allocation. An unattended daemon needs them moved or hosted.
4. Hooks are `curl` entries in the plugin's `hooks.json`, not
   `cli_shim.rs`, and run on the harness host.
5. Tokens are revoked on every boot, and `--plugin-dir` /
   `OPENCODE_CONFIG` are host-bound resource paths.
6. `db_save_rooms` is a wholesale mirror, the design preview URL is the
   webview's loopback, and folder pickers return client paths. None of
   these survive a remote daemon unchanged.
7. Worktree create/restore already exist, so "no git mutations" means
   no commit, merge, push or PR. The daemon inherits exactly today's
   set.
8. The headless emulator recommendation is `vt100` (or a fork) behind a
   trait, gated on a replay test against xterm.js.
9. `docs/agent-api.md` counts 22 verbs; there are 25.
10. The login-shell probe now captures the full env (#565), not only
    `PATH`. It runs on the daemon host, so the daemon host's rc-file
    exports (API keys, proxy settings) become every harness's env, and
    loopback is merged into `NO_PROXY` so the local agent API stays
    reachable.
11. The cross-room verbs (mail, `createdBy`, `close_room`, harness
    control, listings) assume one install and one sqlite, so rooms on
    several daemons need routing (see "Cross-daemon routing"; recon §3
    "Cross-room verbs").
