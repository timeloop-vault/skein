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
                                          │ Runtime: Host | Ssh/tmux |  │
                                          │          sandbox            │
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
approval queue and notifier, and a Runtime trait (Host | Ssh/tmux |
sandbox).

**Transport.** One multiplexed connection per daemon: an in-process
channel, a local socket, or a WebSocket over a private overlay network
(e.g. Tailscale).

**Key property.** Harnesses always talk to a daemon next to them, so no
reverse tunnels are needed.

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

0. **ssh + tmux spike.** Built separately, in another room.
1. **Extract skein-daemon** with the in-process transport and zero
   behaviour change. This is the bulk of the work.
2. **Detached local.** Socket, a daemon that outlives the app, seq
   resume plus terminal snapshots, and a Settings choice of attached vs
   detached.
3. **Remote.** WebSocket over the overlay network, device tokens, a host
   picker at room creation, `list_workspaces`.
4. **Approval queue and push notifier.**
5. **Sandbox runtime.**
6. **Mobile client.** Feed, approvals, `send_message`, and a read-only
   terminal peek.

## Open questions

- Daemon discovery and config UX: a manual list, MagicDNS, or mDNS?
- Review state for a room whose daemon is offline: a read-only cache, or
  hide it?
- Should detached become the default?
- The updater for a remote daemon, and the client/daemon version-skew
  policy.
- Claude Code auth on a remote host (subscription vs API key). This
  affects `spawn_env`.

## Corrections from recon

Verified against commit `4d785fe`. Where this design and the recon
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
