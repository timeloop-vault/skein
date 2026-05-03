# Backlog

Parked ideas. Things we've thought about but decided to defer beyond the
active chapter. Lives outside any chapter so it survives context resets.

Format: short title, one-line "what", brief "why parked." When something
moves into an active phase, delete it here.

## UX surfaces

- **Settings panel proper** — API keys per harness kind, default
  starting harness, default worktree placement, permission mode. The
  corner strip covers theme/density only because we've kept the surface
  tiny.
- **Terminology cleanup: "session" is overloaded.** In Skein "session"
  means a top-level tab (workspace = repo + branch + task). In Claude,
  opencode, and the chapter 5 code it means the agent's conversation
  id. Both end up on `Harness` (`session.id` for the workspace,
  `harness.sessionId` for the conversation), and conversations end up
  fragile to talk about — "the opencode session for this session" is
  the kind of sentence we now have to write. Pick clearer terms before
  the surface grows: e.g. **workspace** for the top-level tab and
  **conversation** for the agent-tool id, or **room** + **thread**.
  Naming is its own small chapter; do it before chapter 7's docs pass
  so the public-facing copy lands once.
- **Persistent workspace history.** `closeSession` deletes the session
  outright — it's gone from `skein.db` the next time
  `db_save_sessions` fires, and there's no "reopen recent" surface.
  We want closed workspaces to be recoverable: either soft-delete
  (archive flag, hidden from the tab strip but listed in a "Recent"
  picker) or a separate "workspaces" table the active session list
  borrows from. Pairs naturally with the terminology cleanup since
  this is the place the words `workspace` and `session` start to mean
  different things.
- **Folder picker shouldn't require a git repo.** `NewSessionDialog`
  gates the commit button on `git_is_repo`; non-git folders fall to a
  "not a repo" state and the dialog can't proceed. Real workflows
  break out of this: a parent dir containing multiple gits (mono-style
  workspace), a notes / scratch dir with no repo at all, an opencode
  config dir. Make git optional — when the folder is a repo, keep the
  branch / worktree picker; when it isn't, drop straight through to
  "harness in this cwd, no worktree." Need to decide what to do for
  multi-git parents (let the user pick which child? treat as plain
  cwd? offer both?) — that's the design question, not "should we
  support it."
- **Command palette (⌘K)** — fuzzy switcher for sessions, harnesses,
  and files. Most demoable single feature we don't have. Worth a
  chapter on its own when keyboard-first usage starts hurting.
- **Full keyboard-shortcut surface** — beyond the few that land in
  chapter 2 (font size, ⌘N). Switch session N (⌘1–9), close session,
  jump between harnesses, focus terminal vs status pane.
- **Real onboarding tour** — chapter 1's tour was fixture-driven and
  gets deleted in chapter 2 phase 1. A real "create your first session"
  walk-through is a separate product surface, not a fixture re-skin.
- **Cross-harness activity feed** — the design has "h1b flagged X in
  h1a's diff" rows. Requires understanding tool boundaries inside
  opaque TUI output. Defer until we have a use case.

## Display polish

- **Syntax highlighting in the diff view** — current diff is plain
  monospace with `+`/`-` colouring. `web-tree-sitter` would add
  per-language highlighting.
- **BYOH permission UX** — only meaningful if we ever build our own
  agent loop. Each harness handles its own permission flow today.
- **Chrome scaling / window controls** — chapter 2 phase 2 sized the
  *terminal* font but left the rest of the chrome at fixed 10–11 px:
  the titlebar wordmark, the settings group inside it, the session
  tab subtext, the status bar. They're readable on a 1320×820 default
  but small on a 4K monitor. Either a global UI-scale preference
  (multiplier on `font-size`) or just bigger fixed sizes. Same patch
  also needs to ship working min/max/close controls — `decorations:
  false` in `tauri.conf.json` means the OS doesn't draw any, and
  phase 1 removed the fake traffic-light decoration that was
  pretending to be there. Either flip to `decorations: true` (loses
  the in-titlebar settings group on Windows) or render real
  Tauri-driven controls (`getCurrentWindow().minimize()` /
  `toggleMaximize()` / `close()`).

## Infra

- **Multi-window** — single window is fine for v0.
- **Auto-updater, code signing, installers** — we run `cargo run` for
  now.
- **Transparent harness resume** (chapter 2 phase 5b) — capture agent
  session ids from on-disk files (Claude writes to
  `~/.claude/sessions/`, opencode similar), resume directly to *this*
  harness's conversation, no picker. Phase 5a uses the picker.

## Migration candidates

- **React → Preact** — Preact is ~3 kB vs React's ~45 kB with a
  near-compatible API (`react` aliases to `preact/compat`). Shrinks the
  bundle meaningfully without touching component code. `poe-inspect-2`
  uses this exact recipe under Tauri, so it's well-trodden. Wait until
  the UI shape settles — chapter 2 reshapes a lot of it.
- **PTY / terminal embedding rework** — the chapter 2 phase 4 R-retry
  path corrupts the visible state when the previous child was a TUI
  (Claude, opencode). The handover between two alt-screen-using
  programs in the same xterm requires careful state management
  (force-exit alt screen, clear viewport, send SIGWINCH to nudge the
  new child to redraw, handle Windows ConPTY's quirk of keeping the
  reader pipe open after child exit). VS Code's terminal and wezterm
  have solved this; we shouldn't reinvent it. When we revisit, study
  `vscode/src/vs/platform/terminal/node/terminalProcess.ts` and
  wezterm's `pty` crate, and likely lift the patterns wholesale.
  Phase 4 ships with Enter-for-shell working reliably; R works for
  non-TUI cmds (shells) and is flaky for TUIs.
