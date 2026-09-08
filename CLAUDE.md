# CLAUDE.md

Skein is a prototype IDE for driving AI coding agents, built around the
design in `docs/design/`. A **room** (top-level tab) is a task backed by
its own git worktree; each room owns N **harnesses**, all sharing the
room's worktree. Most harness kinds are AI coding CLIs in real PTYs
(Claude Code, opencode, a stub gh-copilot, a plain shell); `files` is
the exception — a **non-PTY** harness (#184) that browses and edits the
worktree. That is why `HARNESS_KINDS` carries a capability model
(`pty` / `resume` / `notify`) instead of assuming a terminal.
"Session" in Skein vocabulary means only one thing: the harness tool's
own conversation id (`Harness.sessionId`), used to resume conversations
across restarts.

Skein is daily-driven on macOS and Windows via auto-updating v0.2.x
releases; **v0.2.9 is the latest**. The original 8-chapter build plan is
complete; work is now driven by GitHub issues.

**Roadmap.** `docs/audit-2026-07-03.md` §5 remains the structural
backlog — #19 App.tsx split, #116 harness-adapter consolidation, #76
cross-room view. It predates two things, so read it with these
corrections:

- **Review surface: #52 is the source of truth**, not the audit. It is
  now an epic with the decisions recorded (scope, diff lifetime,
  anchoring, the agent API) and five sub-issues, #211–#215. **#211,
  #212 and #213 landed** — the diff has a baseline, the review pane is
  the right pane's second tab, and the agent reads and answers comments
  over an MCP endpoint (`docs/agent-api.md`). **#215 (Claude Code
  plugin packaging + opencode config injection) is next**, and until it
  lands the MCP config is written by hand per that doc. #106 was
  rescoped out from under it and, now that the Diff card is gone, is a
  review-pane issue.
