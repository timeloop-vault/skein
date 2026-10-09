# skeind — codebase recon

Status: verified recon, read-only. It checks `docs/skeind-design.md`
against the code. Nothing here is production code.
Date: 2026-10-09.
Verified against commit `4d785fe` (branch `issue/skeind-recon-epic`).

**How to read.** Paths are repo-relative. Line numbers are as of that
commit and drift; where a line could not be pinned the row says so. For
a `#[tauri::command]` the line is the attribute line, which is one above
the `fn`. "Covered by handover" refers to the five couplings in the
design's "Why it's a split". "Lives on" is where the logic belongs in a
split: daemon, client, or an open decision.

## 1. Summary

- **The handover's five couplings hold, but the list is incomplete.** All
  five are real. The table in §2 has 46 couplings: 11 are covered by
  the handover, 4 partly, and 31 not at all.
- **Coupling 3 is bidirectional.** The agent API does not only listen.
  `AgentApiState::request_frontend` emits `skein://agent-request` and
  blocks on an answer from the webview. `create_room`, `open_harness`,
  `close_harness`, `close_room`, `list_rooms` and the design verbs all
  depend on a live webview and fail without one.
- **The phase machine is TypeScript.** Phase, delegation and
  background-work deferral, permission clearing, held-mail nudges and
  badge/notification triggers live in `app/src/harnessActivity*.ts`,
  `deferral.ts`, `subagents.ts` and `backgroundTasks.ts`. The frontend
  also writes the `harness_events` rows (`db_record_harness_event`). A
  detached daemon has no phase without a client.
- **The frontend builds argv and picks the opencode port.** `cmdForKind`
  and `resumeCmd` run in the webview; `pick_free_port` is a command the
  webview calls and bakes into `--port`. On a remote host the daemon
  has to do both.
- **Tokens are revoked on every boot** (`setup/state.rs:87`). That
  assumes PTYs die with Skein. It is wrong for a daemon whose harnesses
  outlive a restart.
- **Injected paths are host-bound.** `--plugin-dir` and
  `OPENCODE_CONFIG` point at the Tauri resource dir of whichever machine
  runs the app. A remote daemon must ship its own harness-config bundle.
- **The design preview URL is the webview's own loopback.**
  `design_preview_base` returns `http://127.0.0.1:<port>/...` for an
  iframe, so a remote client cannot reach it without a proxy.
- **Rooms persist by wholesale mirror.** `db_save_rooms` wipes and
  re-inserts every row from one client's state. With several clients
  that erases each other's rooms.
- **Folder pickers return client paths** (`plugin-dialog`), which are
  used as a room's cwd. Remote rooms need a daemon-side browser.
- **Tauri coupling in the Rust backend is thin and concentrated.** The
  engines are Tauri-free apart from command wrappers, `Option<AppHandle>`
  fields, 8 emit sites in moving modules and 5 `Channel` sites. The 85
  commands across 17 files are the surface (§3).

## 2. Coupling points

Paths below are full. `src/...` under `app/src-tauri/` is Rust;
`app/src/...` is TypeScript.

