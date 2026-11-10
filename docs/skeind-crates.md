# Crates: the server side leaves the app (#580)

Phase 1d of the skeind extraction (epic #575, docs/skeind-recon.md) moves
the Tauri-free leaf modules out of `app/src-tauri` into two new workspace
crates. Behaviour is unchanged: the app re-exports every moved module at
its old path, so no importer changed.

Figure: crate layout after #580, arrows pointing from a crate to what it
depends on. Renders the `[dependencies]` path entries of each
`Cargo.toml`; non-normative, the manifests are authoritative.

```mermaid
flowchart TD
    subgraph OUT["Outside the workspace"]
        APP["app/src-tauri"]
    end
    subgraph WS["Cargo workspace"]
        DAEMON["skein-daemon"]
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
  and skein-git.

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

## What stayed in the app, and why

- `agents.rs`: Tauri-free, but it needs `harness_config` (which needs
  `agent_api::state::HarnessIdentity` and `hermes_profile`) and
  `pty::harness_program_lookup`. It moves with the engines (#581/#582).
- `resume.rs`: the 6 `#[tauri::command]` wrappers and `profile_db` stay;
  they call the probe logic in skein-daemon.
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

- #581: the engines (`harness_events_*`, `harness_actions_*`, `review`,
  `review_surface`, and so on) move into skein-daemon.
- #582: the agent API and the in-process transport; `agents.rs` moves with
  this step or the one before it.