- **Files pillar (#49) is half-shipped:** A (#184) and B (#185) landed;
  C (#186) and D (#187) are open.

## Stack

- **Tauri v2** desktop shell (Rust, edition 2024); plugins: updater,
  dialog, clipboard-manager, opener, notifications (the notifications
  plugin is skipped in macOS debug builds — its Swift bridge needs a
  real .app bundle)
- **React 18 + strict TypeScript** UI (Vite); xterm.js for terminals,
  react-virtuoso for the activity feed, CodeMirror 6 for the file
  editor (#185)
- **Three Rust workspace crates** — `skein-git`, `skein-harness`,
  `skein-review` — plus `app/src-tauri`, which is deliberately
  **excluded** from the workspace. Every cargo command therefore needs
  running twice; see Conventions
- **axum** on the tokio runtime Tauri already runs, for the localhost
  agent API + MCP endpoint (#213) — the hyper/http/tower stack under it
  was already locked via reqwest
- **Biome** lint + format (frontend), **clippy pedantic** `-D warnings`
  (Rust)
- Pre-commit hook in `.githooks/pre-commit` (activate:
  `git config core.hooksPath .githooks`) runs, in order: cargo fmt,
  clippy, and tests for BOTH the workspace and `app/src-tauri`
  (excluded from the workspace — `cargo test --workspace` does NOT
  reach its ~160 unit tests; #168), then tsc, vitest and biome. Note the
  workspace fmt needs `--all`: with two members, plain `cargo fmt`
  fails with "Failed to find targets" (#209).

## Layout

    skein/
    ├── crates/skein-harness/        # Tauri-free reader for the harnesses' own stores (#209):
    │   └── src/{claude,opencode}.rs #   Claude JSONL paths/rows/usage/cost-state + subagent
    │                                #   discovery; opencode.db sessions/tree/assistant messages.
    │                                #   Shared with the standalone cost tooling — parser fixes go HERE
    ├── crates/skein-git/            # Pure-Rust libgit2 wrapper. Tauri-free, sync, local-only
    │   └── src/lib.rs               # Repo: open, branches, head_branch, add_worktree,
    │                                #   list/remove_worktree, status, diff_workdir, head_blob
    │                                #   (the review baseline's source, #211);
    │                                #   propose_worktree_path → sibling dir <repo>-wt/<slug>.
    │                                #   No clone/fetch/push/commit yet (#182, #214 add
    │                                #   writes — and #214 argues for the git CLI, not libgit2)
    ├── crates/skein-review/         # The review model (#211 D3/D4, #212 D6). Tauri-free, pure.
    │   └── src/{content,hunks,      #   content+hunks: classify what is on disk
    │             anchor}.rs         #   (text/binary/toolarge/symlink/unreadable), diff
    │                                #   baseline→worktree into hunks, and the two asymmetric
    │                                #   splices — accept (baseline moves, disk untouched) and
    │                                #   reject (disk moves, baseline untouched). Byte-exact:
    │                                #   both splice line slices that keep their terminators.
    │                                #   anchor: comment re-anchoring — anchor on the TEXT, not
    │                                #   the line number; three tiers (exact/exact-elsewhere/
    │                                #   max-overlap) then outdated. Never silently moves or
    │                                #   drops a comment; 22 table tests, no repo needed
    ├── app/
    │   ├── src/                     # React + TS UI
    │   │   ├── App.tsx              # The single React tree (~3.2k LOC hotspot; #19 tracks
    │   │   │                        #   the split): rooms/harness tabs, boot resume,
    │   │   │                        #   notifications, palette items, DnD, keydown dispatch
    │   │   ├── LiveTerminal.tsx     # xterm.js ↔ PTY binding; spawns/kills on (cmd, spawnGen)
    │   │   │                        #   mountKey; attaches the L2c event adapters
    │   │   ├── harnessActivity.ts   # Source of truth for harness phase (spawning/running/
    │   │   │                        #   idle/waiting/exited); L2a idle heuristic + L2b
    │   │   │                        #   patterns + L2c authoritative adapters
    │   │   ├── harnessCmd.ts        # Harness argv: cmdForKind (fresh spawn) + resumeCmd /
    │   │   │                        #   withResumeCmds / unarchiveRoomTransform. Rebuilt from
    │   │   │                        #   the harness RECORD, never by matching the previous
    │   │   │                        #   argv — that pattern-matching was #153/#170. Add a
    │   │   │                        #   spawn flag here and resume keeps working
    │   │   ├── harnessEvents.ts     # L2c translators: ClaudeEvent/OpencodeEvent → phase calls
    │   │   ├── harnessPatterns.ts   # L2b fallback regexes (copilot/shell waiting prompts)
    │   │   ├── data.tsx             # HARNESS_KINDS registry: chip/label/desc + the
    │   │   │                        #   capability model (pty/resume/notify, #184) — small
    │   │   │                        #   but load-bearing. HARNESS_ORDER lives here too
    │   │   ├── types.ts             # Room / Harness / Status vocabulary
    │   │   ├── components.tsx       # Shared atoms (HChip, StatusDot, tabs, picker)
    │   │   ├── shortcuts.ts         # ALL keyboard shortcuts: one platform-agnostic BINDINGS
    │   │   │                        #   table (#151); LiveTerminal swallows via isAppShortcut
    │   │   ├── SettingsModal.tsx    # Settings + in-app updater UI
    │   │   ├── ReopenRoomModal.tsx  # Archived-rooms browser (close = archive, never delete)
    │   │   ├── FilesBody.tsx        # Files pillar B (#185): CodeMirror 6 buffers, save +
    │   │   │   FileTree.tsx         #   write; editor.ts = CM setup; FileTree = dir listing;
    │   │   │   editor.ts            #   filesRegistry = per-harness dirty-buffer registry
    │   │   │   filesRegistry.ts     #   EVERY destroy path (close harness/room/window) must
    │   │   │                        #   consult it — unsaved text lives only in memory
    │   │   ├── SpawnEnvPanel.tsx    # Settings UI for the harness spawn environment (#197)
    │   │   ├── CommandPalette.tsx / statusPopover.ts / Splitter.tsx /
    │   │   │   useFocusRestore.ts / prefs.ts (localStorage UI prefs) / styles.css
    │   │   └── liveContext/         # Right-pane card stack (issue #80): store.ts (backfill
    │   │                            #   500 + live tail of harness-action events, room-scoped),
    │   │                            #   LiveContext.tsx + CardStack (chrome, per-room layout),
    │   │                            #   ActivityCard/feedItems/rows/toolRows/Row/ResultPreview
    │   │                            #   (feed), RoomSubtitle, PlanCard/plan.ts (todo reducer;
    │   │                            #   merges all harnesses — bug #216),
    │   │                            #   useGitBranchWatcher, payload.ts (all payload-shape
    │   │                            #   divergence lives here). review.ts survives the Diff
    │   │                            #   card: accept/reject + attributeHunks (#211 D4).
    │   │                            #   diff.ts is only the render shape + the two harness
    │   │                            #   patch parsers
    │   │   ├── RightPane.tsx        # The right pane's tab strip (#212): Live Context ⇄ Review.
    │   │   │                        #   Both stay mounted (display:none), Mod+R toggles. The
    │   │   │                        #   named seam movable panes will need
    │   │   └── review/              # The review surface (#212, epic #52 B). api.ts (DTO mirror
    │   │                            #   of review_surface.rs + invoke wrappers), ReviewPane
    │   │                            #   (header/base picker/scope switch/orchestration),
    │   │                            #   FileList, CommitList, DiffBody (one unified-diff
    │   │                            #   renderer for all three scopes, hover-to-comment,
    │   │                            #   shift-click to extend), Thread (threads + composer),
    │   │                            #   useReviewData (scope/file fetch + worktree watcher)
    │   └── src-tauri/               # Tauri Rust shell
    │       ├── src/lib.rs           # Builder + 54-command registry; tracing → daily-rotating
    │       │                        #   file in app_log_dir() + stderr (RUST_LOG overrides)
    │       ├── src/pty.rs           # PtyManager (portable-pty); 2 threads per spawn (reader +
    │       │                        #   waiter — the waiter is load-bearing on Windows ConPTY)
    │       ├── src/git.rs           # DTO wrappers around skein-git; GitError → String
    │       ├── src/watcher.rs       # notify-debouncer-mini, 200 ms, .git/ deliberately unfiltered
    │       ├── src/db.rs            # rusqlite: rooms (table `sessions` — legacy name),
    │       │                        #   harness_events, harness_actions, review_baselines (#211),
    │       │                        #   sessions_quarantine;
    │       │                        #   WAL + .bak/.bak.1 snapshots (#167)
    │       ├── src/fs.rs            # list_dir + read/write_file_text — LIVE, called by
    │       │                        #   FileTree/FilesBody since #185. write returns an mtime
    │       │                        #   token for staleness. #174 still wants CSP + scoping
    │       ├── src/spawn_env.rs     # PATH/env merge for harness PTYs (#72, #192, #197, #207):
    │       │                        #   login-shell probe, host-terminal identity stripping,
    │       │                        #   PATHEXT resolution before portable-pty. 50 unit tests
    │       ├── src/spawn_settings.rs # Persisted user overrides for the above
    │       ├── src/resume.rs        # session-existence probes against the tools' own stores
    │       ├── src/review.rs        # Baseline capture + the three baseline commands (#211).
    │       │                        #   Captures from the HEAD blob the moment a LIVE patch
    │       │                        #   row lands — never on backfill, where HEAD has moved
    │       ├── src/review_surface/  # The review the user reads (#212): scope (branch /
    │       │   {dto,git,anchoring,  #   commit / pending), per-file diff, and the nine
    │       │    query,write,        #   thread+comment commands. Split by the question a
    │       │    commands}.rs        #   reader has, not by layer — dto = what the frontend
    │       │                        #   receives, git = what the scope covers, anchoring =
    │       │                        #   where a thread sits NOW (re-matched on every file
    │       │                        #   open, new position written back, anchor text never),
    │       │                        #   query/write = the command logic, commands = the
    │       │                        #   Tauri boundary and nothing else
    │       ├── src/agent_api/       # The agent-facing review API (#213, epic #52 D8):
    │       │   {state,auth,verbs,   #   an axum server on 127.0.0.1:<ephemeral> inside the
    │       │    mcp,http,commands,  #   Tauri process, exposed as MCP over HTTP so Claude
    │       │    tests}.rs           #   Code and opencode both consume it. verbs = the five
    │       │                        #   agent verbs (list/get_comment/get_diff/reply/
    │       │                        #   mark_addressed) — and NO resolve, which is refused
    │       │                        #   by name; auth = the per-room bearer token, which IS
    │       │                        #   the scope. See docs/agent-api.md
    │       ├── src/harness_events_claude.rs    # JSONL tail → ClaudeEvent (L2c-1)
    │       ├── src/harness_events_opencode.rs  # SSE client → OpencodeEvent (L2c-2)
    │       ├── src/harness_actions_claude.rs   # JSONL → harness_actions rows (#80)
    │       ├── src/harness_actions_opencode.rs # SSE/opencode.db → harness_actions rows (#80)
    │       └── src/harness_action_event.rs     # "harness-action" live broadcast (live rows only)
    ├── docs/
    │   ├── audit-2026-07-03.md      # Full-codebase audit; §5 = structural backlog. Predates
    │   │                            #   the #52 review decisions — see Roadmap above
    │   ├── agent-api.md             # The #213 agent API: verbs, headers, error codes, and
    │   │                            #   the hand-written MCP config to use until #215
    │   ├── backlog.md               # Parked ideas (read before adding to any plan)
    │   ├── grok-build-recon-*.md    # Recon of grok-build: reference designs for the review
    │   │                            #   surface (§3 xai-hunk-tracker) + git writes (§6)
    │   ├── files-pillar-design-brief.md  # Design brief behind #49 / #184 / #185
    │   ├── live-context-*.md        # Live Context specs — authority chain in "Design refs" below
    │   ├── working-prototype-plan.md, chapter-*-plan.md, *-recon.md, epic-50-*
    │   │                            # HISTORICAL records of shipped chapters 1–8 + epic #50.
    │   │                            #   Useful for "why", wrong about "what is"
    │   └── design/                  # Design handoff bundles (read-only reference)
    ├── scripts/local-bundle.sh      # macOS bundled-.app smoke test (NOTE: uses the RELEASE
    │                                #   identifier, unlike tauri:build:local)
    ├── .claude/skills/              # git-workflow (branches + commits),
    │                                #   pr-workflow (PRs, stacks, gh traps)
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
  `"harness-action"` Tauri event for live rows only. **Where the store
  paths and row shapes live: `crates/skein-harness` (#209)** — parser
  fixes go there, not in the adapters, so the app and any standalone
  cost tooling stay in sync.
  The Live Context store backfills the newest 500 rows per room and
  appends live ones; Diff/Plan/Activity cards all render from that one
  array. One known consequence of it being **room**-scoped: the Plan
  card merges every harness in the room into one incoherent list
  (#216).
- **Review baselines** (#211, epic #52 D3/D4): a `review_baselines`
  row per (room, file) holds the content the user last reviewed. It is
  captured from the git HEAD blob at the moment a **live** `patch` row
  is recorded — backfill deliberately does not capture, because
  replaying history is exactly when HEAD has already moved past the
  edit. The review pane's **pending** scope renders `baseline →
  worktree`, so a file clears when it is reviewed and *not* when the
  agent commits. Accept advances the baseline and writes nothing;
  reject writes the file back and moves nothing. A file the harness
  only *created* untracked reads as fully added on first touch —
  over-reporting is the safe direction, and one accept settles it.
  (Discovery is still patch-row-driven, which is the hole #221 names.)
- **The review surface** (#212, epic #52 D1/D5/D6/D7): the right pane's
  second tab. Three scopes over one renderer — **branch**
  (`merge-base(HEAD, base) → working tree`, the default and the only
  view that counts committed and uncommitted work together),
  **commit** (one commit vs its first parent), **pending** (the
  baseline model above, and the only scope with accept/reject). Base
  ref is guessed per repo and overridden per room in
  `review_settings`. Comments live in `review_threads` +
  `review_comments`, carry an `author` from the first migration (D7)
  so #213's agent replies are a row change and not a migration, and
  **anchor on the text they were written against, never on a line
  number**. Every thread re-anchors on file open and the new position
  is written back; one that cannot be placed renders above the diff
  against its original code. **Never silently moved, never silently
  dropped** — that is the whole contract, and the matcher lives in
  `crates/skein-review/src/anchor.rs` where it is testable without a
  repo. `review_viewed` stores the *content hash* the user looked at,
  which is what makes "changed since I last looked" fall out for free.
- **The agent API** (#213, epic #52 D8): an axum server bound to
  `127.0.0.1:0` in `setup()`, serving `/mcp` (MCP streamable HTTP —
  what the harnesses talk to) and a plain-JSON `/api/*` mirror of the
  same verbs. `pty_spawn` mints (or reuses) the room's bearer token and
  puts `SKEIN_REVIEW_URL` / `SKEIN_REVIEW_TOKEN` / `SKEIN_ROOM_ID` /
  `SKEIN_HARNESS_ID` into every harness's environment; both harnesses
  expand `${VAR}` in their MCP config, so the ephemeral port never has
  to be agreed in advance. **The token is the scope** — no request names
  a room — and `X-Skein-Harness` is attribution only, never authority.
  Tokens are revoked wholesale on every boot: PTYs die with the app, so
  a token that leaked into an old log stops working. The agent gets
  read, `reply` and `mark_addressed`; **`resolve` is refused by name**,
  because an agent that can close its own comments removes the loop's
  only gate. Writes emit `skein://review-changed` — a comment touches
  only sqlite, so no watcher would otherwise fire. Verbs reuse
  `review_surface::query::file_impl` rather than re-anchoring
  themselves. Full contract: `docs/agent-api.md`.
- **Files** (#185): `FileTree` lists via `list_dir`, `FilesBody` reads
  via `read_file_text` and saves via `write_file_text`, which
  round-trips an mtime token to detect a stale write. Buffer text
  lives **only in memory** until saved, so every destroy path — close
  harness, close room, close window — must consult `filesRegistry`
  and prompt.
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
  `crates/skein-git/tests/` has 46 integration tests against tempfile
  repos; `crates/skein-harness` has ~25 in-module tests;
  `crates/skein-review` has 45; `app/src-tauri` has ~285 in-module unit
  tests in source, of which ~245 run on any one platform — 244 on
  Windows; the rest are
  `cfg`-gated per OS (#202) (spawn-env merging, harness JSONL/SSE
  parsers, db persistence, pty). All run in the hook
  and CI, but **`cargo test --workspace` does NOT reach the tauri
  crate** — it is excluded from the workspace (#168), so the hook runs
  it via a second `--manifest-path`. Same for fmt and clippy. The
  frontend has vitest as of #170 (`cd app && npm test`, in the hook and
  CI) but only `harnessCmd.test.ts` uses it so far — #169 stays open for
  the rest, and it matters: nearly every shipped regression has lived
  there. New suites are `src/**/*.test.ts`, node environment, no DOM;
  pure modules are the cheap wins, so prefer extracting logic out of
  App.tsx over reaching for jsdom.
- **Issues drive the work.** Commit messages name the issue
  (`fix(#158): …`). The chapter/phase system ended with chapter 8;
  plan docs are history, not instructions. Parked ideas live in
  [`docs/backlog.md`](docs/backlog.md) — read it before adding to any
  plan; things there move to GitHub issues when they become real.
- **Branch, commit and PR conventions live in `.claude/skills/`** —
  `git-workflow` (branch naming, commit subject/body, trailers,
  staging hazards) and `pr-workflow` (PR body shape, stacked PRs, the
  post-squash-merge rebase). Read the relevant one before committing or
  opening a PR rather than reconstructing the format from `git log`.
  Two traps they exist to prevent: `gh` has no `--body @-` (that
  silently sets the body to the literal string `@-` — use
  `--body-file -` and verify), and a squash-merge orphans every
  stacked branch below it.
- **Keyboard shortcuts** only via the `BINDINGS` table in
  `shortcuts.ts`; prefer letters/digits (Swedish layout — AltGr =
  Ctrl+Alt collides with punctuation chords) and agree bindings with
  Stefan before committing (#150 tracks user rebinding).

## Current state (2026-09)

Chapters 1–8 all shipped: real PTYs and worktrees, sqlite-persisted
rooms with archive/reopen, harness conversation resume across
restarts, the Live Context right pane (activity/plan/diff cards fed
by harness telemetry), notifications (badge/toast/OS), Windows +
Linux support, keyboard-driven navigation, and distribution with
in-app auto-update. **v0.2.9 is the latest release.**

Landed since the 2026-07-03 audit: #167 (boot-wipe data-loss fix +
persistence hardening), #168 (test gates), #173 (this file's last
rewrite), Files pillar A + B (#184, #185 — the `files` harness kind
and a CodeMirror editor), spawn-environment work (#192, #197, #207),
authoritative session cost (#199), #209 (the `skein-harness` crate
extraction), and a run of Windows daily-driver fixes
(#200/#201/#202/#207/#217). #211 (the review baseline — the
diff finally clears), #212 (the review pane) and #213 (the agent API +
MCP server) open the #52 arc.

Known-weak spots, still open: App.tsx size and duplication (#19 — now
~3.2k LOC), the duplicated Claude/opencode adapter pairs (#116), heavy
sync Tauri commands on the main thread (#171/#172/#178/#179), silent
failure surfacing (#176), and frontend test coverage that is one file
deep (#169 — vitest itself landed with #170).

The **review surface (#52)** is the current headline feature arc; its
decisions are recorded in the epic. #211 (baseline model), #212 (the
review pane — branch-vs-base diff, comments, anchoring) and #213 (the
review API + MCP server, which is what finally lets the agent *read*
the comments) have landed; #215 (harness config injection, so the MCP
server is wired up without a hand-written `.mcp.json`) and #214 (the
land actions) are what remain. Do not plan review work off the audit —
it predates those decisions.

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