| # | Coupling | Mechanism | Evidence | Covered by handover | Lives on |
|---|---|---|---|---|---|
| 1 | PTYs in-process | `PtyManager` owns the master and children via portable-pty; output goes over a `tauri::ipc::Channel`; children die with the process | `app/src-tauri/src/pty/mod.rs:111,243,295`; `app/src-tauri/src/commands/pty.rs:50-110` | yes (1) | daemon (spawn, output); client (xterm) |
| 2 | Claude JSONL tail | notify debouncer on `~/.claude/projects/<enc-cwd>/<sid>.jsonl`, with a home-wide `find_session_jsonl` fallback | `app/src-tauri/src/harness_events_claude/paths.rs:18-34`; `crates/skein-harness/src/claude.rs:132`; `app/src-tauri/src/harness_events_claude/adapter.rs:117` | yes (2) | daemon |
| 3 | Subagent sidecar tailing | `<sid>/subagents/agent-*.jsonl` and `.meta.json` on the same debouncer | `app/src-tauri/src/harness_events_claude/subagents.rs:37`; `app/src-tauri/src/harness_events_claude/attach.rs:337-343` | partly | daemon |
| 4 | Background-task `.output` trailers | reads `.output` paths the harness printed (arbitrary harness-host paths) | `crates/skein-harness/src/claude/background.rs:22,112,663` | no | daemon |
| 5 | opencode SSE | reqwest to `http://127.0.0.1:<port>/event` and `GET /session/<id>` | `app/src-tauri/src/harness_events_opencode.rs:290,912` | yes (2) | daemon |
| 6 | opencode port pinning | `pick_free_port` binds `127.0.0.1:0` and drops it; the frontend bakes the port into argv | `app/src-tauri/src/commands/pty.rs:198`; `app/src-tauri/src/lib.rs:132`; `app/src/harnessCmd.ts:62-68`; `app/src/useHarnessActions.ts:295` | no | daemon allocates; client must not build argv |
| 7 | opencode.db reads | read-only open of opencode's db for session list, existence and action backfill | `app/src-tauri/src/resume.rs:16-31,43,60`; `app/src-tauri/src/harness_actions_opencode.rs:440` | no | daemon |
| 8 | Claude session probes | `claude_session_exists` and `claude_transcript_stat` stat `~/.claude/...` | `app/src-tauri/src/resume.rs:78,124` | no | daemon |
| 9 | Agent API listener | axum bound to `127.0.0.1:0` in `setup()` | `app/src-tauri/src/setup/servers.rs:20,42`; routes `app/src-tauri/src/agent_api/http.rs:79-114` | yes (3) | daemon |
| 10 | Env injection at spawn | `SKEIN_REVIEW_URL/TOKEN`, `SKEIN_ROOM_ID`, `SKEIN_HARNESS_ID` on the child; loopback URL; token minted from sqlite | `app/src-tauri/src/pty/env.rs:200-203`; `app/src-tauri/src/commands/pty.rs:70-84` | yes (3) | daemon |
| 11 | Hook callbacks | three hook events (four commands) in a plugin `hooks.json`; the SessionStart, SessionEnd and PermissionRequest entries `curl` `${SKEIN_REVIEW_URL%/mcp}/api/harness/{session-start,session-end,permission}`; `sh` syntax; `cli_shim.rs` is not involved | `app/src-tauri/harness-config/claude-plugin/hooks/hooks.json:9,28,39`; handlers `app/src-tauri/src/agent_api/http/hooks.rs:28,108,202` | yes (3) | harness host; target must be reachable from it |
| 12 | opencode MCP config | `opencode.json` uses `{env:SKEIN_REVIEW_URL}` and `{env:SKEIN_REVIEW_TOKEN}` | `app/src-tauri/harness-config/opencode/opencode.json` | partly | harness host |
| 13 | Injected resource paths | `--plugin-dir <resource_dir>/harness-config/claude-plugin` and `OPENCODE_CONFIG=<file>` are paths on the app's machine; post-exit shells get `CLAUDE_CODE_PLUGIN_DIRS` | `app/src-tauri/src/harness_config.rs:174-177,335,343`; `app/src-tauri/src/setup/state.rs:58` | no | daemon must carry its own bundle |
| 14 | Agent API calls into the webview | `request_frontend` emits and awaits the webview | `app/src-tauri/src/agent_api/state.rs:466,480`; `agent_api/verbs/room_create.rs:320,358`; `harness_control.rs:218,265,492`; `room_close.rs:194`; `room_listing.rs:76`; `design.rs:271,301` (all under `app/src-tauri/src/`) | no | client/daemon seam |
| 15 | Agent API one-way UI events | `app.emit` for review-changed, harness-permission, mail-changed, session-start, session-end | `app/src-tauri/src/agent_api/state.rs:277,303,324,362,385` | no | event stream |
| 16 | Other server-to-UI emits | harness-action; review discovery's review-changed; os-notification-clicked; open-request; menu events | `app/src-tauri/src/harness_action_event.rs:63`; `app/src-tauri/src/review.rs:853`; `app/src-tauri/src/review_surface/commands.rs:152`; `app/src-tauri/src/os_notify.rs:116`; `app/src-tauri/src/open_request.rs:161`; `app/src-tauri/src/setup/menu.rs:11,15` | no | event stream (first three); client (rest) |
| 17 | Phase state machine in the webview | `harnessActivity*.ts` consumes the channels; badges, deferral timers, mail nudges and the permission phase are all there | `app/src/harnessActivityCore.ts`; `app/src/deferral.ts`; `app/src/subagents.ts`; `app/src/backgroundTasks.ts`; `app/src-tauri/src/lib.rs:126-131` | no | decision needed (§8) |
| 18 | Nudges and mail delivery | the frontend writes to the PTY via `pty_write` | `app/src/harnessInput.ts`; `app/src/useMailDelivery.ts:219`; `app/src-tauri/src/lib.rs:91` | no | client logic over a daemon PTY; must move for unattended |
| 19 | Worktree watcher | notify debouncer on the room cwd, `.git/` unfiltered, `Channel<()>` | `app/src-tauri/src/watcher.rs:59-100`; `app/src-tauri/src/git.rs:455-465`; `app/src-tauri/src/lib.rs:116` | yes (4) | daemon |
| 20 | Review discovery watcher | per-room fs watcher plus a `git status` catch-up | `app/src-tauri/src/review.rs:341-374,798` | partly | daemon |
| 21 | Baseline capture | `Repo::open(cwd)` then `head_blob` at patch-row time | `app/src-tauri/src/review.rs:184-206,350` | partly (4) | daemon |
| 22 | Git reads and worktree add | libgit2 local: status, diff, `git_add_worktree`, `git_restore_worktree`, `git_inspect_folder`, `git_repo_identity` | `app/src-tauri/src/git.rs:78-806`; `app/src-tauri/src/open_request.rs:314` | yes (4) | daemon |
| 23 | Files harness fs | `list_dir`, `read_file_text`, `write_file_text`, `read_image_bytes`, scoped to room cwds via `ensure_room_scope` | `app/src-tauri/src/fs.rs:68-72,89,142,193,249` | yes (4) | daemon |
| 24 | Review surface | per-file diff, `review_image_bytes` and anchoring read the worktree | `app/src-tauri/src/review_surface/commands.rs:28-352` | yes (4) | daemon |
| 25 | Agent API worktree reads | `element_source` walks the worktree (5000 files, 32 MiB cap); `get_diff` reads git | `app/src-tauri/src/agent_api/element_source.rs:1-30` | no | daemon |
| 26 | Design preview server | a second `127.0.0.1:0` listener serves the worktree; `design_preview_base` hands the iframe `http://127.0.0.1:{port}/preview/{token}/`; CSP is null | `app/src-tauri/src/setup/servers.rs:62-77`; `app/src-tauri/src/design/commands.rs:54-64`; `app/src/DesignBody.tsx:306`; `app/src-tauri/tauri.conf.json:31` | no | daemon serves; client iframe; URL must be client-reachable |
| 27 | Design watcher and entries | another recursive watcher and a dir walk | `app/src-tauri/src/design/commands.rs:67,87-131` | no | daemon |
| 28 | sqlite `skein.db` | `<app_data_dir>/skein.db` shared in-process by commands, agent API, adapters and `PtyManager`; rooms store absolute cwd, repoRoot and argv (host-bound) | `app/src-tauri/src/setup/state.rs:24,67-88`; `app/src-tauri/src/lib.rs:104-107` | yes (5) | daemon; commands become requests |
| 29 | Token lifecycle | tokens are revoked wholesale on every boot, assuming PTYs died; breaks harnesses that outlive a restart | `app/src-tauri/src/setup/state.rs:87` | no | daemon; policy must change |
| 30 | `settings.json` and spawn settings | `spawn_settings::load(data_dir)`; the kill switches (`allowAgentMessaging` etc.) are read by the agent API | `app/src-tauri/src/setup/state.rs:30`; `app/src-tauri/src/spawn_settings.rs:101`; `app/src-tauri/src/agent_api/http.rs:189` | no | daemon policy; edited remotely by a client |
| 31 | Login-shell probe | `$SHELL -l -c` (or `-l -i -c`) on the app host; macOS/Linux only. On this commit it captures only `PATH`, spooled to `<data_dir>/probe-<uuid>.out`. #565, in flight elsewhere, extends it to the full env | `app/src-tauri/src/pty/probe.rs:179,286,313,326`; `app/src-tauri/src/spawn_env.rs:54-72` | no | daemon |
| 32 | Spawn env policy | inherits the Skein process env, strips host-terminal vars, reads the Windows registry PATH, sets `SHELL` | `app/src-tauri/src/pty/env.rs:85-190` | no | daemon |
| 33 | Program resolution | `harness_program_lookup` resolves `claude` and `opencode` against the probed PATH | `app/src-tauri/src/pty/preview.rs` (re-exported `pty/mod.rs:52`); `app/src-tauri/src/agents.rs:121`; `app/src-tauri/src/commands/spawn_env.rs:160` | no | daemon |
| 34 | Agent enumeration | runs the CLI (`claude`, `opencode agent list`) plus a disk allowlist; `agent_sees_mcp` reads agent definitions | `app/src-tauri/src/agents.rs:114-140,165-180`; `crates/skein-harness/src/agents/claude.rs:109` | no | daemon |
| 35 | CLI version probe | `claude --version` on the app host | `app/src-tauri/src/commands/spawn_env.rs:152-165` | no | daemon |
| 36 | procscan | `sysinfo` process table of PTY descendants, argv parse, `pid_listens_on(port)` | `app/src-tauri/src/pty/procscan.rs:225-246`; `app/src-tauri/src/commands/pty.rs:169-181` | no | daemon |
| 37 | Frontend-built argv and defaults | `default_shell` and `default_cwd` are asked of the host, but `cmdForKind` and `resumeCmd` are built client-side; `isMac`/`isWindows` come from `navigator.platform` | `app/src/useAppWindowEffects.ts:39-40`; `app/src/harnessCmd.ts`; `app/src/shortcuts.ts:25-34`; `app/src/notifications.tsx:64-78` | no | argv on daemon; platform flags on client |
| 38 | Folder pickers | `plugin-dialog` `open()` returns a client-machine path used as the room cwd | `app/src/MissingFolderCard.tsx:7`; `app/src/RepoMismatchCard.tsx:6`; `app/src/useNewRoomForm.tsx:2` | no | needs a daemon-side browser (`list_workspaces`) |
| 39 | Single-instance, open-request, deep link | a second launch forwards argv and cwd; paths are canonicalized against local rooms; `skein://` scheme | `app/src-tauri/src/lib.rs:67-70`; `app/src-tauri/src/open_request.rs:67,186,314` | no | client; resolution needs the daemon's rooms and disk |
| 40 | `skein` CLI shim | writes `~/.local/bin/<scheme>`; for opening folders, not hooks | `app/src-tauri/src/cli_shim.rs:1-30,345,389` | no | client |
| 41 | OS notifications | `tauri-plugin-notifications` (skipped in macOS debug); on Windows `skein-winnotify` plus `notify-icon.png` in `app_data_dir`; pending click slot | `app/src-tauri/src/lib.rs:77`; `app/src-tauri/src/os_notify.rs:116-129`; `app/src/notifications.tsx` | no | client (a push notifier is daemon, Phase 4) |
| 42 | Clipboard, opener, updater | `plugin-clipboard-manager`, `plugin-opener`, `plugin-updater` | `app/src-tauri/src/lib.rs:72-74`; `app/src/terminalInteractions.ts:12`; `app/src/terminalSetup.ts:7` | no | client |
| 43 | Window and capabilities | `window_raise_main`, the control-center popout window, the macOS menu | `app/src-tauri/src/commands/app.rs:40`; `app/src-tauri/src/setup/window.rs`; `app/src-tauri/src/setup/menu.rs`; `app/src-tauri/capabilities/default.json` | no | client |
| 44 | Logging | `app_log_dir()/skein.log.*` | `app/src-tauri/src/setup/logging.rs:28-30` | no | both, own dirs |
| 45 | Frontend `localStorage` prefs | `prefs.ts` holds per-kind default agents and similar, feeding argv | `app/src/prefs.ts` | no | client; an input to argv building (§8) |
| 46 | Event and action logging via the webview | `db_record_harness_event` and `db_record_harness_action` are invoked from the frontend | `app/src-tauri/src/lib.rs:133,136`; `app/src/useHarnessActions.ts:324`; `app/src/useTransitionLog.ts:27` | no | daemon |

