# Crates: the server side leaves the app (#580)

Phase 1d of the skeind extraction (epic #575, docs/skeind-recon.md, #580)
moved the Tauri-free leaf modules out of `app/src-tauri` into two new
workspace crates. Phase 1e part 1 (#581) moved the next layer: files, git,
the room load, perf, the design server and the review surface. Part 2 of
#581 moved the engines: the PTY manager and the harness event and action
adapters. Behaviour is unchanged: the app re-exports every moved module at
its old path where it still imports it, so no importer changed.

Figure: crate layout after #581 part 2, arrows pointing from a crate to
what it depends on, with what each of the two sides holds. Renders the
`[dependencies]` path entries of each `Cargo.toml` and the module lists
below; non-normative, the manifests and the text are authoritative.

```mermaid
flowchart TD
    subgraph OUT["Outside the workspace"]
        APP["app/src-tauri"]
        STAYS["Stays in the app: agent_api for #582, opencode_sidecar/exit_guard, terminal_paths, build_info, the seam impls, shared_state, client-only files"]
        WRAP["Tauri wrappers: commands/*, resume, design/commands, review/commands, review_surface/commands, hermes_profile/commands, harness_actions_hermes"]
    end
    subgraph WS["Cargo workspace"]
        DAEMON["skein-daemon"]
        MOVED1["Moved in #580: db, spawn_env, spawn_settings, watcher, resume probes, seams"]
        MOVED2["Moved in #581 part 1: fs, git, rooms, perf_hist, perf_spans, design, review, review_surface"]
        MOVED3["Moved in #581 part 2: pty, harness_config, agents, harness_events, harness_actions, harness_work_hermes, hermes_profile, opencode_sidecar"]
        PROTO["skein-proto"]
        GIT["skein-git"]
        HARNESS["skein-harness"]
        REVIEW["skein-review"]
        PROCPRIO["skein-procprio"]
        SYSLOAD["skein-sysload"]
        WEBSHOT["skein-webshot"]
        WINNOTIFY["skein-winnotify"]
    end
    APP --> DAEMON
    APP --> GIT
    APP --> HARNESS
    APP --> REVIEW
    APP --> PROCPRIO
    APP --> SYSLOAD
    APP --> WEBSHOT
    APP --> WINNOTIFY
    DAEMON --> PROTO
    DAEMON --> HARNESS
    DAEMON --> GIT
    DAEMON --> REVIEW
    DAEMON --> PROCPRIO
    DAEMON --- MOVED1
    DAEMON --- MOVED2
    DAEMON --- MOVED3
    APP --- WRAP
    APP --- STAYS
```

## The two crates and the rule

- `crates/skein-proto` is wire types: plain data that skeind and its
  clients both need to name. serde (and serde_json) only, no logic. It has
  `PROTO_VERSION: u32 = 1`, the `Room`/`Harness` family (`rooms`),
  `HarnessEvent`/`HarnessAction` (`harness_log`), `HarnessKind`
  (`harness_kind`), `PendingDto`/`TranscriptStatDto` (`resume`) and
  `DropReason` (`spawn_env`). There is no `hello` handshake (#584) and no
  wire framing; framing waits for the transport phase (#585/#582, open
  question 9 in skeind-recon.md).
- `crates/skein-daemon` is the Tauri-free server side. `cargo tree -p
  skein-daemon` lists no tauri. It depends on skein-proto, skein-harness
  skein-git, skein-review and skein-procprio. Part 2 added portable-pty,
sysinfo, listeners, reqwest, rustls and futures-util.

The rule: a type that crosses the wire and carries no behaviour goes in
skein-proto. Code that runs on the server and needs no Tauri goes in
skein-daemon. Anything that touches `tauri::*` stays in the app.

## What moved into skein-daemon

`db/` (the sqlite layer), `spawn_env` (+ `login_env`), `spawn_settings`
(+ `env_profile`, `priority`), `tool_groups` (was `agent_api/groups.rs`),
`watcher`, `ignore_filter`, `worktree_key` (`relative_key`, was in
`review.rs`), `room_paths`, `resume` (the session-probe logic) and
`harness_kind` (re-exported from skein-proto). With them go the three
seams from phases 1a to 1c: `event_sink`, `stream_sink` and `paths`.

### Added by #581 part 1

- `fs`, `git`: the `_impl` logic and DTOs. The Tauri wrappers stay in the
  app's `commands/fs.rs` and `commands/git.rs`.
- `rooms`: the `load_rooms` logic; the wrapper stays in `commands/rooms.rs`
  (`todos.rs` is unchanged).
- `perf_hist`, and `perf_spans` (`SpanAgg`, `SpanSnapshot`,
  `REVIEW_DISCOVERY_BATCH`). The app's `perf_spans.rs` keeps the other
  statics and the drain.
- `design/`: `serve`, `rewrite`, `device`, `change_filter`, `entries` (the
  logic split out of the old `design/commands.rs`) and the `.js` shims. The
  app keeps `design/mod.rs` (a glob re-export) and `design/commands.rs`
  (the Tauri wrappers).
- `review` and `review/discovery`: baselines and discovery.
  `REVIEW_CHANGED_EVENT` and `ReviewChanged` are defined in
  `skein_daemon::review`; `agent_api/state.rs` re-exports them. The app
  keeps `review.rs` and `review/commands.rs` (4 Tauri wrappers).
- `review_surface` (the root is `review_surface.rs`) and everything under
  it except `commands.rs`. The app keeps `review_surface.rs` (a glob
  re-export), `review_surface/commands.rs` (15 Tauri commands) and
  `element_agent_tests.rs`, a test that needs `agent_api`.

### Added by #581 part 2

- `pty/`: `PtyManager`, `procscan`, `priority` and the new `pty/opencode.rs`.
  The Tauri wrappers stay in `commands/pty.rs`.
- `harness_config`, `agents`, `harness_action_event`,
  `harness_actions_claude`, `harness_actions_opencode`,
  `harness_events_claude/`, `harness_events_opencode` (and its directory)
  and `harness_work_hermes`.
- `harness_actions_hermes`: the tail logic. The app keeps a file with its 3
  Tauri commands.
- `hermes_profile/`: the logic, with `entries.rs` split out of the old
  commands. The app keeps `hermes_profile/commands.rs` (3 Tauri commands).
- `opencode_sidecar/` including the new `start.rs`. The app keeps
  `exit_guard.rs`, which implements `tauri::Resource`, and the start
  command loop in `commands/opencode_sidecar.rs`, because
  `start_attempt`, `StartGuard` and `Ready` hold an `AppHandle`.
- Small new modules: `agent_endpoint` (`HarnessIdentity`,
  `AgentApiEndpoint`, `AgentApiStatus`; `agent_api/state.rs` re-exports
  them), `spawn_env/state.rs` (`SpawnEnvState`, `home_dir`, payload),
  `spawn_env/lookups.rs`, `tls` (`install_rustls_provider`), `build_info`
  (`set_version`/`version`) and `harness_log` (context sizes).
- `build_info`: the version comes from the app's `build.rs`
  (`SKEIN_VERSION`), so the app passes it once at the top of `run()`.
- `perf_spans` gained `PTY_WRITE` and `PTY_CHUNK_SEND`; `resume` gained
  `profile_db`.

## What stayed in the app, and why

- Every `#[tauri::command]` wrapper: `commands/*`, `resume.rs`,
  `review/commands.rs`, `review_surface/commands.rs`, `design/commands.rs`,
  `hermes_profile/commands.rs` and `harness_actions_hermes.rs`.
- `agent_api/`, which moves with the transport in #582.
- `opencode_sidecar/exit_guard.rs`, which implements `tauri::Resource`.
- `terminal_paths.rs` and `build_info.rs` (the app side of the version
  hand-off).
- Client-only files: `open_request`, `os_notify`, `cli_shim`, `setup/*`,
  `updater_proxy`, `perf_cmds` and the other perf files.
- The Tauri impls of the seams: `tauri_event_sink.rs`,
  `tauri_stream_sink.rs`, `tauri_paths.rs`.
- `shared_state.rs`, which builds the state from a `Paths` and an event
  sink.

## Re-export pattern

`app/src-tauri/src/lib.rs` declares each moved module as
`pub(crate) use skein_daemon::db;` (and the same for the others), and
`agent_api` re-exports `tool_groups` as `agent_api::groups`. Importers keep
their old paths. The app stays excluded from the workspace, so it is still
checked and tested through its own `--manifest-path`.

## Lints and the `test-support` feature

skein-daemon has its own `[lints]` table rather than `workspace = true`.
The values match the workspace (`unsafe_code = "forbid"`, clippy
pedantic) plus `missing_errors_doc` and `missing_panics_doc` allowed,
because this is app code moved as-is. The `test-support` cargo feature
exposes `db::test_support`; the app enables it as a dev-dependency so its
tests can build databases.

## Next

- #582: the agent API and the in-process transport. `agent_api/` is the
  last large piece of server logic still in the app.
