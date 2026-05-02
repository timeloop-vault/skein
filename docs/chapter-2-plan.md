# Chapter 2 — From prototype to usable

After chapter 1 the codebase still pretends to be the prototype it was
imitating: seeded fictional sessions (s1–s5), static TUI panels, the
13-step scripted tour, the macOS traffic-light decoration. Real and
fake live side-by-side, which is confusing for a user and noise for
the codebase.

Three behavioral gaps also block "leave it open all day" use:

1. PTYs die when you switch session tabs.
2. Closing Skein loses the agent conversation.
3. When a harness's CLI exits, the pane becomes dead — you can't keep
   typing.

Plus the prototype is uncomfortable to live in for long: fixed font
size, fixed pane split, no good way to reach back to the harness after
a few minutes of moving around.

Chapter 2 strips the scaffolding, fixes those three gaps, makes it
comfortable, and lands a CLAUDE.md.

## Phase 1 — Strip the design scaffolding + write CLAUDE.md

**Goal:** every line in the codebase corresponds to something running.

- Delete `INITIAL_SESSIONS`, `SESSION_DATA`, and orphaned types
  (`TreeNode`, `ActiveFile`, `PlanItem`, `ActivityEvent`,
  `SessionData`, fixture-shape `DiffLine`).
- Delete the static TUI panels (`ClaudePanel`, `OpenCodePanel`,
  `CopilotPanel`, `ByohPanel`, `ByohResolvedPanel`,
  `CopilotErroredPanel`). `HarnessBody` collapses to "render a
  `LiveTerminal`."
- Delete the scripted tour (`TOUR_STEPS`, `TourOverlay`, launch button,
  `preTourSessions` machinery). Originals stay in `docs/design/`.
- Delete the right-pane tab strip — those tabs only had meaning with
  fixture data. Always show `LiveStatus` for sessions with a `cwd`.
- Delete the macOS traffic-light decoration.
- Settings strip: keep theme + density, drop "Reset to empty" /
  "Restore samples".
- First-run UX is the empty state. No seeding.
- New: **`CLAUDE.md`** at repo root. Drafted now while context is
  fresh; landed in the same commit as the cleanup.

## Phase 2 — Quality-of-life UI

**Goal:** Skein gets out of the way.

- **Live font size.** Runtime control on xterm font (12–18 pt range),
  `Ctrl++` / `Ctrl+-` shortcuts, `+`/`-` pair in the settings strip.
  Persisted.
- **Resizable splits.** Drag-divider between the harness column and
  the right pane. Drag-divider between LiveStatus's file list and its
  diff view. State persisted across restarts.
- **Settings strip rework.** The corner `<div>` works but feels like
  a debug overlay. Moves into a small toolbar slot in the title bar
  or becomes a popover. Keep contents minimal.
- **Empty state polish.** "No data for this session yet" copy goes
  away (no more sessions without `cwd`). LiveStatus's empty-diff
  placeholder gets a less placeholder-y look.

## Phase 3 — Cross-session PTY persistence

**Goal:** switching session tabs doesn't kill running harnesses.

- Lift the "all harnesses mounted" pattern from inside the active
  session to the whole app.
- Every session's harnesses stay mounted with `display: none` on
  inactive sessions. xterm scrollback and the PTY child both survive
  tab switches.
- Verify the X on harness tabs actually `pty_kill`s — that's the
  explicit "free this resource" path.
- Memory cost is real (~30 MB per `pwsh` × however many tabs you keep
  open), but: **users decide**. No LRU eviction in chapter 2.

## Phase 4 — PTY exit → reusable shell

**Goal:** when a harness's CLI exits, the pane stays useful.

- **Rust:** `PtyManager` detects child exit. The same channel emits a
  synthetic last frame: `\n[skein] claude exited (0)\n`.
- **Frontend:** `LiveTerminal` writes a follow-up footer line
  `Press Enter for shell, R to retry.` and intercepts the next
  keystroke (without forwarding to the dead PTY). **Auto-prompts the
  moment exit is detected** — no waiting for the user to notice.
- **Enter:** respawn the user's default shell into the same xterm.
  Scrollback intact. Update the harness's stored `cmd` so a Skein
  restart re-spawns as shell, not the dead CLI.
- **R:** respawn the original `cmd` again.

## Phase 5 — Harness resume on Skein restart

**Goal:** closing Skein doesn't lose your agent conversation.

- Per-kind resume strategy. Phase 5a uses the simplest path that works:
  - Claude Code → `claude --resume` (Claude shows a picker; user picks
    the right conversation).
  - opencode → `opencode --continue` (resumes most recent in the cwd).
  - gh copilot → no resume; fresh start.
  - Shell → fresh shell.
- On Skein boot, after `db_load_sessions`, the harness's stored `cmd`
  is rewritten to its resume form before mount. So Claude sessions
  transparently re-attach when LiveTerminal spawns.
- **Phase 5b later:** capture the actual session id (Claude writes to
  `~/.claude/sessions/`, opencode similar) and resume directly to
  *this* harness's conversation — no picker. Tracked in
  [`backlog.md`](./backlog.md).

## Out of scope for chapter 2

See [`backlog.md`](./backlog.md) — anything we considered for
chapter 2 but pushed out has been merged there.
