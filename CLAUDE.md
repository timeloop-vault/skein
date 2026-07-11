# CLAUDE.md

Skein is a prototype IDE for driving AI coding agents, built around the
design in `docs/design/`. A **room** (top-level tab) is a task backed by
its own git worktree; each room owns N **harnesses** — AI coding CLIs
(Claude Code, opencode, a stub gh-copilot, or a plain shell) running in
real PTYs, all sharing the room's worktree. "Session" in Skein
vocabulary means only one thing: the harness tool's own conversation id
(`Harness.sessionId`), used to resume conversations across restarts.

Skein is daily-driven on macOS and Windows via auto-updating v0.2.x
releases. The original 8-chapter build plan is complete; work is now
driven by GitHub issues. The de facto roadmap is
`docs/audit-2026-07-03.md` §5 (order of attack) — headline arcs: #19
App.tsx split, #116 harness-adapter consolidation, #49 files pillar,
#52/#106 review surface, #76 cross-room view.

## Stack

- **Tauri v2** desktop shell (Rust, edition 2024); plugins: updater,
  dialog, clipboard-manager, opener, notifications (the notifications
  plugin is skipped in macOS debug builds — its Swift bridge needs a
  real .app bundle)
- **React 18 + strict TypeScript** UI (Vite); xterm.js for terminals,
  react-virtuoso for the activity feed
- **Biome** lint + format (frontend), **clippy pedantic** `-D warnings`
  (Rust)