Corrections this table makes to the handover and to the working notes:

- Coupling 11 is `curl` in `hooks.json`, not `cli_shim.rs`. There are
  three hook events and four commands: SessionStart has two entries (the
  `curl` ping, plus a synchronous env-file `unset` for nested `claude`
  children), then SessionEnd and PermissionRequest.
- The handover's coupling 3 (agent API) is half the story: rows 14 to
  16 show it also pushes into the webview and blocks on it.
- The handover's coupling 2 also covers rows 3, 4, 7 and 8.
- Rows 17, 18 and 46 move unattended logic from TypeScript to the
  daemon, or leave it with a client that is not there.
- Two stores, not one: `skein.db` (row 28) and the tools' own stores
  (`~/.claude`, `opencode.db`, rows 2, 7, 8). The latter must sit next
  to the harness.
- Not checked: whether PTYs are killed explicitly on window close;
  `skein-winnotify` internals; the design pane's injected JS
  (`invoke.js`, `changes.js`).

## 3. Protocol surface

### 3a. Commands

85 commands, listed in `generate_handler!` at
`app/src-tauri/src/lib.rs:83-169`. `os_notify` has two `cfg` variants of
two commands; they count once.

Classes:

- **D**: daemon request/response.
- **D+S**: a request that today returns a stream. It becomes an
  event-stream or PTY-channel subscription.
- **C**: client-local.
- **?**: open question (§8).

Totals: **D 66, D+S 5, C 8, ? 6 = 85.**

**app** (`app/src-tauri/src/commands/app.rs`): D 1, C 2.

| Command | Line | Class |
|---|---|---|
| `ping` | 6 | D (hello/health) |
| `frontend_log` | 20 | C |
| `window_raise_main` | 40 | C |

**fs** (`app/src-tauri/src/fs.rs`): D 4. All room-scoped by
`ensure_room_scope` (`:68`).

| Command | Line | Class |
|---|---|---|
| `list_dir` | 88 | D |
| `read_file_text` | 141 | D |
| `read_image_bytes` | 192 | D (binary reply, `tauri::ipc::Response` at `:196`) |
| `write_file_text` | 248 | D |

**pty** (`app/src-tauri/src/commands/pty.rs`): D+S 1, D 5.

| Command | Line | Class |
|---|---|---|
| `pty_spawn` | 49 | D+S (PTY channel) |
| `pty_write` | 123 | D (PTY channel input) |
| `pty_resize` | 136 | D (PTY channel resize) |
| `pty_kill` | 148 | D |
| `pty_scan_opencode` | 168 | D |
| `pick_free_port` | 197 | D (must run on the daemon host) |

**spawn_env** (`app/src-tauri/src/commands/spawn_env.rs`): D 8, ? 1.

