# skeind: draft issues

Status: draft, not filed. Nothing here exists on GitHub yet.

Issue numbers are placeholders (`#E` for the epic, `#P0`, `#P1a` and so on
for the phases). Replace them once the issues are filed, and fix the
checklist in the epic body in the same pass.

Design: `docs/skeind-design.md`. Recon: `docs/skeind-recon.md`, with the
sections "Coupling points", "Protocol surface", "Phase 1 sizing",
"Headless terminal emulator", "Corrections" and "Open questions".
`docs/backlog.md` has nothing on daemons or remote hosts. Its one remote
entry (the release-policy min-version floor) is relevant only to the
version-skew open question below.

Each section holds the title, the labels (all exist in the repo) and the
body in a fenced block, ready for `gh issue create --body-file`.

Sizing is stated in modules, commands and touch points. The counts come
from the recon: 85 Tauri commands in 17 files, 5 streaming Channel sites,
7 event names emitted from modules that move.

## Epic

**Title:** skeind: run harnesses detached from the UI and on remote hosts

**Labels:** `area:rooms`, `area:agent-api`

```markdown
## Goal

Harnesses run somewhere other than the machine showing the UI, and keep
running with no client attached. Later, desktop and mobile clients attach
to the same rooms.

Design: `docs/skeind-design.md`. Recon: `docs/skeind-recon.md` ("Coupling
points", "Protocol surface", "Phase 1 sizing", "Headless terminal
emulator", "Corrections", "Open questions").

## Modes

Three modes, one code path. Only the lifecycle differs.

- **Attached local**: today's behaviour. The daemon (`skeind`) lives and
  dies with the app.
- **Detached local**: the daemon survives the app quitting; the app
  reattaches on the next launch.
- **Remote**: an always-on host. Clients attach over the network.

A client may connect to several daemons. A room belongs to exactly one
daemon.

## Non-goals

- **Skein still performs no git mutations.** No commit, merge, push or PR
  in any mode: a daemon, a remote host or a sandbox changes where the
  agent runs, not who lands branches (D9 on #52). Worktree creation at
  room creation already exists and is not what that decision is about.
- Harnesses surviving a daemon crash. Durability comes from session
  resume, not from PTY survival.
- Multi-user shared rooms.
- Building VM or sandbox infrastructure.
- Provisioning checkouts on a remote host. Separate workspace tooling owns
  that; Skein stays free of clone and credential logic.

## Phases

Each phase is usable on its own.

- [ ] #P0 ssh + tmux spike (owned by another room)
- [ ] #P1a Event-sink trait replaces AppHandle/Emitter in the modules that move
- [ ] #P1b Stream sinks and command/impl split for Channel commands and binary fs reads
- [ ] #P1c Path and resource provider replaces app.path()
- [ ] #P1d skein-proto and skein-daemon crates; move the leaf modules
- [ ] #P1e Move the engines into skein-daemon
- [ ] #P1f Agent API and in-process transport; the app embeds the daemon
- [ ] #P2a Daemon-owned harness lifecycle
- [ ] #P2b Detached local mode
- [ ] #P3 Remote: network transport, device tokens, host picker
- [ ] #P4 Approval queue and push notifier
- [ ] #P5 Sandbox runtime behind the Runtime trait
- [ ] #P6 Mobile client

Phase 1 is the bulk of the code motion and changes no behaviour. Extract
behind the transport trait with the in-process transport first, prove no
regression, then add real transports.

## Standing decisions

- Extraction is in place first (traits inside `app/src-tauri`), then the
  crate move. Every step leaves the app working.
- Crates: `skein-daemon` has no Tauri dependency (checked with
  `cargo tree -p skein-daemon | grep tauri` being empty); `skein-proto`
  holds the versioned wire types.
- One multiplexed connection per daemon: in-process channel, local
  socket, or WebSocket over a private overlay network.
- Two protocol channels: an event stream with a monotonic per-daemon seq
  (reconnect with `since=<seq>`, or snapshot plus tail when the gap is too
  old), and a PTY channel (bytes, input, resize, attach/detach).
- `hello { proto_version, client_kind, capabilities }` from day one.
- Auth: none in-process, filesystem permissions on the local socket,
  per-device revocable tokens for remote. Never rely on the network alone.
- Terminal reattach uses a headless emulator per PTY (recommended: `vt100`
  behind a trait) that sends a screen snapshot, then live bytes. A raw
  byte ring is not replayed into a fresh xterm.js.
- Resize: the last active client sets the size.
- Durability: the daemon owns which harnesses should run and their
  `sessionId`, and re-spawns them with resume on start.
- Harnesses always talk to a daemon next to them, so no reverse tunnels.

## Open questions

Tracked in the phase issue that needs the answer, and in `docs/skeind-recon.md`
("Open questions").

- Daemon discovery and config UX: manual list, MagicDNS, or mDNS? (#P3)
- Review state for a room whose daemon is offline: read-only cache, or
  hide it? (#P3)
- Should detached become the default? (#P2b)
- The updater for a remote daemon, and the client/daemon version-skew
  policy. (#P3)
- Claude Code auth on a remote host (subscription vs API key); affects
  `spawn_env`. (#P3)
- Binary framing on the wire for `read_image_bytes` and similar. (#P1b)
- Which of the phase machine, deferral timers, mail nudges and
  notifications move to the daemon, and in what order. (#P2a)
- Whether tmux-backed PTYs are a `Runtime` implementation. (#P0)

## Out of scope for this epic

Anything the design lists as a non-goal, plus a `create_workspace` verb
(optional, later, would shell out).
```