- Pre-commit hook in `.githooks/pre-commit` (activate:
  `git config core.hooksPath .githooks`) runs, in order: cargo fmt,
  clippy, and tests for BOTH the workspace and `app/src-tauri`
  (excluded from the workspace — `cargo test --workspace` does NOT
  reach its ~100 unit tests; #168), then tsc and biome.

## Layout

    skein/
    ├── crates/skein-git/            # Pure-Rust libgit2 wrapper. Tauri-free, sync, local-only
    │   └── src/lib.rs               # Repo: open, branches, head_branch, add_worktree,
    │                                #   list/remove_worktree, status, diff_workdir;
    │                                #   propose_worktree_path → sibling dir <repo>-wt/<slug>.
    │                                #   No clone/fetch/push/commit yet (#182 adds writes)
    ├── app/
    │   ├── src/                     # React + TS UI
    │   │   ├── App.tsx              # The single React tree (~2.9k LOC hotspot; #19 tracks
    │   │   │                        #   the split): rooms/harness tabs, boot resume,
    │   │   │                        #   notifications, palette items, DnD, keydown dispatch
    │   │   ├── LiveTerminal.tsx     # xterm.js ↔ PTY binding; spawns/kills on (cmd, spawnGen)
    │   │   │                        #   mountKey; attaches the L2c event adapters
    │   │   ├── harnessActivity.ts   # Source of truth for harness phase (spawning/running/
    │   │   │                        #   idle/waiting/exited); L2a idle heuristic + L2b
    │   │   │                        #   patterns + L2c authoritative adapters
    │   │   ├── harnessEvents.ts     # L2c translators: ClaudeEvent/OpencodeEvent → phase calls
    │   │   ├── harnessPatterns.ts   # L2b fallback regexes (copilot/shell waiting prompts)
    │   │   ├── data.tsx             # HARNESS_KINDS registry (chip/label per kind) — small
    │   │   │                        #   but load-bearing
    │   │   ├── types.ts             # Room / Harness / Status vocabulary
    │   │   ├── components.tsx       # Shared atoms (HChip, StatusDot, tabs, picker)
    │   │   ├── shortcuts.ts         # ALL keyboard shortcuts: one platform-agnostic BINDINGS
    │   │   │                        #   table (#151); LiveTerminal swallows via isAppShortcut
    │   │   ├── SettingsModal.tsx    # Settings + in-app updater UI
    │   │   ├── ReopenRoomModal.tsx  # Archived-rooms browser (close = archive, never delete)
    │   │   ├── CommandPalette.tsx / statusPopover.ts / Splitter.tsx /
    │   │   │   useFocusRestore.ts / prefs.ts (localStorage UI prefs) / styles.css
    │   │   └── liveContext/         # Right-pane card stack (issue #80): store.ts (backfill
    │   │                            #   500 + live tail of harness-action events),
    │   │                            #   LiveContext.tsx + CardStack (chrome, per-room layout),
    │   │                            #   ActivityCard/feedItems/rows/toolRows (feed),
    │   │                            #   PlanCard/plan.ts (todo reducer), DiffCard/diff.ts +
    │   │                            #   useWorktreeDiff (git diff w/ harness-patch fallback),
    │   │                            #   payload.ts (all payload-shape divergence lives here)
    │   └── src-tauri/               # Tauri Rust shell
    │       ├── src/lib.rs           # Builder + 36-command registry; tracing → daily-rotating
    │       │                        #   file in app_log_dir() + stderr (RUST_LOG overrides)
    │       ├── src/pty.rs           # PtyManager (portable-pty); 2 threads per spawn (reader +
    │       │                        #   waiter — the waiter is load-bearing on Windows ConPTY)
    │       ├── src/git.rs           # DTO wrappers around skein-git; GitError → String
    │       ├── src/watcher.rs       # notify-debouncer-mini, 200 ms, .git/ deliberately unfiltered
    │       ├── src/db.rs            # rusqlite: rooms (table `sessions` — legacy name),
    │       │                        #   harness_events, harness_actions, sessions_quarantine;
    │       │                        #   WAL + .bak/.bak.1 snapshots (#167)
    │       ├── src/fs.rs            # dir listing + file previews (currently no frontend callers;
    │       │                        #   #49 revives, #174 scopes)
    │       ├── src/resume.rs        # session-existence probes against the tools' own stores
    │       ├── src/harness_events_claude.rs    # JSONL tail → ClaudeEvent (L2c-1)
    │       ├── src/harness_events_opencode.rs  # SSE client → OpencodeEvent (L2c-2)
    │       ├── src/harness_actions_claude.rs   # JSONL → harness_actions rows (#80)
    │       ├── src/harness_actions_opencode.rs # SSE/opencode.db → harness_actions rows (#80)
    │       └── src/harness_action_event.rs     # "harness-action" live broadcast (live rows only)
    ├── docs/
    │   ├── audit-2026-07-03.md      # Full-codebase audit; §5 = the current roadmap
    │   ├── backlog.md               # Parked ideas (read before adding to any plan)
    │   ├── live-context-*.md        # Live Context specs — authority chain in "Design refs" below
    │   ├── working-prototype-plan.md, chapter-*-plan.md, *-recon.md, epic-50-*
    │   │                            # HISTORICAL records of shipped chapters 1–8 + epic #50.
    │   │                            #   Useful for "why", wrong about "what is"
    │   └── design/                  # Design handoff bundles (read-only reference)
    ├── scripts/local-bundle.sh      # macOS bundled-.app smoke test (NOTE: uses the RELEASE
    │                                #   identifier, unlike tauri:build:local)
    ├── .githooks/pre-commit         # The de facto gate (CI is manual-trigger only)
    └── .github/workflows/           # ci.yml (workflow_dispatch only), release.yml (tag-driven
                                     #   3-OS matrix + updater latest.json)

## Data flow

- **Rooms** persist in sqlite at `<APP_DATA>/skein.db`, one row per
  room, the whole Room as a camelCase JSON blob. `App.tsx` hydrates
  once on mount (`db_load_rooms`); every `rooms` state change after a
  successful load mirrors back wholesale (`db_save_rooms`, wipe +
  re-insert in one transaction). **#167 hardening:** unparseable rows
  are quarantined to `sessions_quarantine` (never silently dropped);
  a wholesale load failure parks the autosave and shows a retry card;
  `save_all` refuses an empty commit before a successful load; WAL +
  `skein.db.bak` / `.bak.1` last-known-good snapshots rotate on the
  first clean non-empty load per process. **Field policy: every field
  added to Room/Harness after v0.2.5 MUST be `#[serde(default)]` or
  `Option`** — a required field makes old blobs unparseable.
- **PTYs** live in `PtyManager`. `pty_spawn` returns an opaque id;
  output streams over a per-spawn `tauri::ipc::Channel<PtyEvent>` — a
  tagged enum `{kind:"data",chunk}` / `{kind:"exit",code}`. PTYs
  survive room/harness tab switches: all active rooms and all their
  harnesses stay mounted, hidden with `display:none`. PTYs die with
  the Skein process; on boot the hydrate path drops stale sessionIds
  (`resume.rs` probes the tools' own stores), pre-allocates fresh
  opencode ports, and rewrites each stored cmd to its resume form
  (`resumeCmd`: `claude --resume <sid>`,
  `opencode --port <p> --hostname 127.0.0.1 --session <sid>`).
  Reopening an archived room runs the same rewrite (#153).
- **Watchers** (`WatcherManager`, notify-debouncer-mini, 200 ms,
  `.git/` deliberately unfiltered) push `()` over a Channel; the
  active room's Live Context re-fetches `git_diff` and the status-bar
  branch on each tick.
- **Harness telemetry** (epic #50 + issue #80): Claude Code — Skein
  pre-allocates the session uuid (`--session-id`), tails
  `~/.claude/projects/<encoded-cwd>/<sid>.jsonl` (50 ms debounce);
  opencode — Skein pins `--port` and subscribes to the embedded
  server's `/event` SSE stream. Each source feeds two consumers:
  (a) semantic events → `harnessEvents.ts` → the `harnessActivity`
  phase store (single state machine; transitions are logged to
  `harness_events` and drive badges/toasts/OS notifications), and
  (b) action extraction → `harness_actions` rows in sqlite + a global
  `"harness-action"` Tauri event for live rows only. The Live Context
  store backfills the newest 500 rows per room and appends live ones;
  Diff/Plan/Activity cards all render from that one array.
- **Git ops** go through `crates/skein-git`;
  `app/src-tauri/src/git.rs` is a thin DTO layer. Anything richer
  than DTO glue belongs in the crate, where it's testable.

## Running it

    # First time:
    git config core.hooksPath .githooks
    cd app && npm install

Three build profiles (issue #21), each with its own bundle identifier
so APP_DATA / logs / config never bleed between them:

| Profile       | Command                     | Identifier                       | Label         |
| ------------- | --------------------------- | -------------------------------- | ------------- |
| dev           | `npm run tauri:dev`         | `com.timeloop-vault.skein.dev`   | Skein (dev)   |
| local release | `npm run tauri:build:local` | `com.timeloop-vault.skein.local` | Skein (local) |
| release       | `npm run tauri build`       | `com.timeloop-vault.skein`       | Skein         |

- **dev** — debug build, hot-reload, devtools; day-to-day feature work.
- **local release** — optimized bundle beside the daily driver without
  touching its state.
- **release** — what the GitHub pipeline ships. Releases are cut by
  publishing a GitHub Release with a `vX.Y.Z` tag; `release.yml`
  builds macOS (Apple Silicon only) / Windows / Linux, attaches
  installers + the updater's `latest.json`, and patches the version
  into the build. The `0.1.0` version strings in tauri.conf.json /
  package.json / Cargo.toml are placeholders — do NOT "fix" them.
  Builds are OS-unsigned; the in-app updater verifies its own
  minisign signature (Settings → About → Check for updates).
- `scripts/local-bundle.sh` (macOS) builds and Finder-launches a
  bundled .app for reproducing installed-app-only bugs — note it uses
  the *release* identifier, so it shares the daily driver's state.

App data dirs on Windows:
`%APPDATA%\com.timeloop-vault.skein\` (release),
`%APPDATA%\com.timeloop-vault.skein.dev\` (dev),
`%APPDATA%\com.timeloop-vault.skein.local\` (local release).
To reset persisted state for a profile, delete every `skein.db*`
file in its dir — `skein.db`, the WAL sidecars `skein.db-wal` /
`skein.db-shm`, and the `skein.db.bak` / `.bak.1` snapshots (#167).
Deleting `skein.db` alone can pair a stale WAL with the fresh file
and corrupt it.

Rust logs: daily-rotating `skein.log.*` in the profile's log dir +
stderr; `RUST_LOG` overrides the default `info` filter.

## Conventions

- **Rust:** edition 2024. `unsafe_code = "forbid"`. Clippy pedantic
  warn, `-D warnings`. Tauri commands collapse `GitError` / `PtyError`
  to `String` at the boundary — they round-trip via JSON anyway.
- **TS:** strict, `noUncheckedIndexedAccess`,
  `exactOptionalPropertyTypes`. Biome with tabs + double quotes — run
  it from `app/` (`cd app && npx biome check .`), never from the repo
  root with a path argument.
- **Tests live with the code that owns them.**
  `crates/skein-git/tests/` has 18 integration tests against tempfile
  repos. `app/src-tauri` has ~100 in-module unit tests (harness
  JSONL/SSE parsers, db persistence). Both run in the hook and CI;
  remember `cargo test --workspace` alone does NOT cover the tauri
  crate (#168). The frontend has no test infra yet — #169 tracks it.
- **Issues drive the work.** Commit messages name the issue
  (`fix(#158): …`). The chapter/phase system ended with chapter 8;
  plan docs are history, not instructions. Parked ideas live in
  [`docs/backlog.md`](docs/backlog.md) — read it before adding to any
  plan; things there move to GitHub issues when they become real.
- **Keyboard shortcuts** only via the `BINDINGS` table in
  `shortcuts.ts`; prefer letters/digits (Swedish layout — AltGr =
  Ctrl+Alt collides with punctuation chords) and agree bindings with
  Stefan before committing (#150 tracks user rebinding).

## Current state (2026-07)

Chapters 1–8 all shipped: real PTYs and worktrees, sqlite-persisted
rooms with archive/reopen, harness conversation resume across
restarts, the Live Context right pane (activity/plan/diff cards fed
by harness telemetry), notifications (badge/toast/OS), Windows +
Linux support, keyboard-driven navigation, and distribution with
in-app auto-update. v0.2.5 is the latest release.

A full-codebase audit (2026-07-03) produced the current roadmap —
read `docs/audit-2026-07-03.md` §5 before picking up work. Landed
since: #167 (boot-wipe data-loss fix + persistence hardening), #168
(test gates). The known-weak spots it names: App.tsx size/duplication
(#19), the duplicated Claude/opencode adapter pairs (#116), heavy
sync Tauri commands on the main thread (#171/#172/#178/#179), and
silent failure surfacing (#176).

## Design references

For UI conventions the design archive in `docs/design/` is the source
of truth — start with `docs/design/skein/project/Skein Prototype.html`
and the chat transcript in `docs/design/skein/chats/chat1.md`.

For the right-pane **Live Context** stack (issue #80), the authority
chain from most to least binding:

1. Shipped code in `app/src/liveContext/` (`payload.ts`, `rows.tsx`,
   `toolRows.tsx`, `store.ts`)
2. `docs/live-context-d2-buildmap.md` — payload shapes and kind
   dispatch. **The #1 trap:** the handover's row-catalogue labels
   (`edit`, `read`, `bash`, `todowrite`, …) are NOT backend `kind`
   values; real dispatch is `row.kind` (`tool_call` / `patch` /
   `plan_change` / …) then `payload.tool`, with
   `payload.is_error === true` short-circuiting to the error row. A
   naive `switch(row.kind)` on handover labels matches nothing.
3. `docs/live-context-handover.md` — design/UX intent, row treatments
4. `docs/live-context-design-brief.md` (rationale) and
   `docs/live-context-recon.md` (raw payload provenance) — historical
