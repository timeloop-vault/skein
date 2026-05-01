# Working prototype — phased plan

The current `app/` is a faithful **visual** port of the design handoff in
[`docs/design/`](./design/). Every harness pane is a hand-written TUI mock,
the file tree and diff are static fixtures, and the session list never
touches disk. This document is the plan for replacing that fakery — one
seam at a time — with real machinery.

## Goal of v0 (working prototype)

Prove the **session-first, multi-harness, shared-worktree** model is real
when wired to actual processes. Specifically:

- The user types `claude` (or picks a quickstart) and the *real* Claude Code
  TUI runs inside Skein's harness pane — keystrokes go to the PTY, output
  streams back, colors and box-drawing render correctly.
- Two harnesses (e.g. Claude + opencode) run in the same workspace, edit the
  same worktree, and Skein's right pane shows the diff updating live as
  either of them writes.
- Skein survives restart: closing and reopening it brings back the same
  sessions, harnesses, and worktrees.

We are *not* building our own agent loop, our own permission UI, or our
own model routing — those are the harness's job (BYOH). Skein's job is to
be the room that hosts them.

## Stack additions

| Concern | Choice | Rationale |
|---|---|---|
| Pseudo-terminal | [`portable-pty`](https://crates.io/crates/portable-pty) (Rust) | Cross-platform PTY (Windows ConPTY, Unix openpty), maintained, used by wezterm |
| Terminal renderer | [xterm.js](https://xtermjs.org/) + `@xterm/addon-fit` | The default. Handles ANSI, scrollback, selection, mouse |
| IPC | Tauri events (binary `Vec<u8>` payload) | Avoids JSON-encoding every byte of TTY traffic |
| Session DB | [`rusqlite`](https://crates.io/crates/rusqlite) (bundled) | Same crate poe-inspect uses, no server, file lives in `~/.local/share/skein/skein.db` |
| Git ops | [`git2`](https://crates.io/crates/git2) for worktree + diff, fall back to shelling out for anything weird | Worktree create/remove, `diff HEAD`, branch listing |
| File watcher | [`notify`](https://crates.io/crates/notify) with debouncer | Triggers a re-diff when files change |
| State management | Frontend `useReducer` + a thin `invoke()` wrapper | No global store needed yet |

The pre-commit hook stays as-is — same four checks, no new ones.

## Phases

Each phase is independently shippable and ends with a demo we can stare at.
Phases build strictly on the previous one — don't start phase N+1 until N
is real.

---

### Phase 1 — One real PTY in the harness pane *(foundational)*

**Goal:** click "Claude Code" in the quickstart, see the actual `claude`
binary running inside the harness pane.

- **Backend (Rust)**
  - `crates/skein-pty` (workspace member): wraps `portable-pty` with a
    `Pty::spawn(cmd, cwd, env, size) -> PtyHandle` API. `PtyHandle` exposes
    a `Receiver<Vec<u8>>` for output, a `write(&[u8])` for input, and a
    `resize(rows, cols)` for SIGWINCH equivalent.
  - Tauri commands:
    - `pty_spawn({ cmd, args, cwd, rows, cols }) -> { harnessId }`
    - `pty_write({ harnessId, data: Vec<u8> })`
    - `pty_resize({ harnessId, rows, cols })`
    - `pty_kill({ harnessId })`
  - Output stream: per-spawn `tauri::ipc::Channel<String>` passed in as a
    command argument. UTF-8-lossy on the Rust side; xterm.js ingests
    strings directly via `term.write(string)`. Switch to a binary channel
    later if profiling says it matters.
- **Frontend (TS)**
  - Drop xterm.js into the `term-*` panels. Replace the hand-written
    "look like Claude Code" markup with one xterm instance per harness.
  - Hook `term.onData` → `invoke('pty_write', ...)`.
  - Listen on `pty://output/{id}` → `term.write(Uint8Array)`.
  - Resize: `ResizeObserver` on the pane → `pty_resize`.
- **Scope cuts:** single harness, single session, no DB, no worktree
  integration. The session list is still hard-coded fixtures.
- **Demo:** run `skein`, hit "Claude Code" quickstart, see the real
  `claude` CLI splash, type a question, get an answer.

**Details to nail (not blockers — well-trodden ground):**
- **IPC throughput.** Tauri events JSON-encode payloads, which is fine for
  small UI signals but painful for a TUI dumping a screenful of ANSI in
  one frame. Use `tauri::ipc::Channel` per spawn rather than `emit` — it
  delivers in order and handles backpressure, and we can swap to a binary
  payload type later without changing the Rust API.
- **Windows ConPTY redraws.** TUIs that toggle alt-screen (`claude`,
  `vim`, anything fullscreen) have historically had occasional resize
  bugs on ConPTY. `portable-pty` smooths most of them; verify with
  `claude` early so we catch the rough edges before building on top.
- **Env forwarding.** Set `TERM=xterm-256color` and `COLORTERM=truecolor`
  by default, and forward the user's `PATH` / `HOME` / locale vars so
  spawned binaries can find their auth files and config.

---

### Phase 2 — Two PTYs per session, shared worktree

**Goal:** the cross-harness story from the design becomes real. Add a
second harness inside the same session, both run in the same `cwd`,
edits one makes are visible to the other on disk.

- Lift `harnesses: Harness[]` out of the fixture data — track the live
  set in a Tauri-managed `Mutex<HashMap<HarnessId, PtyHandle>>`.
- Multiple xterm instances mounted simultaneously; only one is visible
  (CSS `display: none` on the inactive ones — *don't* unmount, or
  scrollback dies).
- Session-level `cwd` becomes the source of truth. Both harnesses spawn
  with that `cwd`. No worktree creation yet — point at an existing
  directory the user picks in the new-session dialog.
- **Demo:** start a session in `~/code/skein`, open Claude in one tab and
  opencode in the other, ask Claude to write a file, switch tabs, ask
  opencode to read it.

---

### Phase 3 — Session persistence (sqlite)

**Goal:** restart Skein, get back the same sessions. Closed harnesses
stay closed; live ones don't survive (PTYs die with the parent process —
that's expected).

- Schema (initial cut):
  ```sql
  CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    branch TEXT,
    task TEXT,
    created_at INTEGER NOT NULL
  );
  CREATE TABLE harnesses (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,         -- 'claude' | 'opencode' | 'copilot' | 'byoh' | 'custom'
    name TEXT NOT NULL,
    cmd TEXT NOT NULL,          -- exact command line we'd re-run
    cwd TEXT NOT NULL,          -- usually = session.repo_path, may differ
    created_at INTEGER NOT NULL
  );
  ```
- Tauri commands: `db_list_sessions`, `db_create_session`, `db_create_harness`,
  `db_delete_session`, `db_delete_harness`. No update commands yet —
  rename/move come later.
- Migration story: a single `CREATE TABLE IF NOT EXISTS` block at startup
  is enough until we ship to anyone but ourselves.
- DB lives at `dirs::data_dir().join("skein/skein.db")` — created on
  first launch.
- **Demo:** create three sessions, kill the app, relaunch, see all three
  back. Click into one, the harness tabs are remembered (closed; the
  user re-clicks "start" to re-spawn the PTY).

---

### Phase 4 — Real git worktrees

**Goal:** the "New session" dialog actually creates a worktree.

- Replace `REPO_PRESETS` with a real picker: file dialog → repo path,
  validate it's a git repo, list branches via `git2`.
- "New worktree" radio: shells out to `git worktree add <path> -b <branch>`
  in a sibling directory (e.g. `~/code/skein-wt/<task-slug>`). Path is
  configurable in settings later; for now hardcode the parent.
- "Current branch" radio: just point the session at the existing repo
  path with `git2::Repository::open` to read `HEAD`.
- Session deletion offers (but doesn't force) `git worktree remove`.
- **Demo:** new session on `skein` → "feat/foo" worktree → `cd` shows the
  branch is checked out → both harnesses see the same files.

---

### Phase 5 — Live diff in the right pane

**Goal:** kill the static `SESSION_DATA` diff. Show real changes against
the worktree's base ref.

- On session activation, compute the base ref: `git merge-base HEAD <main-branch>`.
  (Picking the main branch is its own UX problem — start with
  `origin/main`, fall back to `main`, fall back to `master`.)
- File tree: `git status --porcelain=v1` style enumeration via `git2`,
  plus the worktree's tracked files. Touched markers come from `status`.
- Diff: `git2::Diff::tree_to_workdir_with_index` against the base ref.
  Render the patch text — for a prototype, plain `<pre>` with line-level
  +/- coloring is fine; we already have the CSS.
- File watcher: `notify` on the worktree root, debounce 200ms, recompute
  the diff on the active session only. Background sessions stay stale
  until activated.
- **Demo:** ask Claude to edit a file in one harness, watch the right
  pane refresh within ~250ms.

---

### Phase 6 — Connecting it all

**Goal:** the new-session dialog from the design becomes the real
front door.

- Wire the dialog's "Create session" to: (a) `db_create_session`,
  (b) `git worktree add` (if "new worktree"), (c) auto-spawn the
  selected starting harness via `pty_spawn`.
- Idle harness terminal (`IdleHarnessTerminal`) becomes a real bare
  shell — spawn the user's `$SHELL` in the worktree, no auto-command.
  The quickstart buttons just `pty_write` the command + Enter.
- "+ harness" stops being a picker stub: opens the same harness picker
  card, on click spawns a new PTY inside the active session's `cwd`.
- **Demo:** end-to-end. Empty state → new session → real worktree →
  real Claude → "+ harness" → real opencode → both edit the same file →
  live diff → close app → reopen → sessions intact.

## Out of scope for v0

These are real, but separate concerns. Listing them here so we don't
accidentally start them:

- **Permission UX for BYOH** — only relevant if/when we build our own
  agent loop. Until then, each harness handles its own permission flow
  inside its TUI (where Claude Code, opencode, Copilot already do).
- **Activity feed across harnesses** — the design's cross-harness "h1b
  flagged X" feed is interesting but requires understanding tool
  boundaries inside opaque TUI output. Defer until we have a need.
- **Settings panel** — current corner toggles are enough.
- **Command palette (⌘K)** — nice but not load-bearing.
- **Tour overlay** — keep the current scripted tour; it still doubles
  as a self-explaining demo.
- **Multi-window** — single window is fine for v0.
- **Auto-updater, code signing, installers** — we're running `cargo run`
  for now.

## Open questions to settle as we go

1. **Where does the user's `$SHELL` come from on Windows?** PowerShell
   by default? Configurable per-session? Per-harness?
2. **Do we forward terminal bell / OSC sequences (window title)?** xterm.js
   handles them; we'd just need to decide what to do with them at the
   Skein chrome level.
3. **What happens when a harness crashes mid-session?** The design shows
   an errored state; the implementation needs to detect non-zero exit and
   surface the same UI without losing scrollback.
4. **Worktree placement**: sibling directory (`~/code/skein-wt/foo`) or
   under the repo (`./.skein/worktrees/foo`)? Tradeoffs around editor
   indexing + `.gitignore`.
5. **How does Skein know which CLI binary to run?** Hard-code `claude`,
   `opencode`, `gh copilot suggest` for v0 and let users edit the command
   in the new-session dialog. PATH discovery can wait.

## Order of operations

Go in the order above. Each phase ships something demoable. Phase 1 is
first because everything else depends on it, not because the technology
is unproven — VS Code, wezterm, Hyper, Warp, and Zed all run on the same
recipe (xterm.js + a PTY backend). The work is in the integration details
above, not in inventing anything new.