## Phase 0

**Title:** skeind P0: ssh + tmux spike (record of the question)

**Labels:** (none)

```markdown
Part of #E.

## Why

Before committing to a `Runtime` trait shape, find out whether a harness
PTY can live in a tmux session on a remote host, reached over ssh, and
still give Skein what it needs: byte output, input, resize, and
reattach.

This spike is owned by a separate room that is already in progress. This
issue only records the question it answers and what the later phases need
from the answer. It is not a work order.

## Question

Can a tmux-backed PTY be driven from the daemon as a `Runtime`
implementation, rather than something beside it?

## What later phases need from the answer

- #P1f and #P2b: the `Runtime` trait methods (`ensure_room_env`,
  `spawn_pty`, `watch_paths`, `teardown`) must not assume an in-process
  `portable-pty` master. If tmux-backed PTYs fit as a `Runtime`, the trait
  stays as designed; if not, say which method breaks.
- #P2b: whether the headless emulator is still needed when tmux already
  holds screen state, or whether tmux's own capture replaces it for that
  runtime.
- #P3: whether the remote mode is "daemon on the host" only, or also
  "daemon here, PTYs over ssh". The design assumes the first.
- Transcript tailing and the watcher: the design runs them next to the
  harness (see "Coupling points" in the recon). Record whether the spike
  changes that.

## Acceptance

The spike's findings are written down where #P1f and #P2b can cite them,
and the question and needs above are answered or explicitly left open.
```

## Phase 1a

**Title:** skeind P1a: event-sink trait replaces AppHandle and Emitter in the modules that move

**Labels:** `refactor`, `area:agent-api`

