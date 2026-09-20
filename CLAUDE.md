# CLAUDE.md

Skein is a prototype IDE for driving AI coding agents, built around the
design in `docs/design/`. A **room** (top-level tab) is a task backed by
its own git worktree; each room owns N **harnesses**, all sharing the
room's worktree. Most harness kinds are AI coding CLIs in real PTYs
(Claude Code, opencode, a stub gh-copilot, a plain shell); `files` is
the exception — a **non-PTY** harness (#184) that browses and edits the
worktree. That is why `HARNESS_KINDS` carries a capability model
(`pty` / `resume` / `notify` / `agents`) instead of assuming a terminal.
"Session" in Skein vocabulary means only one thing: the harness tool's
own conversation id (`Harness.sessionId`), used to resume conversations
across restarts.

Skein is daily-driven on macOS and Windows via auto-updating v0.2.x
releases. The original 8-chapter build plan is complete.

**Where the work is.** GitHub issues are the only backlog; this file
does not restate their status. The order of attack is
`gh issue list --label priority-high`; parked ideas live in
`docs/backlog.md`; design decisions for the review surface are recorded
on epic #52. `docs/audit-2026-07-03.md` §5 is a historical snapshot,
not a roadmap. Two standing decisions that no issue body will tell you:

- **Skein performs no git mutations.** It does not commit, merge, push
  or open pull requests (D9 on #52, corrected 2026-09-09 after #229
  shipped and was reverted). The reviewer's sign-off is the one fact
  Skein owns; the agent lands branches by reading
  `.claude/skills/pr-workflow`. A tested git-CLI write layer survives
  unshipped in `2bcae5a` (tag `archive/214-land-actions`) — lift it, don't
  rewrite it, if an issue ever needs writes.
- **Reviews happen after the fact, against git.** The review unit is
  still committed work on the room's branch, so the branch and commit
  scopes stay git-only and nothing needs to cover gitignored paths —
  but the pending scope's baseline discovery runs off a per-room
  filesystem watcher (#221) and therefore does work in a non-git room,
  with no `git status` catch-up for changes made while Skein was
  closed, so it only ever discovers from live watcher ticks.

## Stack

- **Tauri v2** desktop shell (Rust, edition 2024); plugins: updater,
  dialog, clipboard-manager, opener, notifications (the notifications
  plugin is skipped in macOS debug builds — its Swift bridge needs a
  real .app bundle)
- **React 18 + strict TypeScript** UI (Vite); xterm.js for terminals,
  react-virtuoso for the activity feed, CodeMirror 6 for the file
  editor (#185)
- **Four Rust workspace crates** — `skein-git`, `skein-harness`,
  `skein-review`, `skein-winnotify` — plus `app/src-tauri`, which is
  deliberately **excluded** from the workspace. Every cargo command
  therefore needs running twice; see Conventions
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
    │   ├── src/{claude,opencode}.rs #   Claude JSONL paths/rows/usage/cost-state + subagent
    │                                #   discovery; opencode.db sessions/tree/assistant messages.
    │                                #   Since #276: the agent-<id>.meta.json sidecar parse
    │                                #   (undocumented upstream, every field optional) and the
    │                                #   terminal-stop_reason check that tells a subagent
    │                                #   transcript's own end of turn from its exit — it has no
    │                                #   user to await, so those are the same event. Shared with
    │                                #   the standalone cost tooling — parser fixes go HERE
    │   └── src/agents/              #   Which agents each CLI accepts at `--agent` (#246).
    │                                #   Names come from the CLI itself — an unknown --agent makes
    │                                #   Claude enumerate its own list; opencode has `agent list`.
    │                                #   Disk parsing is enrichment ONLY (description + the tools
    │                                #   allowlist), because globbing the config dirs drifts silently
    ├── crates/skein-git/            # Pure-Rust libgit2 wrapper. Tauri-free, sync, local-only
    │   └── src/lib.rs               # Repo: open, branches, head_branch, add_worktree,
    │                                #   list/remove_worktree, status, diff_workdir, head_blob
    │                                #   (the review baseline's source, #211);
    │                                #   propose_worktree_path → sibling dir <repo>-wt/<slug>.
    │                                #   READS ONLY, deliberately: Skein performs no git
    │                                #   mutations (D9 corrected, #182 closed not planned).
    │                                #   A tested git-CLI write layer sits unshipped in
    │                                #   2bcae5a (tag archive/214-land-actions) — lift it, don't
    │                                #   rewrite it, if an issue ever needs writes
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
    ├── crates/skein-winnotify/      # Windows-only OS toasts + COM click activation (#155):
    │                                #   unpackaged-app AUMID registration, a
    │                                #   INotificationActivationCallback so Action Center/cold-start
    │                                #   clicks work, foreground-window recovery. The workspace's
    │                                #   ONE crate allowed `unsafe` (its own `[lints]` table, not
    │                                #   `workspace = true`) — raw WinRT/Win32 calls can't be made
    │                                #   safe otherwise. app/src-tauri keeps `forbid` and only
    │                                #   calls its safe functions
    ├── app/
    │   ├── src/                     # React + TS UI
    │   │   ├── App.tsx              # The single React tree (~3.2k LOC hotspot; #19 tracks
    │   │   │                        #   the split): rooms/harness tabs, boot resume,
    │   │   │                        #   notifications, palette items, DnD, keydown dispatch
    │   │   ├── LiveTerminal.tsx     # xterm.js ↔ PTY binding; spawns/kills on (cmd, spawnGen)
    │   │   │                        #   mountKey; attaches the L2c event adapters
    │   │   ├── harnessActivity.ts   # Source of truth for harness phase (spawning/running/
    │   │   │                        #   idle/waiting/exited); L2a idle heuristic + L2b
    │   │   │                        #   patterns + L2c authoritative adapters. #277 defers
    │   │   │                        #   a main session's end-of-turn while its subagents
    │   │   │                        #   are still working, rather than adding a phase
    │   │   ├── subagents.ts         # Per-harness live-Claude-subagent registry + useLiveSubagents
    │   │   │                        #   (#276, epic #298). Disk is the source of truth: the
    │   │   │                        #   adapter re-derives the live set from the transcripts on
    │   │   │                        #   every attach, so `record` is idempotent and a restart
    │   │   │                        #   replay never looks like a second start. `workingCount`
    │   │   │                        #   (excludes attach-time orphans) and `presumeGone` back
    │   │   │                        #   #277's end-of-turn deferral in harnessActivity.ts
    │   │   ├── harnessCmd.ts        # Harness argv: cmdForKind (fresh spawn) + resumeCmd /
    │   │   │                        #   withResumeCmds / unarchiveRoomTransform. Rebuilt from
    │   │   │                        #   the harness RECORD, never by matching the previous
    │   │   │                        #   argv — that pattern-matching was #153/#170. Add a
    │   │   │                        #   spawn flag here and resume keeps working (#247's
    │   │   │                        #   `--agent` is the proof: one `withAgent` per arm)
    │   │   ├── agents.ts            # Which agents a kind accepts at `--agent` (#247): DTO
    │   │   │                        #   mirror of agents.rs + validateAgent. A stored name
    │   │   │                        #   is re-checked at EVERY spawn, not only at pick time —
    │   │   │                        #   only a fresh `claude --agent X` fails loudly; resume
    │   │   │                        #   and opencode fall back silently
    │   │   ├── harnessEvents.ts     # L2c translators: ClaudeEvent/OpencodeEvent → phase calls
    │   │   ├── harnessPatterns.ts   # L2b fallback regexes (copilot/shell waiting prompts)
    │   │   ├── data.tsx             # HARNESS_KINDS registry: chip/label/desc + the
    │   │   │                        #   capability model (pty/resume/notify/agents/
    │   │   │                        #   agentSwitchable, #184 + #247 + #248) — small but
    │   │   │                        #   load-bearing. HARNESS_ORDER too
    │   │   ├── harnessAgent.ts      # What agent a harness is on, honestly worded (#248):
    │   │   │                        #   "agent X" (Claude), "started as X" / "last message
    │   │   │                        #   to Y" (opencode, from SSE user messages)
    │   │   ├── harnessInput.ts      # The #238 nudge seam (#41 reuses it): a per-harness
    │   │   │                        #   paste/submit registry LiveTerminal fills in once its
    │   │   │                        #   PTY is live, plus the pure gate `canSendPrompt` — pty
    │   │   │                        #   capability, registered, phase===waiting, a proven L2c
    │   │   │                        #   adapter, #215 config injection, and bracketed-paste
    │   │   │                        #   for multi-line bodies. No nudge where safety can't be
    │   │   │                        #   proven
    │   │   ├── types.ts             # Room / Harness / Status vocabulary
    │   │   ├── components.tsx       # Shared atoms (HChip, StatusDot, tabs, the two-step
    │   │   │                        #   harness picker — kind, then agent for the kinds
    │   │   │                        #   that take one)
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
    │   │                            #   merges all harnesses in the room — accepted),
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
    │   │                            #   useReviewData (scope/file fetch + worktree watcher +
    │   │                            #   useSignoff), signoff.ts + SignoffControl (#214: the
    │   │                            #   sign-off control, three states — none/approved/
    │   │                            #   lapsed. Skein records the approval; the AGENT lands),
    │   │                            #   nudges.ts (#238: the header Nudge button's three
    │   │                            #   fixed prompts, picked by sign-off state + open count)
    │   └── src-tauri/               # Tauri Rust shell
    │       ├── src/lib.rs           # Builder + 57-command registry; tracing → daily-rotating
    │       │                        #   file in app_log_dir() + stderr (RUST_LOG overrides)
    │       ├── src/pty.rs           # PtyManager (portable-pty); 4 threads per spawn (raw
    │       │                        #   reader + coalescer + writer + waiter — the waiter is
    │       │                        #   load-bearing on Windows ConPTY; #171 split the reader
    │       │                        #   in two to batch output and gave stdin its own thread
    │       │                        #   so a wedged child can't block write/resize/kill)
    │       ├── src/git.rs           # DTO wrappers around skein-git; GitError → String
    │       ├── src/watcher.rs       # notify-debouncer-mini, 200 ms, .git/ deliberately unfiltered
    │       ├── src/db.rs            # rusqlite: rooms (table `sessions` — legacy name),
    │       │                        #   harness_events, harness_actions, review_baselines (#211),
    │       │                        #   sessions_quarantine;
    │       │                        #   WAL + .bak/.bak.1 snapshots (#167)
    │       ├── src/fs.rs            # list_dir + read/write_file_text — LIVE, called by
    │       │                        #   FileTree/FilesBody since #185. write returns an mtime
    │       │                        #   token for staleness. Paths are NOT yet scoped to the room (#174)
    │       ├── src/spawn_env.rs     # PATH/env merge for harness PTYs (#72, #192, #197, #207):
    │       │                        #   login-shell probe, host-terminal identity stripping,
    │       │                        #   PATHEXT resolution before portable-pty. 50 unit tests
    │       ├── src/spawn_settings.rs # Persisted user overrides for the above
    │       ├── src/harness_config.rs # What Skein injects at spawn so an agent CLI finds the
    │       │                        #   #213 review API (#215): `--plugin-dir` for Claude Code,
    │       │                        #   OPENCODE_CONFIG for opencode, both session-scoped and
    │       │                        #   ADDITIVE — the user's own plugins and config are
    │       │                        #   untouched, and NOTHING is written into the worktree.
    │       │                        #   No bound endpoint = no injection, or the config would
    │       │                        #   interpolate a variable that isn't set
    │       ├── harness-config/      # The bundle those two point at, shipped as a Tauri
    │       │   claude-plugin/       #   resource (see tauri.conf.json `resources`):
    │       │   opencode/            #   .claude-plugin/plugin.json + .mcp.json + a lazily
    │       │                        #   loaded skein-review SKILL.md; opencode.json
    │       ├── src/agents.rs        # DTO layer over the crate's agent discovery (#246), plus
    │       │                        #   the two things it can't do: resolve the program the way
    │       │                        #   a real spawn does, and pass #215's --plugin-dir so the
    │       │                        #   list matches what the spawn will accept
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
    │       │                        #   Tauri boundary and nothing else.
    │       │   + signoff.rs         #   signoff = has the reviewer approved (#214), keyed to
    │       │                        #   the head_sha it approved — a later commit makes it
    │       │                        #   STALE rather than silently covering unread work
    │       ├── src/agent_api/       # The agent-facing review API (#213, epic #52 D8):
    │       │   {state,auth,verbs,   #   an axum server on 127.0.0.1:<ephemeral> inside the
    │       │    mcp,http,commands,  #   Tauri process, exposed as MCP over HTTP so Claude
    │       │    tests}.rs           #   Code and opencode both consume it. verbs = the six
    │       │                        #   agent verbs (list/get_comment/get_diff/reply/
    │       │                        #   mark_addressed/review_status) — and NO resolve and
    │       │                        #   NO approve, both refused BY NAME; auth = the
    │       │                        #   per-room bearer token, which IS the scope.
    │       │                        #   See docs/agent-api.md
    │       ├── src/harness_events_claude.rs    # JSONL tail → ClaudeEvent (L2c-1). Since #276,
    │       │                                   #   tails every `subagents/agent-*.jsonl` sidecar
    │       │                                   #   alongside the main file on the same debouncer,
    │       │                                   #   re-deriving the live set from disk on every
    │       │                                   #   attach — SubagentStart/ToolResult/End, plus one
    │       │                                   #   `subagent_end` harness_actions row per finish.
    │       │                                   #   `SubagentStart.initial` (#277) is true only
    │       │                                   #   for the disk-seeded attach batch, never a
    │       │                                   #   live start
    │       ├── src/harness_events_opencode.rs  # SSE client → OpencodeEvent (L2c-2)
    │       ├── src/harness_actions_claude.rs   # JSONL → harness_actions rows (#80)
    │       ├── src/harness_actions_opencode.rs # SSE/opencode.db → harness_actions rows (#80)
    │       └── src/harness_action_event.rs     # "harness-action" live broadcast (live rows only)
    ├── docs/
    │   ├── audit-2026-07-03.md      # Full-codebase audit; §5 = structural backlog. Predates
    │   │                            #   the #52 review decisions — see Roadmap above
    │   ├── agent-api.md             # The #213 agent API: verbs, headers, error codes, and
    │   │                            #   how #215 gets it in front of each harness
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
  **Subagent awareness (#276, epic #298):** the main transcript never
  carries a subagent's own rows — zero sidechain rows in every session
  sampled; the two `isSidechain` filters in
  `claude.rs`/`harness_events_claude.rs` are defensive legacy, not
  load-bearing. Each delegation gets its own sibling file,
  `<sid>/subagents/agent-<id>.jsonl`, with an undocumented
  `agent-<id>.meta.json` sidecar giving `agentType`/`description`
  (`crates/skein-harness`: every field optional, unknown keys ignored on
  purpose — the shape belongs to Claude Code, not Skein).
  `harness_events_claude.rs` tails every such file on the same debouncer
  as the main one and re-derives the live set from the transcripts on
  every attach, never from a `SubagentStart`/`SubagentStop` hook pair —
  #276 shipped without the hooks it originally specified, because a
  hook-fed set of outstanding ids can drift from reality with nothing to
  repair it (parked in #309, which also records that upstream
  `SubagentStop` is reported not to always fire). A subagent has no user
  to await, so its own end of turn IS its exit: a terminal `stop_reason`
  (`end_turn`/`stop_sequence`/`max_tokens`) means done, verified 221/221
  on real transcripts — nothing polls mtime. The frontend mirrors the
  live set in a pure per-harness registry, `subagents.ts`. The feed
  already renders the delegation via the main transcript's `Agent`
  tool_call row, so the only new row is `subagent_end`: a *background*
  subagent's `AgentRow` lands at launch and nothing else ever marks it
  finished.
  **End-of-turn deferral (#277, epic #298):** a main session ending its
  turn while `subagents.workingCount` is non-zero does not move to
  `waiting` — that phase means "your turn", and it was a lie while
  delegated work ran, measured on 127 real session dirs: 1,894
  end-of-turns with a subagent still running, in 119 of them (94%).
  `awaitingPromptFromAdapter` arms a deferral instead of moving the
  phase; no new phase was added (decision (c) on #277) — the harness
  stays `running` (or `permission`, which outranks it), and
  `statusLabel` prints "delegating · N agents" in its place. The
  dominant resolution needs no timer: any main-transcript work signal
  disarms the deferral, and the session's OWN next end-of-turn — by
  when the working set is empty, since it woke to work the delegation's
  result — is what actually flips to `waiting`, carrying
  `delegationSummary`'s "N delegated agents finished". Measured on 1009
  real delegations, that wake-up followed a subagent's terminal row
  every time: 27 ms median, 1.7 s p90, 54.1 s max. Two safety nets exist
  because a leaked entry means permanent silence, worse than the noise
  being fixed: `DELEGATION_SETTLE_MS` (60 s, above the 54.1 s worst
  wake-up) flushes once the working set is seen empty and stays empty;
  `DELEGATION_CEILING_MS` (15 min of total subagent silence — measured
  2.4% of 1038 real subagents have an internal quiet gap that long)
  presumes the working set gone and notifies without claiming anything
  finished, since Skein can't know that. The restart leak — every
  attach-time subagent reading as newly working — is closed by
  `ClaudeEvent::SubagentStart.initial`, true only for the disk-seeded
  attach batch; `subagents.workingCount` excludes those via
  `SubagentEntry.fromAttach` (sticky false once a live start is seen;
  2.8% of real transcripts are these orphans). The mechanism keys only
  on `subagents.workingCount`, not on anything Claude-specific — the
  sources are `delegation-settled`/`delegation-ceiling`, not
  `l2c1-claude-*` — so a future opencode child-session signal would
  inherit it for free; opencode registers no subagents today, so the
  count is always 0, and none of this needs #215 config injection, same
  as subagent discovery generally — only the *permission* signal does.
  **`permission` is its own phase (#86)**, distinct from `waiting`
  (end of turn / needs input), and outranks it everywhere. Claude's
  JSONL records nothing when a dialog opens, so the signal is a
  `PermissionRequest` command hook in the #215 plugin bundle that
  curls `POST /api/harness/permission` → `skein://harness-permission`
  (verified: it does not fire for auto-approved tools). Nothing says
  when the dialog is *answered*, so a decisive keystroke in that PTY
  clears it, backed by the next tool_result / prompt / end-turn;
  PTY output and "still working" adapter events never do. opencode
  gets exact `permission.asked`/`replied` ids; its `question.*`
  events map to `waiting`. Injection off = no Claude permission
  signal, just `waiting` as before.
  A subagent's own `PermissionRequest` fires the same hook — POSTing to
  `/api/harness/permission` exactly like a main-session dialog — but the
  *answering* tool result lands only in the subagent's own transcript,
  which nothing in the main tail reads, so the keystroke fallback can't
  clear it either, and a mouse click reaches it even less (a click
  arrives via xterm's `onData`, not `onKey`, so it was never going to be
  seen). That was epic #298's headline bug: the badge stuck for minutes
  on a subagent dialog nobody could answer with a keystroke. The fix is
  the subagent's own answering tool result: `SubagentToolResult` calls
  `harnessActivity.clearPermission`, a no-op unless the phase is already
  `permission`, moving it only to `running` — never
  `waiting`/`idle`/`spawning`/`exited`, and never a notification,
  because a subagent tool result proves only that its own gate is gone,
  not that the harness is doing anything in particular.
  That "own gate" claim needed enforcing, and review caught that it
  wasn't: the hook payload carries `agent_id` when it fires inside a
  subagent, Skein reads it (`agent_api/http.rs`), ships it on
  `skein://harness-permission` as `agentId`, and stores it as
  `permissionAgentId`; `clearPermission(id, source, agentId)` now clears
  only when the stored `permissionAgentId` is `null` **or** equals the
  reporting subagent's id. The null case still clears unconditionally —
  that is a main-session dialog, or a CLI that didn't report the id, and
  falling back to the old behaviour is the safe direction, since a
  dialog cleared slightly early is recoverable while one stuck for
  minutes is the bug being fixed. The bug this correlation prevents,
  worth naming because it is non-obvious: two background subagents, B
  opens a dialog, A returns an unrelated tool result, and without
  correlation A's result clears B's still-open dialog — the badge reads
  "running" while the harness is actually blocked. Concurrent delegation
  is exactly this epic's target workload. A user action (the decisive
  keystroke) and adapter loss (`releasePermission`) still clear
  unconditionally, whoever opened the dialog.
  `/api/harness/permission` now writes one `tracing::info!` per request
  (#176) — the 2026-09-19 incident could only be reconstructed from the
  database because it did not.
  The Live Context store backfills the newest 500 rows per room and
  appends live ones; the Plan and Activity cards both render from that
  one array. It is **room**-scoped, so the Plan card shows every
  harness's todos in one list — a known and accepted consequence.
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
  (#221: a per-room filesystem watcher also discovers baselines, so a
  shell write, a hand edit or the `files` harness shows up too, not
  only a harness `patch` row — captured with an empty `harness_id`, so
  no chip, until a later `patch` row claims the file. The watcher also
  runs one `git status` catch-up on start, to cover changes made while
  Skein itself was closed; a non-git room has no status to catch up
  from, so it only ever discovers from live watcher ticks.)
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
- **Harness config injection** (#215, epic #52 E): the variables above
  are useless until the CLI knows there is a server at that address, so
  `pty_spawn` also appends `--plugin-dir <resources>/harness-config/`
  `claude-plugin` for Claude Code and sets `OPENCODE_CONFIG` for
  opencode. Both are **session-scoped and additive** — installed plugins
  and the user's own config still load, and a repo's `opencode.json`
  still beats Skein's — and **nothing is written into the worktree**, so
  there is no file in the review's own diff, no MCP trust prompt, and
  nothing left pointing at a dead port once a room is archived. The
  spawn path passes the harness *kind*, not a guess from `cmd`: the two
  disagree the moment "press Enter for shell" rewrites an argv, and
  `--plugin-dir` on a shell would break the spawn. Settings → Shell &
  environment shows what is injected and switches either off; turning
  the opencode one off is how `OPENCODE_CONFIG` becomes the user's
  again, which is why that key is reserved only *while* being injected.
- **The sign-off** (#214, epic #52 D9 as corrected): a `review_signoff`
  row per room — the reviewer's approval, and **the only thing in Skein
  an agent treats as permission**. The whole design is the `head_sha`
  column: an approval names the commit it was granted against, so a
  later commit makes it read **stale** rather than silently stretching
  over code nobody reviewed. Same trick `review_viewed` plays one level
  down with its content hash. Unresolved threads never block a
  sign-off; the counts ride along so the reviewer decides in view of
  them. The agent reads it through `review_status` and **cannot grant
  one** — `approve`/`sign_off`/`mark_approved` are refused by name
  beside `resolve`, for the same reason.
  **Skein performs no git mutations.** It does not merge, push, or open
  pull requests: which strategy, which forge, what a PR body looks like
  are all things the repo already states in `.claude/skills/` and
  `CLAUDE.md`, and the agent reads them. Skein owns the one fact that
  lives nowhere else. (#229 tried the other way and was reverted; see
  the Roadmap note.)
- **Agent selection** (#246 + #247, epic #219): both CLIs bind
  `--agent` at launch and Claude cannot change it afterwards, so the
  choice is made *before* the process starts — a second step in the `+
  harness` picker, and a field in New room remembered per folder.
  `Harness.agent` is the authority, not the argv: `cmdForKind` and
  `resumeCmd` both re-pass it, because `claude --resume` with no flag
  restores whatever agent the *conversation* started as. Every call
  site gates on `capabilities.agents`, never on a kind comparison.
  Two failures the UI has to name because nothing else will: an
  agent whose `tools` allowlist omits MCP **cannot see the review
  tools at all** — healthy connection, no error, nothing in any log
  (#215) — so those rows are badged; and a stored name that no longer
  resolves is **re-checked at every spawn**, since only a fresh
  `claude --agent X` fails loudly while resume and both opencode paths
  fall back silently. A degraded list never blocks a spawn: it cannot
  prove a name is gone.
  **#248** adds a per-kind default in Settings (`prefs.ts`
  `defaultAgents` — localStorage, because the frontend builds the argv),
  preselected in the picker and prefilled in New room, where a folder's
  own remembered agent still wins. No default is an absent key, never a
  name. The agent shows in the status bar and the harness-tab popover,
  worded by `harnessAgent.ts`: Claude is "agent X"; opencode can switch
  mid-session, so it is "started as X" until an SSE `message.updated`
  for a **user** message in the harness's **own** session says "last
  message to Y" (assistant messages report `compaction`; `/event` also
  carries subagent child sessions). The observation is never written
  back to `Harness.agent`, which is what resume re-passes.
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
  touching its state. Its exe is `skein-app-local.exe` (`mainBinaryName`,
  #293): the NSIS installer refuses while *any* process of its own exe
  name runs, so a shared `skein-app.exe` blocked installing it beside
  the release. Release keeps `skein-app` for the updater and existing
  installs; dev needs nothing, since `tauri dev` never renames.
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
  frontend has vitest (`cd app && npm test`, in the hook and CI), but
  coverage is thin and nearly every shipped regression has lived in the
  frontend. New suites are `src/**/*.test.ts`, node environment, no
  DOM; pure modules are the cheap wins, so prefer extracting logic out
  of App.tsx over reaching for jsdom.
- **Never spawn a bare `git` in a test.** Build fixture repos with
  `git2`, which takes the path as an argument — the way `review.rs`,
  `review_surface/signoff.rs` and `agent_api/tests.rs` all do. **Git
  exports `GIT_DIR` (and `GIT_INDEX_FILE`) to every hook it runs**, so
  a test spawned from the pre-commit gate inherits them and a
  `Command::new("git")` in a `TempDir` silently operates on *this
  repository* instead: `git init` re-initialises it and sets
  `core.bare = true` on the config every worktree shares — which reads
  from outside like the `.git` folder vanished — `git config` writes
  test identity into it, and `git commit` commits the work in progress
  onto the branch under test. All three happened on 2026-09-09. Any
  future code that does spawn git, in tests or in production, must
  `env_remove` `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`,
  `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`,
  `GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_NAMESPACE` and `GIT_PREFIX`
  first; `2bcae5a` (tag `archive/214-land-actions`) has that, tested.
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
  Stefan before committing. User rebinding is not planned.

## Current state

Chapters 1–8 all shipped: real PTYs and worktrees, sqlite-persisted
rooms with archive/reopen, harness conversation resume across restarts,
the Live Context right pane, notifications (badge/toast/OS), Windows +
Linux support, keyboard-driven navigation, distribution with in-app
auto-update, a `files` harness with a CodeMirror editor, the review
surface (baseline, review pane, agent API + MCP server, sign-off, config
injection), and Claude subagent awareness — a feed row for a finished
delegation, permission clearing for a subagent's own dialog (#276), and
an end-of-turn deferral so a harness with subagents still working reads
"delegating" rather than a premature "waiting" (#277).
What is open, what is next and what is known-weak is on GitHub — see
"Where the work is" at the top. `git log` records what landed and when.

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
