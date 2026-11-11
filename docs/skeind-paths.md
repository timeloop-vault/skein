# Paths: where server-side state lives (#579)

A daemon has no `tauri::App` (#575), so the server-side setup code
cannot ask Tauri where the app data or resource directory is. It reads a
`Paths` value instead. The Tauri build fills it from the Tauri path API;
a daemon will build its own. Behaviour is unchanged.

Figure: where each server-side path comes from and who uses it. Renders
`paths.rs` and `daemon.rs` in `crates/skein-daemon/src`, and
`tauri_paths.rs` and `setup/embed.rs` in `app/src-tauri/src`;
non-normative, the code is authoritative.

```mermaid
flowchart LR
    A["Tauri path API (app_data_dir, resource_dir)"] --> T["tauri_paths.rs"]
    T --> P["Paths (paths.rs, Tauri-free)"]
    P --> S["spawn settings: settings.json"]
    P --> Q["probe spool files"]
    P --> D["skein.db"]
    P --> O["opencode sidecar records"]
    P --> H["harness config bundle (--plugin-dir, OPENCODE_CONFIG)"]
    X["Daemon builds Paths itself (#580+, bundle location #585)"] -.-> P
    C["Still on app.path(): setup/logging.rs (log dir), os_notify.rs (notify icon), updater_proxy/commands.rs"]
```

`Paths` carries `data_dir` (fatal if unresolvable) and
`resource_dir: Result<PathBuf, String>` (never fatal: the error text
becomes `HarnessConfig::unavailable`). `Daemon::build` (`daemon.rs` in
skein-daemon, #582; it replaced the app's `shared_state.rs` and
`setup/state.rs`) builds the state from a `Paths` and an in-process
transport with no Tauri types, so a test can do it in a temp dir. The login-shell probe is a `Probe` parameter,
so tests pass `Probe::Skip`. The log dir is deliberately not in `Paths`:
logging is a client concern.

## Construction order in `setup()`

1. `Paths` from the Tauri app.
2. Spawn env (creates the data dir, loads settings), then the login-shell
   probe.
3. Updater proxy init (client side, reads `paths.data_dir`).
4. Harness config, from `paths.resource_dir`.
5. Database, then the managers on it and the event sink.
6. Agent API server (`AgentApiState` needs the database; it holds no
   `AppHandle` since #582).
7. Design preview (`PreviewState` needs the database).
8. Manage the database.
9. Window, open request, menu, perf.

Step 2 is `daemon::spawn_env`; steps 4 to 7 are `Daemon::build`, which
returns the two server futures instead of spawning them. `setup/embed.rs`
calls both, manages each piece (the database last) and spawns the
listeners. The order is also written in the module doc of `setup.rs`.