```markdown
Part of #E. Phase 1 (zero behaviour change). Depends on: nothing.

## Why

Server-to-UI pushes are `app.emit(...)` calls on a Tauri `AppHandle`. A
daemon has no `AppHandle`. A trait the moving modules emit into lets the
Tauri app, and later a network transport, supply the other end. This is
the first seam in dependency order and needs no other change.

See "Phase 1 sizing" and "Protocol surface" in `docs/skeind-recon.md`.

## Scope

All paths under `app/src-tauri/src/`. Done in place; nothing moves crates
yet.

- Add an `EventSink` trait (name open) and a Tauri implementation that
  emits the same event names as today.
- Replace `AppHandle`/`Emitter` in:
  - `harness_action_event.rs` (1 emit, `harness-action`)
  - `review.rs` (the `emit_review_changed` site, `skein://review-changed`)
  - `review_surface/commands.rs` (`:152`, emits `skein://review-changed`)
  - `agent_api/state.rs` (6 emit sites: review-changed, harness-permission,
    session-start, session-end, mail-changed, and the `agent-request`
    emit used by `request_frontend`, which stays as is until #P1f)
  - `harness_events_claude/` (`ClaudeEventsManager`, field
    `app: Option<tauri::AppHandle>`) and `harness_events_opencode.rs`
    (`Option<AppHandle>` at 5 sites)
  - `agent_api/verbs/mail.rs` (`Option<AppHandle>` at 2 sites)
- Event names stay exactly as listed under "Protocol surface" in the
  recon; the Tauri implementation emits the same 7 names from the moving
  modules.

## Out of scope

- Client-only emitters: `open_request.rs`, `os_notify.rs`,
  `setup/menu.rs` (they stay in the app).
- Sequence numbers or replay. Nothing here adds a seq.
- `request_frontend` behaviour (#P1f).

## Acceptance

- `Grep` for `AppHandle` and `Emitter` in the modules listed above finds
  nothing, except the Tauri `EventSink` implementation file.
- `cargo test --manifest-path app/src-tauri/Cargo.toml` passes with the
  same test count; tests that use `None` or `for_test` pass unchanged.
- App launches; review changes, mail, permission dialogs and the Live
  Context feed update exactly as before (manual check on the dev profile).
- `cargo clippy` pedantic clean for both the workspace and
  `app/src-tauri`.
```

## Phase 1b

**Title:** skeind P1b: stream sinks and command/impl split for Channel commands and binary fs reads

**Labels:** `refactor`

```markdown
Part of #E. Phase 1 (zero behaviour change). Depends on: #P1a.

## Why

Five commands stream through `tauri::ipc::Channel`, and `fs.rs` returns
`tauri::ipc::Response` for raw bytes. A moving module cannot import those
types. The wrappers should hold only `State` and a call into an `_impl`
function that takes a stream sink; that sink is what a network transport
later implements.

## Scope

Paths under `app/src-tauri/src/`.

- Streaming sites to put behind a sink trait or closure (5):
  - `commands/pty.rs:58` `Channel<PtyEvent>`
  - `commands/harness_events.rs:36` `Channel<ClaudeEvent>`
  - `commands/harness_events.rs:205` `Channel<OpencodeEvent>`
  - `git.rs:457` `Channel<()>` (`git_watch_start`)
  - `design/commands.rs:89` `Channel<()>` (`design_watch_start`)
- Binary read: `fs.rs` `read_image_bytes` (and the `ipc::Response` at the
  other return site). Return plain bytes from the impl; the Tauri wrapper
  wraps them.
- Normalise the wrapper/impl split where it is mixed today: `git.rs`
  (11 commands) and `fs.rs` (4 commands) mix wrappers and logic, and
  `review.rs` has a command that takes `AppHandle` as a parameter.
- Channel sends already ignore errors (`let _ =`) except
  `harness_events.rs`, which breaks on error. Keep each site's behaviour.

## Out of scope

- Choosing the wire framing for binary reads. This issue owns
  the decision: it splits `fs.rs:192` and `review_image_bytes` off
  `tauri::ipc::Response`, so it picks the framing. Record it in
  `docs/skeind-recon.md` "Open questions" and in the epic.
- Moving files between crates.

## Acceptance

- No file outside a command wrapper imports `tauri::ipc`.
- `cargo test --manifest-path app/src-tauri/Cargo.toml` passes with the
  same test count.
- App launches; PTY spawn and output, Claude and opencode event
  attachment, the file tree refresh, the git branch watcher, the design
  preview refresh and image reads in the files harness all work as before.

## Risk

`pty_spawn` blocks on the login-shell probe (up to `PROBE_WAIT`, 6 s) and
the managed state is not `Arc`-wrapped. Do not change that here; note it
if the sink design needs the state shared.
```

## Phase 1c

**Title:** skeind P1c: path and resource provider replaces app.path()

**Labels:** `refactor`

```markdown
Part of #E. Phase 1 (zero behaviour change). Depends on: nothing; can run
alongside #P1a and #P1b.

## Why

A daemon cannot ask Tauri where its data, log and resource directories
are, and the harness-config bundle (the `--plugin-dir` and
`OPENCODE_CONFIG` targets) is a Tauri resource path on the UI machine
today. A plain `Paths { data_dir, resource_dir, log_dir }` value lets the
daemon be constructed with no `tauri::App`.

## Scope

- `app/src-tauri/src/setup/state.rs`: the two `app.path()` uses
  (`app_data_dir()`, `resource_dir()`) become reads from a `Paths` value;
  the app builds that value from Tauri once, at startup.
- `app/src-tauri/src/harness_config.rs`: take the bundle location from
  `Paths`, not from the app handle.
- Db location, `spawn_settings` load path, and the probe spool files
  under `data_dir` read from `Paths`.
- Reproduce the construction order that `setup/state.rs` and
  `setup/servers.rs` imply (managed state shared across modules, such as
  `AgentApiEndpoint` and `PreviewEndpoint`). Write that order down in a
  comment or in the recon.

## Out of scope

- Logging setup (`setup/logging.rs`). Both sides keep their own log dir.
- Where the daemon's bundle is installed. That is #P3.

## Acceptance

- A test constructs the shared state from `Paths` with no `tauri::App`.
- `cargo test --manifest-path app/src-tauri/Cargo.toml` passes.
- App launches on the dev, local and release identifiers with data in the
  same directories as before (compare the `skein.db` location).
- Harness spawn still injects the plugin bundle and `OPENCODE_CONFIG`
  (check Settings, Shell & environment).
```

## Phase 1d

**Title:** skeind P1d: create skein-proto and skein-daemon crates; move the leaf modules

**Labels:** `refactor`

```markdown
Part of #E. Phase 1 (zero behaviour change). Depends on: none strictly;
can start alongside #P1a. Do after #P1c if db paths are touched.

## Why

The first crate move should be the modules with no Tauri touch points, so
it proves the crate boundary and the wiring without any risk from the
event or stream seams.

## Scope

- New workspace crate `skein-proto`: the DTOs the moved modules expose.
  Versioned types from the start; no `hello` handshake yet (that is #P2b).
- New workspace crate `skein-daemon`, no Tauri dependency.
- `skein-proto` encodes the binary framing that #P1b chose.
- Move, from `app/src-tauri/src/`, modules whose logic has no Tauri touch
  points:
  - `db/` (about 4.7k lines, 22 `.rs` files, 13 non-test)
  - `spawn_env.rs`, `spawn_settings.rs`, `agents.rs`, `resume.rs`,
    `room_paths.rs`, `harness_kind.rs`, `watcher.rs`
- `resume.rs`'s probe functions are Tauri-free, but the file holds 4
  `#[tauri::command]` wrappers (`:40,57,75,116`) and 3
  `tauri::async_runtime::spawn_blocking` calls (`:42,59,77`). The probe
  logic moves into `skein-daemon` with `tokio::task::spawn_blocking`; the
  4 wrappers stay in the app and call it.
- `app/src-tauri` depends on `skein-daemon` and re-exports or imports from
  it; command wrappers stay in the app.
- Update the pre-commit hook and CI so fmt, clippy and tests run for the
  new crates (they join the workspace; `app/src-tauri` is still excluded).
- Keep the file-size guidance in CLAUDE.md in mind; do not grow files past
  the limits while moving them.

## Out of scope

- Engines (`pty`, event adapters, review) and anything with Tauri touch
  points: #P1e.
- Changing any DTO shape.

## Acceptance

- `cargo tree -p skein-daemon | grep tauri` prints nothing.
- `cargo test --workspace` and
  `cargo test --manifest-path app/src-tauri/Cargo.toml` pass; the total
  count of moved tests is unchanged.
- App launches with no behaviour change.
- `cargo fmt --all -- --check` and clippy pedantic `-D warnings` pass for
  the workspace and for `app/src-tauri`.
```

## Phase 1e

**Title:** skeind P1e: move the engines into skein-daemon

**Labels:** `refactor`, `area:harness-telemetry`

```markdown
Part of #E. Phase 1 (zero behaviour change). Depends on: #P1a, #P1b,
#P1c, #P1d.

## Why

The engines are where the daemon's behaviour lives. After the sinks and
path provider are in place they have no Tauri dependency left apart from
`#[tauri::command]` wrappers, which stay behind in the app.

## Scope

Move from `app/src-tauri/src/` into `skein-daemon`:

- `pty/` including `procscan.rs` (about 2.1k lines plus tests)
- `harness_events_claude/` (about 25 files, about 7k lines) and
  `harness_events_opencode.rs`
- `harness_actions_claude/` and `harness_actions_opencode.rs`
  (0 touch points)
- `harness_action_event.rs` (behind the sink from #P1a)
- `review.rs` and `review_surface/` (`commands.rs` holds 15 commands; the
  other 13 files have 0 touch points)
- `design/` serve, rewrite and device (the axum server is already
  Tauri-free); `design/commands.rs` splits
- `fs.rs` and `git.rs` implementations (wrappers stay)
- `harness_config.rs`
- The `commands/` files that are pure request/response over daemon state:
  `harness_log.rs` (7 commands), `rooms.rs`, `todos.rs`. Mixed files
  (`pty.rs`, `harness_events.rs`, `spawn_env.rs`) split: wrapper in the
  app, impl in the daemon.

Stays in the app: `open_request.rs`, `os_notify.rs`, `cli_shim.rs`,
`commands/app.rs`, `setup/menu.rs`, `setup/window.rs`, `lib.rs` (the
handler registry).

## Out of scope

- `agent_api/` and the embed (#P1f).
- Moving phase logic from TypeScript (#P2a).
- Re-scoping `fs.rs` paths. Paths are scoped to room cwds already
  (`ensure_room_scope`); CLAUDE.md still calls this unscoped (#174),
  which is stale. Fix that text as part of this move if touched.

## Acceptance

- `cargo tree -p skein-daemon | grep tauri` prints nothing.
- `cargo test --workspace` and
  `cargo test --manifest-path app/src-tauri/Cargo.toml` pass; moved test
  count unchanged.
- App launches; PTY spawn, Claude and opencode telemetry, the Live Context
  feed, the review pane, the files harness and the design preview behave
  as before.
- `procscan` still finds an opencode restarted from the post-exit shell
  (#517 behaviour).

## Risk

Windows ConPTY waiter thread in `pty/mod.rs` was not read in the recon.
Review it before moving it and test the move on Windows.
```

## Phase 1f

**Title:** skeind P1f: agent API and in-process transport; the app embeds the daemon

**Labels:** `refactor`, `area:agent-api`

```markdown
Part of #E. Phase 1 (zero behaviour change). Depends on: #P1e.

## Why

The agent API is the highest-risk module (about 16k lines including about
7k of tests) and the place where the UI and daemon are most entangled.
Moving it and rewriting daemon construction is what makes the app a client
of an embedded daemon. This is the end of Phase 1.

## Scope

- Move `agent_api/` (about 60 files): `state.rs` (16 touch points),
  `commands.rs` (10), `verbs/mail.rs` (3), and `http/*` (axum `State`
  only).
- Rewrite `setup/state.rs` and `setup/servers.rs` into daemon
  construction plus an embed: the app builds the daemon with `Paths`
  (#P1c) and an in-process transport, keeps its command wrappers, and
  forwards events from the daemon to the webview.
- **`request_frontend`** (`agent_api/state.rs`): the agent API emits
  `skein://agent-request` and waits for the webview to answer. Used by
  `create_room`, `open_harness`/`close_harness`, `close_room`,
  `list_rooms` and the design verbs. A headless daemon has no webview.
  In this phase only isolate it behind a client-request trait:
  - the Tauri implementation does what it does today, including the
    error "no webview is listening for agent requests";
  - behaviour is unchanged.
  Making these verbs work with no client is #P2a.
- Keep `AgentApiEndpoint` and `PreviewEndpoint` ordering from #P1c.

## Out of scope

- Any transport other than in-process.
- Changing token lifetime (revoked wholesale on every boot today): #P2a.
- Changing `docs/agent-api.md` verbs or contracts.

## Acceptance

- `cargo tree -p skein-daemon | grep tauri` prints nothing.
- `cargo test --workspace` and
  `cargo test --manifest-path app/src-tauri/Cargo.toml` pass, including
  all `agent_api` suites; test count unchanged.
- App launches with no behaviour change: MCP review verbs, the mailbox,
  `create_room` and the hook callbacks (`session-start`, `session-end`,
  permission) work on the dev profile.
- Single-instance, deep-link open and OS notifications are untouched.

## Open question

How much of the command-wrapper layer stays hand-written versus generated
from the verb table. Do not decide it here; keep wrappers thin.
```

## Phase 2a

**Title:** skeind P2a: daemon-owned harness lifecycle

**Labels:** `refactor`, `area:harness-telemetry`

```markdown
Part of #E. Phase 2 prerequisite. Depends on: #P1f.

## Why

A daemon with no client attached cannot work today. Argv, ports, event
logging, the phase machine and mail delivery all live in the TypeScript
frontend; rooms are saved wholesale by the client; tokens are revoked on
every boot. See "Coupling points" in `docs/skeind-recon.md` (items on
frontend-built argv, port pinning, phase state, nudges and mail, token
lifecycle).

## Scope

Move from the frontend into the daemon what unattended operation needs.

- **Argv building**: `harnessCmd.ts` (`cmdForKind`, `resumeCmd`,
  `withResumeCmds`, `unarchiveRoomTransform`), the defaults feeding it
  (`default_shell`, `default_cwd`, `defaultAgents`), so the daemon builds
  argv from the harness record with host-correct notions of platform.
- **opencode port allocation**: `pick_free_port` and the `--port` baked
  into argv on the client; the daemon allocates on its own host.
- **Event and action logging**: `db_record_harness_event` and
  `db_record_harness_action` are called from the webview; the daemon
  writes them.
- **Phase logic as needed**: `harnessActivity`, `deferral.ts`,
  `subagents.ts`, `backgroundTasks.ts`, the delegation timers, the
  permission phase, mail nudges (#381) and delivery through the `pty_write`
  seam. Open question below.
- **Room persistence**: replace `db_save_rooms` / `save_all` (wipe and
  re-insert) with per-room mutations that N clients can issue.
- **Agent API verbs that call into the webview** via `request_frontend`
  (`create_room`, `open_harness`/`close_harness`, `close_room`,
  `list_rooms`, design verbs): make them work with no client, or define
  the refusal.
- **Token lifetime**: revocation on boot must not break harnesses that
  outlive the client. The policy itself is an open question below.
- Daemon-host probes stay on the daemon: login-shell probe, program
  resolution, agent enumeration, CLI version probe.

## Out of scope

- The socket, reattach and snapshots (#P2b).
- Approval queue and push (#P4), which depends on this phase logic
  living in the daemon.
- Anything about Skein and git writes.

## Acceptance

- With the app embedded (attached mode), the app no longer builds argv,
  allocates ports or writes `harness_events` rows; the daemon does.
- A test starts the daemon with no client, creates a room and spawns a
  harness through the daemon API, and the phase and `harness_events` rows
  update.
- Two simulated clients write different rooms without erasing each other.
- `cargo test --workspace`, `cargo test --manifest-path
  app/src-tauri/Cargo.toml` and `cd app && npm test` pass.
- No change in what the user sees in attached mode.

## Open questions

- Token lifetime across client/daemon restarts: see recon §8 item 5.
- How much of the phase machine, deferral timers and mail nudges moves to
  the daemon, versus staying a client view over daemon-owned facts. The
  recon says an unattended daemon cannot leave it all in the webview, but
  the split is not decided.
- Order of moves, given that nearly every shipped regression has lived in
  the frontend and coverage there is thin.
- Whether `request_frontend` verbs need a client at all once the daemon
  owns room creation.
```

## Phase 2b

**Title:** skeind P2b: detached local mode

**Labels:** `area:rooms`, `area:ui`

```markdown
Part of #E. Phase 2. Depends on: #P2a. Informed by: #P0.

## Why

The first mode that gives a user something new: close the app, and the
harnesses keep working.

## Scope

- **Local socket** transport with filesystem permissions as the auth.
- **Daemon lifecycle**: the daemon outlives the app; the app finds and
  reattaches on the next launch; clean shutdown path; single-instance
  rules for the daemon (two daemons on one `skein.db` erase each other's
  rooms, so the existing single-instance concern applies).
- **`hello { proto_version, client_kind, capabilities }`** on connect.
- **Event stream**: a monotonic per-daemon seq covering every state
  change; reconnect with `since=<seq>`; snapshot plus tail when the gap is
  too old. Today no event has a seq or replay (see "Protocol surface").
- **PTY channel** with a headless emulator per PTY: bytes, input, resize,
  attach/detach. On attach send a screen snapshot, then live bytes.
  Emulator choice follows "Headless terminal emulator" in the recon:
  `vt100` recommended, behind a trait so `alacritty_terminal` or a fork
  can replace it. Snapshot and subscribe happen under one lock so no
  bytes are lost between them.
- **Resize**: last active client sets the size, re-asserted on
  reactivation.
- **Durability**: on daemon start, re-spawn the harnesses that should be
  running with resume and mark the resume in the event stream.
- **Settings**: attached vs detached.

## Out of scope

- Network transports and device tokens (#P3).
- Scrollback history beyond what the snapshot carries: `vt100`'s
  `state_formatted` has no history. Decide visible-screen-only attach or
  a separate scrollback pass here.

## Acceptance

- Quit the app with harnesses running; relaunch; the same harnesses are
  attached and the feed catches up from `since=<seq>`.
- Kill and restart the daemon; harnesses are re-spawned with resume and
  the event stream records it.
- **Emulator check**: replay a recorded Claude Code session through the
  emulator trait and compare the attach snapshot against xterm.js
  rendering of the same bytes. This is the gate for choosing `vt100`;
  failure points to the alternatives in the recon.
- Parser panics on odd resizes are caught and do not kill the daemon.
- `cargo test --workspace`, `cargo test --manifest-path
  app/src-tauri/Cargo.toml` and `cd app && npm test` pass.

## Open questions

- Should detached become the default?
- Is the snapshot visible-screen-only, or does it page scrollback out
  separately?
- Does a tmux-backed runtime (from #P0) change the emulator's role?
```

## Phase 3

**Title:** skeind P3: remote mode: network transport, device tokens, host picker

**Labels:** `area:rooms`, `security`

```markdown
Part of #E. Phase 3. Depends on: #P2b.

## Why

An always-on host runs the harnesses; the desktop client attaches over
the network.

## Scope

- **WebSocket transport** over a private overlay network. Reachability
  comes from the network; it is never the only protection.
- **Per-device revocable tokens** issued by the daemon; list and revoke
  from a client.
- **Version skew**: `hello { proto_version, client_kind, capabilities }`
  with a stated policy for what happens when client and daemon differ
  (refuse, degrade by capability, or prompt to update).
- **Host picker** at room creation, and a client that holds N daemon
  connections with reconnect and the last-seen seq per daemon. A room
  belongs to exactly one daemon.
- **`list_workspaces`** returning main checkouts under
  `SKEIND_WORKSPACE_ROOT`. A remote room is created against a checkout
  that already exists on the host; provisioning is out of scope.
- **Folder picker replacement**: the native dialog returns a client-machine
  path (used by `useNewRoomForm`, `MissingFolderCard`, `RepoMismatchCard`).
  Remote needs a daemon-side browser over `list_workspaces`.
- **Harness-config bundle shipped with the daemon**: `--plugin-dir` and
  `OPENCODE_CONFIG` point at a path that must exist on the harness host.
- **Design preview reachability**: the preview server listens on
  127.0.0.1 and the iframe loads it from the webview's own loopback, so a
  remote preview needs a proxy or tunnel.
- **Host-bound state**: rooms store absolute cwd, `repoRoot` and argv;
  these are daemon-host paths and clients must not interpret them locally.
- Client-local things stay client-local: OS notifications, clipboard,
  opener, updater, `skein` CLI shim, open-from-outside resolution against
  local rooms.
- Files harness and `fs.rs` reachable by a remote client: confirm the
  room-scoped canonicalization is the only path rule and test traversal.

## Out of scope

- Provisioning checkouts or credentials on the host.
- A `create_workspace` verb (optional, later).
- Mobile (#P6) and push (#P4).

## Acceptance

- A client on one machine creates a room against a workspace on another
  host and works in it: terminal, feed, review, files, design preview.
- A revoked device token is refused on the next request and its open
  connection is closed.
- A client with an incompatible `proto_version` gets the documented
  outcome.
- Tests cover token issue, revoke, expiry and the fs scope against `..`
  and symlink escapes.
- `cargo test --workspace`, `cargo test --manifest-path
  app/src-tauri/Cargo.toml` and `cd app && npm test` pass.

## Open questions

- Daemon discovery and config UX: manual list, MagicDNS, or mDNS?
- Review state for a room whose daemon is offline: read-only cache, or
  hide it?
- The updater for a remote daemon, and the version-skew policy (the
  backlog's remote release policy entry is related).
- Claude Code auth on a remote host (subscription vs API key); affects
  `spawn_env`.
- Token transport and storage on the client.
```

## Phase 4

**Title:** skeind P4: approval queue and push notifier

**Labels:** `area:notifications`, `area:agent-api`

```markdown
Part of #E. Phase 4. Depends on: #P2a (phase logic in the daemon), #P2b.
Useful before #P3 but not required by it.

## Why

A harness blocked on a permission dialog while no client is attached
needs a human to find out. Today the permission signal is a hook that
posts to `/api/harness/permission`, and what happens next is webview
state.

## Scope

- **Blocking permission hook**: the `PermissionRequest` hook in the #215
  plugin bundle becomes a daemon **approval record** that the hook waits
  on. The record appears in the event stream with an id.
- **Any client can answer; the first answer wins.** Later answers get a
  clear "already answered".
- **Per-room timeout policy**: keep waiting (default), auto-deny after N
  minutes, or notify again.
- **Notifier**: a pluggable interface in the daemon with a self-hosted push
  service as the first implementation (an ntfy-style HTTP topic). APNs and
  FCM come later, behind the same interface.
- Approvals are answered by a person: the agent API still has no verb to
  approve anything, and `approve`/`sign_off` stay refused by name.

## Out of scope

- The mobile client UI (#P6).
- APNs and FCM.
- Subagent dialog correlation changes. Keep the `agentId` behaviour from
  epic #298.

## Acceptance

- With no client attached, a permission request produces a push and a
  record in the event stream; answering from any client unblocks the hook.
- Two clients answering at once: exactly one wins, the other is told so.
- Timeout policies each have a test, including auto-deny.
- A notifier outage does not block or fail the hook.
- `cargo test --workspace` and `cargo test --manifest-path
  app/src-tauri/Cargo.toml` pass.

## Open questions

- What the push payload may contain, since the repo and any server are
  not private to the user's devices: keep it to ids and a short label.
- Whether opencode's permission events use the same record.
```

## Phase 5

**Title:** skeind P5: sandbox runtime behind the Runtime trait

**Labels:** `area:harness-telemetry`, `security`

```markdown
Part of #E. Phase 5. Depends on: #P1f, #P2a. Informed by: #P0.

## Why

Run a room's harnesses inside an isolated environment while the daemon
stays outside. Building sandbox infrastructure is not part of this
epic; this issue is the adapter.

## Scope

A `Runtime` implementation next to `HostRuntime`, with the trait methods
`ensure_room_env`, `spawn_pty`, `watch_paths` and `teardown`.

Constraints from the design:

- **One sandbox per room**, since a room's harnesses share a worktree.
- **The daemon runs outside** the sandbox and execs PTYs into it.
- **Worktree and harness state dir are readable by the daemon**: the
  transcript tails (`~/.claude/projects/...`, subagent sidecars,
  background-task `.output` files) and the opencode store must be reachable.
- **Egress must reach the agent API**: the harness hook and MCP URLs
  point at the daemon's listener.
- The candidate sandbox is at 0.1.0, so everything specific to it sits
  strictly behind the trait; no sandbox type leaks into the engines.

## Out of scope

- Building or operating the sandbox.
- Any git mutation: sandboxing does not change that Skein performs none.
- Multi-sandbox rooms.

## Acceptance

- The engines (events, review, files) run unchanged against a
  `Runtime` that is not `HostRuntime`; a test double exercises every
  trait method.
- A room in the sandbox spawns a harness, shows its feed and permission
  state, and receives review replies through the agent API.
- Teardown removes the room's environment and leaves the worktree.
- `cargo tree -p skein-daemon` shows no sandbox-specific dependency
  outside the runtime module.

## Open questions

- Whether `watch_paths` is a filesystem watcher on the daemon side or part
  of the runtime.
- Credential handling inside the sandbox; the daemon never owns clone
  or push credentials.
```

## Phase 6

**Title:** skeind P6: mobile client

**Labels:** `area:ui`

```markdown
Part of #E. Phase 6. Depends on: #P3, #P4.

## Why

Check on rooms and answer approvals away from the desk.

## Scope

A client that speaks the same protocol (`client_kind` identifies it in
`hello`), with:

- the activity feed per room
- the approval queue: see and answer pending approvals
- `send_message` to a harness or room
- a read-only terminal peek, using the emulator snapshot from #P2b and
  no input

## Out of scope

- Editing files, writing review comments, accepting or rejecting hunks,
  or anything with a write path beyond messages and approvals.
- Running a daemon on the device.

## Acceptance

- Connects to a remote daemon with a device token and survives a network
  change by resuming from `since=<seq>`.
- A push from #P4 opens the pending approval and answering unblocks the
  harness.
- Terminal peek cannot send input.

## Open questions

- Should a mobile client be able to grant sign-off?
- Native app vs web client; the design does not decide.
- How the device token is delivered to the phone.
```
