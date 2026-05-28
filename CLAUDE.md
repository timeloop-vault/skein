# CLAUDE.md

Skein is a prototype IDE built around the design in `docs/design/`. It
tests whether the "session-first, multi-harness, shared-worktree" model
holds up when wired into a Tauri + React desktop app with real PTYs and
real git operations.

## Stack

- **Tauri v2** desktop shell (Rust, edition 2024)
- **React 18 + strict TypeScript** for the UI
- **Vite** dev server / bundler
- **Biome** lint + format on the frontend
- **clippy pedantic** with `-D warnings` on the Rust side
- Pre-commit hook in `.githooks/pre-commit` enforces all four (cargo
  fmt, cargo clippy, tsc, biome) plus `cargo test --workspace`

## Layout

    skein/
    ├── crates/skein-git/             # Pure-Rust libgit2 wrapper. Tauri-free.
    │   └── src/lib.rs                # Repo: open, branches, add_worktree,
    │                                 #       status, diff_workdir
    ├── app/
    │   ├── src/                      # React + TS UI
    │   │   ├── App.tsx               # Main tree (sessions, harnesses, panes)
    │   │   ├── LiveTerminal.tsx      # xterm.js + Tauri PTY binding
    │   │   ├── LiveStatus.tsx        # Real-time worktree status + diff view
    │   │   ├── components.tsx        # Shared atoms (HChip, StatusDot, …)
    │   │   ├── data.tsx              # (going away in chapter 2)
    │   │   ├── types.ts
    │   │   └── styles.css
    │   └── src-tauri/                # Tauri Rust shell
    │       ├── src/lib.rs            # Builder + command registry
    │       ├── src/pty.rs            # PtyManager (portable-pty)
    │       ├── src/git.rs            # Tauri wrappers around skein-git
    │       ├── src/watcher.rs        # Filesystem watcher (notify-debouncer-mini)
    │       └── src/db.rs             # rusqlite session persistence
    ├── docs/
    │   ├── design/                   # Design handoff bundles (read-only ref)
    │   ├── working-prototype-plan.md # Chapter 1 (complete)
    │   ├── chapter-2-plan.md         # Current work
    │   ├── backlog.md                # Parked ideas (read before adding to a plan)
    │   ├── live-context-recon.md     # Issue #80: data inventory
    │   ├── live-context-design-brief.md  # Issue #80: handoff to designer
    │   └── live-context-handover.md  # Issue #80: authoritative impl spec
    └── .githooks/pre-commit          # Activate via:
                                      #   git config core.hooksPath .githooks

## Data flow

- **Sessions** are persisted in sqlite at `<APP_DATA>/skein.db` (one
  row per session, full Session as a JSON blob). `App.tsx` loads on
  mount; every `sessions` state change mirrors back to the DB.
  Tour-driven state changes are gated to never reach the DB.
- **PTYs** live in `PtyManager`. `pty_spawn` returns an opaque id;
  output streams over a per-spawn `tauri::ipc::Channel<String>`.
  PTYs die with the parent process — no reconnect across Skein
  restarts in chapter 1 (chapter 2 phase 5 changes that for harnesses
  with native resume).
- **Watchers** (`WatcherManager`) wrap `notify-debouncer-mini`. One
  per active session's worktree, debounced 200 ms, pushes `()` over a
  Channel. Frontend re-fetches `git_status` and `git_diff` on each
  tick.
- **Git ops** go through `crates/skein-git`. `app/src-tauri/src/git.rs`
  is a thin wrapper that converts to serde-friendly DTOs.

## Running it

    # First time:
    git config core.hooksPath .githooks
    cd app && npm install

Skein has three build profiles (issue #21). Each has its own bundle
identifier, so APP_DATA / log dir / config dir are all isolated —
state from one never bleeds into another.

| Profile       | Command                       | Identifier                         | Window/dock label |
| ------------- | ----------------------------- | ---------------------------------- | ----------------- |
| dev           | `npm run tauri:dev`           | `com.timeloop-vault.skein.dev`     | Skein (dev)       |
| local release | `npm run tauri:build:local`   | `com.timeloop-vault.skein.local`   | Skein (local)     |
| release       | `npm run tauri build`         | `com.timeloop-vault.skein`         | Skein             |

- **dev** — debug build, hot-reload, what you use day-to-day for
  feature work.
- **local release** — release-mode optimized bundle for testing what
  the *real* user experience looks like before cutting a release.
  Lives next to your daily driver in /Applications without touching
  its state.
- **release** — what `tauri build` produces by default and what the
  GitHub release pipeline ships. This is the daily-driver bundle.

App data dirs on Windows:
`%APPDATA%\com.timeloop-vault.skein\` (release),
`%APPDATA%\com.timeloop-vault.skein.dev\` (dev),
`%APPDATA%\com.timeloop-vault.skein.local\` (local release).
Delete `skein.db` in any of them to reset persisted state for that
profile.

## Conventions

- **Rust:** edition 2024. `unsafe_code = "forbid"`. Clippy pedantic
  warn, `-D warnings`. Tauri commands collapse `GitError` / `PtyError`
  to `String` at the boundary — they round-trip via JSON anyway.
- **TS:** strict, `noUncheckedIndexedAccess`,
  `exactOptionalPropertyTypes`. Biome with tabs + double quotes.
- **Tests live with the code that owns them.**
  `crates/skein-git/tests/` has 17 integration tests against tempfile
  repos. Tauri side has no tests yet — its surface is mostly thin
  command wrappers.
- **Phases drive the work.** `working-prototype-plan.md` and
  `chapter-2-plan.md` describe order; commit messages name the phase.
- Things we've decided not to do *yet* live in
  [`docs/backlog.md`](docs/backlog.md). Read before adding to a plan.

## Current state

Chapter 1 is complete: real PTYs, sqlite-persisted sessions, real
worktrees created on session creation, live worktree status + diff
with file-watcher auto-refresh. Two known limitations are addressed
by chapter 2:

- PTYs die on session-tab switch (chapter 2 phase 3)
- Harnesses don't resume their conversations on Skein restart
  (phase 5)

When in doubt about a UI convention, the design archive in
`docs/design/` is the source of truth — start with
`docs/design/skein/project/Skein Prototype.html` and the chat
transcript in `docs/design/skein/chats/chat1.md`.

For the right-pane **Live Context** stack (issue #80) the spec
trio at the docs root is authoritative: `live-context-recon.md`
(data we can extract), `live-context-design-brief.md` (the
handoff to the design pass), and `live-context-handover.md`
(what to build). The handover wins on conflicts; the prototype
files in `docs/design/skein/project/` (`Live Context.html`,
`Live Context Prototype.html`, `live-context-*.jsx`,
`lc-proto.jsx`) are the visual reference.