| Command | Line | Class |
|---|---|---|
| `spawn_settings_load` | 54 | D |
| `spawn_settings_save` | 78 | D |
| `spawn_env_preview` | 101 | D |
| `list_harness_agents` | 127 | D |
| `claude_cli_version` | 151 | D |
| `harness_config_status` | 180 | D |
| `spawn_env_reprobe` | 188 | D |
| `default_shell` | 210 | D |
| `default_cwd` | 246 | ? (the daemon host's home, for a remote room) |

**rooms** (`app/src-tauri/src/commands/rooms.rs`): D 1, ? 1.

| Command | Line | Class |
|---|---|---|
| `db_load_rooms` | 17 | D |
| `db_save_rooms` | 83 | ? (wholesale mirror; needs per-room mutations) |

**todos** (`app/src-tauri/src/commands/todos.rs`): ? 2.

| Command | Line | Class |
|---|---|---|
| `db_load_global_todos` | 11 | ? (global across daemons) |
| `db_save_global_todos` | 23 | ? |

**git** (`app/src-tauri/src/git.rs`): D+S 1, D 10.

| Command | Line | Class |
|---|---|---|
| `git_is_repo` | 77 | D |
| `git_inspect_folder` | 124 | D |
| `git_repo_identity` | 304 | D |
| `git_head_branch` | 318 | D |
| `git_propose_worktree_path` | 328 | D |
| `git_add_worktree` | 342 | D (creates a worktree; see §7) |
| `git_restore_worktree` | 382 | D (see §7) |
| `git_status` | 437 | D |
| `git_watch_start` | 454 | D+S (`Channel<()>`) |
| `git_watch_stop` | 472 | D |
| `git_diff` | 805 | D |

**design** (`app/src-tauri/src/design/commands.rs`): D 1, D+S 1, ? 1.

| Command | Line | Class |
|---|---|---|
| `design_preview_base` | 53 | ? (URL is the webview's loopback) |
| `design_list_entries` | 66 | D |
| `design_watch_start` | 86 | D+S (`Channel<()>`) |

**resume** (`app/src-tauri/src/resume.rs`): D 4.

| Command | Line | Class |
|---|---|---|
| `opencode_list_sessions` | 40 | D |
| `opencode_session_exists` | 57 | D |
| `claude_session_exists` | 75 | D |
| `claude_transcript_stat` | 116 | D |

**harness_events** (`app/src-tauri/src/commands/harness_events.rs`):
D+S 2, D 3.

| Command | Line | Class |
|---|---|---|
| `claude_events_attach` | 29 | D+S (`Channel<ClaudeEvent>`) |
| `claude_events_detach` | 119 | D |
| `claude_events_reattach` | 140 | D |
| `opencode_events_attach` | 198 | D+S (`Channel<OpencodeEvent>`) |
| `opencode_events_detach` | 216 | D |

**harness_log** (`app/src-tauri/src/commands/harness_log.rs`): D 7.

| Command | Line | Class |
|---|---|---|
| `db_record_harness_event` | 21 | D (the frontend writes phase transitions) |
| `db_recent_harness_events_by_harness` | 51 | D |
| `db_recent_harness_events_by_room` | 70 | D |
| `db_record_harness_action` | 97 | D |
| `db_recent_harness_actions_by_harness` | 128 | D |
| `db_recent_harness_actions_by_room` | 147 | D |
| `db_recent_harness_actions_by_room_and_kind` | 166 | D |

**review** (`app/src-tauri/src/review.rs`): D 4.

| Command | Line | Class |
|---|---|---|
| `review_pending` | 720 | D |
| `review_accept` | 736 | D |
| `review_reject` | 765 | D |
| `review_discovery_start` | 797 | D (starts a watcher that emits review-changed) |

**review_surface**
(`app/src-tauri/src/review_surface/commands.rs`): D 15.

| Command | Line | Class |
|---|---|---|
| `review_scope` | 28 | D |
| `review_file` | 51 | D |
| `review_image_bytes` | 79 | D (binary reply) |
| `review_add_thread` | 108 | D |
| `review_element_seen` | 132 | D (also emits review-changed, `:152`) |
| `review_element_threads` | 167 | D |
| `review_reply` | 183 | D |
| `review_edit_comment` | 217 | D |
| `review_delete_comment` | 240 | D |
| `review_delete_thread` | 257 | D |
| `review_resolve_thread` | 273 | D |
| `review_mark_viewed` | 294 | D |
| `review_set_base` | 315 | D |
| `review_signoff_status` | 334 | D |
| `review_set_signoff` | 352 | D |

**agent_api** (`app/src-tauri/src/agent_api/commands.rs`): D 3, ? 1.

| Command | Line | Class |
|---|---|---|
| `agent_api_status` | 19 | D |
| `mail_unread` | 27 | D |
| `mail_last_status_by_room` | 55 | D |
| `agent_request_complete` | 82 | ? (the answer leg of the `request_frontend` round trip) |

**os_notify** (`app/src-tauri/src/os_notify.rs`): C 2.

| Command | Line | Class |
|---|---|---|
| `os_notify_show` | 225 (Windows), 253 (stub) | C |
| `os_notify_take_pending` | 270 (Windows), 278 (stub) | C |

**open_request** (`app/src-tauri/src/open_request.rs`): C 1.

| Command | Line | Class |
|---|---|---|
| `open_request_take` | 413 | C (resolves against the stored rooms, so the client needs the daemon's room list) |

**cli_shim** (`app/src-tauri/src/cli_shim.rs`): C 3.

| Command | Line | Class |
|---|---|---|
| `cli_shim_status` | 199 | C |
| `cli_shim_install` | 206 | C |
| `cli_shim_uninstall` | 233 | C |

### 3b. Events and channels

Producers are under `app/src-tauri/src/`, consumers under `app/src/`.

| Event or channel | Emitted at | Consumed at | Purpose | Class |
|---|---|---|---|---|
| `skein://review-changed` `{roomId}` | `review.rs:853`; `review_surface/commands.rs:152`; `agent_api/state.rs:277` | `review/useReviewData.ts:117`; `useDesignComments.ts:87`; `controlCenter/useControlCenterData.ts:229` | a comment or review file changed | event-stream event |
| `skein://harness-permission` | `agent_api/state.rs:303` | `useHarnessHookEvents.ts:34` | Claude permission dialog opened | event-stream event |
| `skein://harness-session-start` | `agent_api/state.rs:362` | `useHarnessHookEvents.ts:185` | SessionStart hook | event-stream event |
| `skein://harness-session-end` | `agent_api/state.rs:385` | `useHarnessHookEvents.ts:225` | SessionEnd hook | event-stream event |
| `skein://mail-changed` | `agent_api/state.rs:324` | `useMailDelivery.ts:219`; `controlCenter/useControlCenterData.ts:229` | mailbox changed | event-stream event |
| `skein://agent-request` | `agent_api/state.rs:480` | `useAgentRequests.ts:325` | agent API asks the webview to answer | open question (replaced by a client-request channel, §8) |
| `harness-action` | `harness_action_event.rs:63` | `liveContext/store.ts:135`; `useApiErrorToasts.ts:40` | live action rows | event-stream event |
| `skein://open-request` | `open_request.rs:161` | `useOpenRequests.ts:81` | path handed in from outside | client-local |
| `skein://os-notification-clicked` | `os_notify.rs:116` | `useOsNotificationClicks.ts:122` | OS toast clicked | client-local |
| `skein://open-settings` | `setup/menu.rs:11` | `useAppWindowEffects.ts:122` | menu to Settings | client-local |
| `skein://quit-requested` | `setup/menu.rs:15` | `useAppWindowEffects.ts:84` | menu quit, frontend guards unsaved state | client-local |
| `Channel<PtyEvent>` | `commands/pty.rs:58` (send `:106`) | `useTerminalSpawn.ts:172` (invoke `:277`) | PTY output and exit | PTY channel |
| `Channel<ClaudeEvent>` | `commands/harness_events.rs:36` (send `:65`) | `harnessEventsClaude.ts:137` (invoke `:171`) | Claude JSONL semantic events | event-stream event |
| `Channel<OpencodeEvent>` | `commands/harness_events.rs:205` (send `:209`) | `harnessEventsOpencode.ts:86` (invoke `:105`) | opencode SSE semantic events | event-stream event |
| `Channel<()>` git watch | `git.rs:457` (send `:465`) | `FileTree.tsx:121`; `liveContext/useGitBranchWatcher.ts:36`; `review/useReviewData.ts:49` | worktree watcher tick | event-stream event |
| `Channel<()>` design watch | `design/commands.rs:89` (send `:99`) | `useDesignPreview.ts:70` | design-file watcher tick | event-stream event |

Notes:

- **There is no seq and no replay anywhere today.** Channels are
  per-invoke and die with the webview. Events are fire-and-forget; the
  `Channel` sends ignore errors (`let _ =`), and only
  `harness_events.rs:65` stops on a send error. The design's monotonic
  seq and `since=` resume are new.
- `skein://review-changed` is also emitted from the webview itself
  (`app/src/review/api.ts:260`), so a client can originate an event the
  daemon would otherwise own.
- Git and design watches share `WatcherManager`; `useDesignPreview.ts:83`
  stops the design watch with `git_watch_stop`.
- Window-to-window `cc-popout:*` events
  (`app/src/controlCenter/popoutProtocol.ts:36-42`) have no Rust emitter
  and stay client-local.

### 3c. The existing request vocabulary: agent API verbs and hooks

The protocol's requests should map onto the agent API where possible
(design: "Requests map 1:1 onto existing agent API verbs"). The surface
is `docs/agent-api.md`; the route table is
`app/src-tauri/src/agent_api/http.rs:79-114`.

`docs/agent-api.md` says "All twenty-two verbs" (`:97`, and again at
`:1304`). Counting is not that simple: the document has 18 verb
headings at `###` level that are verbs (7 review including
`skein_info`, 3 mailbox, 4 room and harness control, 4 listing and
lookup; the other `###` headings are caps, scope notes and the
session-end signal) and the 7 design-pane verbs sit in a
table and bullet list under `## Driving the design pane` (`:685`)
rather than under headings. The real number is **25 verbs**. The
"twenty-two" figure predates the design verbs.

| Group | Verbs |
|---|---|
| Review | `skein_info`, `list_comments`, `get_comment`, `get_diff`, `reply`, `mark_addressed`, `review_status` |
| Mailbox | `send_message`, `read_messages`, `message_history` |
| Rooms and harnesses | `create_room`, `close_room`, `open_harness`, `close_harness`, `find_rooms_for_path`, `list_rooms`, `get_room`, `list_harnesses` |
| Design pane | `list_design_harnesses`, `get_design_state`, `open_design_entry`, `set_design_device`, `show_element`, `invoke_element`, `show_changes` |

Refused by name: `resolve`, `approve`, `sign_off`, `archive_room`,
`remove_worktree`, `delete_room`.

Hook endpoints (not verbs, no MCP form; called by `curl` from the
harness host):

- `POST /api/harness/permission` (carries `agent_id`),
- `POST /api/harness/session-start`,
- `POST /api/harness/session-end`.

Facts the protocol inherits:

- Bearer token per room, revoked on every boot; `X-Skein-Harness` is
  attribution only; a non-localhost `Origin` is rejected with 403.
  `docs/agent-api.md` does not discuss any non-loopback use.
- Env injected into a harness: `SKEIN_REVIEW_URL`, `SKEIN_REVIEW_TOKEN`,
  `SKEIN_ROOM_ID`, `SKEIN_HARNESS_ID`, `SKEIN_VERSION`, expanded with
  `${VAR}` in MCP config.
- Of these verbs, `create_room`, `open_harness`, `close_harness`,
  `close_room`, `list_rooms` and the design verbs are the ones that
  round-trip through the webview today (coupling 14). They are the
  ones that cannot be a simple 1:1 mapping in a headless daemon.

## 4. Phase 1 sizing by module

Phase 1 is "extract skein-daemon with the in-process transport and zero
behaviour change". Size is given as counts and what is uncertain.

Totals:

- 85 commands across 17 files.
- 5 streaming `Channel` sites: `commands/pty.rs:58`,
  `commands/harness_events.rs:36`, `commands/harness_events.rs:205`,
  `design/commands.rs:89`, `git.rs:457`.
- 8 `app.emit` sites in modules that move: 6 in
  `agent_api/state.rs`, 1 in `harness_action_event.rs`, 1 in
  `review.rs`. Plus `review_surface/commands.rs:152`, which emits
  through `tauri::Emitter` directly. In modules that stay client-side:
  `open_request.rs`, `os_notify.rs`, and 2 in `setup/menu.rs`.
- 2 `app.path()` sites, both in `setup/state.rs` (`:24`, `:58`);
  `os_notify.rs:129` and `setup/logging.rs:28` are client-side.

Module table (all under `app/src-tauri/src/` unless stated):

| Module | Lines | Moves / stays | Tauri touch points | Notable leak | Risk |
|---|---|---|---|---|---|
| `db/` (22 `.rs` files, 13 non-test) | ~4.7k | moves | 0 | none | clean; move first |
| `pty/` (mod, env, output, probe, preview, procscan) | ~2.1k + tests | moves | 3 in `mod.rs`, 1 in `output.rs`, 0 in `procscan` | `commands/pty.rs:58` `Channel<PtyEvent>` is the only sink | low |
| `commands/pty.rs` | 221 | split | 21; 6 commands | `:58` | the wrapper builds the sink |
| `commands/harness_events.rs` | 222 | split | 25; 5 commands | `:36`, `:205` | two more streaming sinks |
| `commands/harness_log.rs` | 180 | moves | 21 (`State`); 7 commands | none | pure request/response |
| `commands/spawn_env.rs` | 256 | split | 23 (`State`, `AppHandle`, `Manager`); 9 commands | `:54-81` | settings UI stays, probe moves |
| `commands/rooms.rs`, `todos.rs`, `app.rs` | 93 / 33 / 43 | rooms, todos move; app stays | 6 / 6 / 4 | none | trivial |
| `harness_events_claude/` (~25 files) | ~7k | moves | `manager.rs` 11, `adapter.rs` 3, `attach.rs` 3, `supervise.rs` 6 | `app: Option<tauri::AppHandle>` at `manager.rs:44`, `adapter.rs:82,471` | `AppHandle` only emits `harness-action`; tests use `None` |
| `harness_events_opencode.rs` | 930 | moves | 8 (`Option<AppHandle>` at `:182,284`) | `:186` `new(db, app)` | same pattern |
| `harness_actions_claude.rs`, `harness_actions_opencode.rs` | 654 / 566 | moves | 0 | none | clean |
| `harness_action_event.rs` | 66 | behind a sink | 3 | `:43`, `:63` | the emit shim |
| `watcher.rs` | 110 | moves | 0 real | none | callback-based; consumers `review.rs:797`, `design/commands.rs:86` |
| `fs.rs` | 516 | split | 15; 4 commands; `tauri::ipc::Response` at `:196,:203` | `:192` binary read | binary framing on the wire |
| `git.rs` | 814 | split | 24; 11 commands; `Channel` import `:17` | `git_watch_start` `:454` | wrappers and logic mixed |
| `resume.rs` | 149 | moves | 4 `#[tauri::command]` attributes (`:40,57,75,116`) + 3 `tauri::async_runtime::spawn_blocking` (`:42,59,77`) | `:42,59,77` | probe logic moves, wrappers stay; swap to tokio |
| `review.rs` | 864 | split | 19; `tauri::Emitter` `:59` | `:801` `AppHandle` param; `:852-853` `emit_review_changed` | sink seam plus watcher wiring |
| `review_surface/` | ~6k | moves | `commands.rs` 51 (15 commands); 13 other files 0 | `commands.rs` | already isolated |
| `agent_api/` (55 files) | ~16k incl. ~7k tests | moves, with a caveat | `state.rs` 16; `commands.rs` 10; `verbs/mail.rs` 3 (`Option<AppHandle>` `:82,264`); `http/*` axum `State` only | `state.rs:120,232` `AppHandle`; emits `:277,303,324,362,385,480`; `:466` `request_frontend` | highest |
| `design/` (serve, commands, rewrite, device) | ~1.4k | serve, rewrite, device move; commands split | `commands.rs` 16 (`Channel<()>` `:13,:89`) | `:89` | the axum server is already Tauri-free |
| `harness_config.rs` | 747 | moves | 0; resource dir comes from `setup/state.rs:58` | `:174` `resolve(resource_dir)` | needs a resource-dir provider |
| `spawn_env.rs`, `spawn_settings.rs`, `agents.rs`, `harness_kind.rs`, `room_paths.rs` | 496 / 457 / 385 / 216 / 141 | move | 0 | none | clean |
| `lib.rs` | 187 | stays (registry) | 12 | none | none |
| `setup/` (logging, menu, servers, state, window) | ~400 | stays; `state.rs` + `servers.rs` become daemon boot | `state` 14, `servers` 5, `menu` 8, `window` 4 | `state.rs:24` `app_data_dir()`, `:58` `resource_dir()`, `:92-100` `app.manage(...)` | daemon construction lives here |
| `open_request.rs`, `os_notify.rs`, `cli_shim.rs` | 786 / 334 / 410 | stay | 11 / 20 / 10 | `open_request.rs:161`, `os_notify.rs:116` | pure client |
| `build_info.rs` | 60 | either | 0 | none | trivial |

### Seams, in dependency order

1. **Event sink trait.** Replace `Option<AppHandle>` with
   `Option<Arc<dyn EventSink>>` in `harness_action_event.rs`,
   `review.rs`, `review_surface/commands.rs:152`, `agent_api/state.rs`
   (6 emits), the Claude and opencode event managers and
   `verbs/mail.rs`. No dependencies; done in place in `app/src-tauri`.
   *Check:* no `AppHandle` or `Emitter` in those modules; the Tauri
   implementation emits the same seven event names; tests that pass
   `None` or `for_test` pass unchanged.
2. **Stream sink and request/response split.** For the five `Channel`
   commands and the `fs.rs:196` `ipc::Response`. Wrappers hold only
   `State` and a call into an `_impl`; the stream is a trait or closure.
   Depends on 1. *Check:* no non-wrapper file imports `tauri::ipc`; PTY
   spawn and event attach still work.
3. **Path and resource provider.** `setup/state.rs:24,58`,
   `harness_config.rs`, db location, log dir, `spawn_settings`.
   Independent of 1 and 2. *Check:* a test builds the daemon state from
   `Paths { data_dir, resource_dir, log_dir }` with no `tauri::App`.
4. **Crate extraction, leaf modules.** `skein-proto` (DTOs), db,
   `spawn_env`, `spawn_settings`, `agents`, `resume`, `room_paths`,
   `harness_kind`, `watcher`. All but `resume` have 0 Tauri touch points;
   `resume.rs`'s probe functions are Tauri-free, but the file holds 4
   command wrappers and 3 `spawn_blocking` calls: the probe logic moves
   into `skein-daemon` (`spawn_blocking` becomes
   `tokio::task::spawn_blocking`) and the 4 wrappers stay in the app and
   call it. They can start in parallel with 1. *Check:* `cargo tree -p skein-daemon` lists
   no `tauri`.
5. **Engine extraction.** `pty/` (incl. `procscan`), `harness_events_*`,
   `harness_actions_*`, `review`, `review_surface`, `design/serve`, and
   the `fs`/`git` implementations. Depends on 1 to 4. *Check:* the
   daemon crate builds and tests with no `tauri`; app tests pass; the
   moved test count is unchanged.
6. **Agent API and in-process transport.** `agent_api/*`, the command
   wrappers, and a rewrite of `setup/state.rs` + `setup/servers.rs` to
   embed the daemon. Depends on 5. `request_frontend` is its own
   sub-issue. *Check:* the app launches with no behaviour change; the
   `agent_api` suites pass.

### Risks and uncertainties

- **`request_frontend`** (`agent_api/state.rs:466`). Used by
  `room_create.rs`, `harness_control.rs`, `room_close.rs`,
  `room_listing.rs` and `design.rs`. A headless daemon has no webview,
  so these verbs need a client-request channel that tolerates no
  client. The logic behind them (room creation, harness spawn, argv)
  also lives in the frontend (`useNewRoomForm`, `useHarnessCreation`,
  `harnessCmd.ts`).
- **Managed state shared across modules.** `AgentApiEndpoint` and
  `PreviewEndpoint` aliases were not resolved to their definitions. The
  `app.manage` order in `setup/state.rs:92-100` and `servers.rs` implies
  construction dependencies the provider must reproduce.
- **`pty_spawn` blocks up to `PROBE_WAIT` (6 s)** on the login-shell
  probe (`commands/pty.rs:44`, `pty/preview.rs:300`); the managed state
  is not `Arc`-wrapped. The Windows ConPTY waiter thread in
  `pty/mod.rs` was not read.
- **Phase logic is TypeScript** (coupling 17). Phase 1 can leave it in
  the client; Phase 2 and 4 cannot.
  `harness_events_claude/supervise.rs` only partly covers it.
- **Rooms persistence is a wholesale mirror** (`save_all`). Fine for
  one embedded client; wrong for N.
- **The `_impl` split is not uniform.** `git.rs` and `fs.rs` mix wrappers
  and logic; `review.rs:797` takes an `AppHandle`.
- **Binary framing.** `fs.rs:192` and `review_surface/commands.rs:79`
  return raw bytes; the wire format has to carry them.

## 5. Headless terminal emulator

The design says "e.g. alacritty_terminal or vt100". The evaluation
below marks each fact V (checked at a primary source) or C (claimed by
a secondary source, not re-checked).

| Criterion | `vt100` 0.16.2 | `alacritty_terminal` 0.26.0 |
|---|---|---|
| Snapshot as bytes | V: `Screen::state_formatted()` returns escape codes that reproduce `contents_formatted` plus `input_mode_formatted`; also `contents_formatted`, `rows_formatted`, `cursor_state_formatted`, `attributes_formatted` and `*_diff` variants | V (absence): no snapshot or serialise API; walk `Term`/`Grid` cells and emit SGR/CUP yourself |
| Alternate screen | V: `Screen::alternate_screen()`. C: formatted output reflects the active screen | C: separate alt grid in `Term` |
| Scrollback | V: `set_scrollback(rows)` moves the view; `contents_formatted` is visible contents only, so history must be paged out separately or sent as plain text | C: real history in `Grid`, configurable; serialise yourself |
| Truecolor, wide chars | V: fg/bg colour accessors, dim (0.16.0). Open issues: VS16 emoji width, wide-char/small-screen panics, panic in `Screen::text` when narrower than a glyph | C: mature (backs Alacritty). V: 0.26.1-dev fixes unbounded per-cell memory for zero-width cells |
| Resize and reflow | `set_size`, `row_wrapped`; C: no real reflow. V: open issue, `Row::clear_wide` panic after resize; 0.16.2 fixed an out-of-bounds cursor after resize | C: `Term::resize` reflows |
| Latest release | V: 0.16.2, 2025-07-12 | V: 0.26.0, 2026-04-06 |
| Cadence | V: 0.15.2 in Feb 2023, then 0.16.0 to 0.16.2 in Jul 2025; ~13.5M downloads | V: 0.24.2 Jan 2025, 0.25.0 Feb 2025, 0.25.1 Oct 2025, 0.26.0 Apr 2026; ~1.9M downloads |
| Maintainers and issues | single maintainer; last push 2025-07-12; 23 open issues and PRs | the Alacritty project; active; 341 open (whole repo) |
| API stability | small API; 0.16.0 removed `title*`/`bells_diff`/`errors` for `Parser::process_cb` callbacks | V: the changelog marks breaking changes every minor (0.26.0 `ChildEvent::Exited` to `ExitStatus`; 0.25.0 replaced `Options::hold`). Pin an exact version |
| Licence | MIT (V) | Apache-2.0 (V) |
| PTY coupling | none; feed it bytes | own tty and event-loop module (ignorable); `Term` generic over a listener; heavier |

Alternatives. `shpool_vt100` is a vt100 fork (2025-02-25, V); C: shpool
uses it for attach-restore, unconfirmed. `avt` 0.18.0 (2026-05-05,
Apache-2.0, asciinema's emulator, primary and alt buffers, V; its
dump-as-escape-codes API was not checked). `wezterm-term` is not on
crates.io, git only. `@xterm/headless` with `@xterm/addon-serialize`
would add a Node process next to the Rust daemon.

**Recommendation: `vt100`, or a fork of it, behind a trait.**

- `state_formatted` gives snapshot-then-stream without an owned
  serialiser; alacritty would need one written and tested.
- No PTY or event-loop coupling, MIT, small API, less churn.
- xterm.js is the real renderer. The daemon grid only has to recreate
  the screen. Reflow matters less because the daemon can resize the PTY
  and let the TUI redraw.
- The trait keeps the choice reversible: `feed(bytes)`, `resize`,
  `snapshot() -> bytes`.

Risks:

- **Scrollback gap.** `state_formatted` has no history. Needs a
  separate pass, or visible-screen-only attach.
- **Maintenance.** Open issues include HVP/HPA/REP, mode 2026
  (synchronized output) and unicode width. Ink-style redraws may hit
  them. Plan to vendor or fork.
- **Panics on odd resizes.** Wrap the parser in `catch_unwind`.
- **Bytes lost between snapshot and stream.** Take the snapshot and
  subscribe under one lock.
- **Unvalidated against Claude Code's TUI.**

**Proposed acceptance test.** Record a real Claude Code session's PTY
byte stream, with resizes and the alternate screen. Replay it into
`vt100` and into xterm.js. Compare the daemon's snapshot with xterm.js's
buffer at several cut points (after resize, in the alternate screen,
mid-redraw). Gate the Phase 2 snapshot work on that comparison.

Sources:

- https://crates.io/crates/vt100
- https://docs.rs/vt100/0.16.2/vt100/struct.Screen.html
- https://raw.githubusercontent.com/doy/vt100-rust/main/CHANGELOG.md
- https://github.com/doy/vt100-rust/issues
- https://crates.io/crates/alacritty_terminal
- https://raw.githubusercontent.com/alacritty/alacritty/master/alacritty_terminal/CHANGELOG.md
- https://docs.rs/alacritty_terminal
- https://crates.io/crates/avt
- https://github.com/asciinema/avt
- https://crates.io/crates/shpool_vt100

## 6. Backlog and prior art

No parked idea in `docs/backlog.md` covers a daemon, headless backend,
detached or remote harnesses, ssh/tmux, a mobile or web client, push
notifications, approvals while away, or multi-machine use. The nearest
items:

- Room guardrail presets and sandboxed rooms
  (`docs/backlog.md:63-69`): attended, unattended and review-only
  presets, and a long-range kernel sandbox confined to the worktree.
  The only sandbox item; relevant to Phase 5.
- Notification hooks (`docs/backlog.md:60-62`): a user shell command per
  event as a Slack or webhook escape hatch. Closest to push, but
  local-only as written.
- Remote release policy (`docs/backlog.md:53-59`): an app-update policy
  JSON. It is about app updates, not remote harnesses.
- BYOH permission UX (`docs/backlog.md:27-28`): each harness handles its
  own permission flow. Bears on the approval queue.

Other docs that touch the area:

- `docs/grok-build-recon-2026-07-16.md:146`: strip `TMUX` markers so
  Skein inside tmux does not leak into harness detection.
- `docs/epic-50-l2c-2-recon.md:27,233`: `opencode serve` is a headless
  opencode server, and a remote opencode is reachable via
  `opencode attach <url>`.
- `docs/design-surface-recon.md:82,406`: headless Chrome, and a daemon
  bound to a host.

## 7. Corrections to the handover

1. **Hooks are `curl` in `hooks.json`, not `cli_shim.rs`.** The file is
   `app/src-tauri/harness-config/claude-plugin/hooks/hooks.json`. It has
   three hook events (SessionStart, SessionEnd, PermissionRequest) and
   four commands, in `sh` syntax. They run on the harness host and must
   reach the agent API URL. `cli_shim.rs` installs the `skein` command
   for opening folders and has nothing to do with hooks.
2. **Coupling 3 is bidirectional.** The agent API server also emits into
   the webview and blocks on its answers (`request_frontend`). Without a
   webview, `create_room`, `open_harness`, `close_harness`, `close_room`,
   `list_rooms` and the design verbs fail.
3. **"Transcript tailing" is wider than the JSONL file.** It also
   covers Claude subagent sidecars, background-task `.output` trailers,
   `opencode.db` reads, and the `~/.claude` existence and stat probes.
4. **The frontend owns phase, argv, ports and event logging.** The
   phase machine, delegation deferral, mail nudges, `cmdForKind` and
   `resumeCmd`, the opencode port, and the `harness_events` writes are
   all in the webview.
5. **Tokens are revoked on every boot** (`setup/state.rs:87`), on the
   assumption that PTYs die with the app. A daemon that outlives its
   clients breaks that assumption.
6. **Injected paths are host-bound.** `--plugin-dir` and
   `OPENCODE_CONFIG` are resource paths on the app's machine. A remote
   daemon needs its own copy of the bundle at a path it can resolve.
7. **"Skein performs no git mutations" needs a gloss.**
   `git_add_worktree` and `git_restore_worktree` already exist for room
   creation. The standing decision is about commit, merge, push and
   pull request. The daemon inherits exactly today's set and adds
   none.
8. **CLAUDE.md is stale about `fs.rs` scoping (#174).** It says paths
   are not yet scoped to the room. The code scopes all four commands
   through `ensure_room_scope` (`app/src-tauri/src/fs.rs:68`).
9. **The login-shell probe captures only `PATH` on this commit.** #565,
   in flight elsewhere, extends it to the full login-shell env. Either
   way it runs wherever the daemon runs, so it probes the daemon host,
   not the client.
10. **Design preview and folder pickers are client-machine concepts.**
    The preview URL is the webview's loopback, and `plugin-dialog`
    returns a client path. Neither survives a remote daemon unchanged.
11. **The agent API has 25 verbs, not twenty-two.** See §3c.
12. **Rooms persist by wholesale mirror**, which the handover's "sqlite
    state lives beside the app" does not capture. It matters as soon as a
    second client exists.

## 8. New open questions

1. **Where does the phase machine live once the daemon is unattended?**
   Port it to Rust, or run a headless client that hosts the TypeScript.
   Permission state, deferral timers, held mail, notifications and the
   approval queue (Phase 4) depend on the answer.
2. **Who builds argv and allocates ports?** The daemon must, on its own
   host. Per-kind defaults from `prefs.ts` (localStorage) are an input;
   where do they live?
3. **What replaces `request_frontend` when no client is attached?**
   Daemon-owned room creation and harness open/close, or a request
   queue that waits for a client. `create_room` today resolves
   `(kind, agent)` through the webview.
4. **Per-room mutations or `save_all` with N clients?** A wholesale
   mirror cannot stay. What is the unit of write, and how does a client
   learn another client's change?
5. **Token lifetime across daemon restarts.** A harness that outlives a
   restart holds a token that is revoked today. Keep tokens, re-issue
   them, or revoke only for harnesses that did not survive?
6. **Design preview reachability.** Proxy the preview HTTP through the
   daemon connection, or give the iframe a daemon-served URL? The
   `Origin` rule and the null CSP both interact with this.
7. **Folder picking for remote rooms.** `list_workspaces` only, or a
   general daemon-side directory browser? `default_cwd` has the same
   question.
8. **Global todos with several daemons.**
   `db_load_global_todos`/`db_save_global_todos` are neither per-room nor
   per-daemon. Client-local, or a designated home daemon?
9. **Binary payload framing.** `read_image_bytes` and
   `review_image_bytes` return raw bytes today. Does the wire carry
   binary frames, or base64 in JSON?
10. **The webview-originated `review-changed` event**
    (`app/src/review/api.ts:260`). With a daemon the emitter must be a
    daemon event, or other clients never see it.
11. **Harness-config bundle on a remote host.** How is it shipped,
    versioned and matched to the client's expectations, given the hooks
    and the MCP config are host-side files?
12. **Does worktree create/restore stay in the daemon for remote
    rooms?** The room/workspace contract says the main checkout already
    exists on the host. `git_add_worktree` and `git_restore_worktree`
    are the only writes; are they inside that contract?
13. **Hook and MCP URL reachability under non-host runtimes.** Hooks run
    on the harness host and need the agent API URL. Under an ssh/tmux or
    sandbox runtime the harness may not share the daemon's loopback.
