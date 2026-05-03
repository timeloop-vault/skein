# Backlog

Parked ideas. Things we've thought about but decided to defer beyond the
active chapter. Lives outside any chapter so it survives context resets.

Format: short title, one-line "what", brief "why parked." When something
moves into an active phase, delete it here.

## UX surfaces

- **Settings panel proper** — API keys per harness kind, default
  starting harness, default worktree placement, permission mode. The
  cog modal currently covers theme/density/font/scale only because
  we've kept the surface tiny.
- **Real onboarding tour** — chapter 1's tour was fixture-driven and
  got deleted in chapter 2 phase 1. A real "create your first room"
  walk-through is a separate product surface, not a fixture re-skin.
- **Cross-harness activity feed** — the design has "h1b flagged X in
  h1a's diff" rows. Requires understanding tool boundaries inside
  opaque TUI output. Defer until we have a use case.
- **Multi-git parent picker** — when a folder contains several child
  repos, let the user pick which child to tie the room to. Chapter 6
  phase 3 treats these as plain non-git cwd; this would add the
  "pick a child or use the parent" picker on top.
- **Worktree cleanup on archive** — closing a room (chapter 6 phase 2)
  doesn't `git worktree remove` the worktree dir. Add a "delete
  worktree on close" toggle when the surface justifies it.

## Display polish

- **Syntax highlighting in the diff view** — current diff is plain
  monospace with `+`/`-` colouring. `web-tree-sitter` would add
  per-language highlighting.
- **BYOH permission UX** — only meaningful if we ever build our own
  agent loop. Each harness handles its own permission flow today.

## Infra

- **Multi-window** — single window is fine for v0.
- **Auto-updater, code signing, installers** — chapter 8 (was the
  "B" of A→C→B; bumped after chapter 6 took the A slot).

## Migration candidates

- **React → Preact** — Preact is ~3 kB vs React's ~45 kB with a
  near-compatible API (`react` aliases to `preact/compat`). Shrinks the
  bundle meaningfully without touching component code. `poe-inspect-2`
  uses this exact recipe under Tauri, so it's well-trodden. Wait until
  the UI shape settles — chapters 6 and 7 reshape a lot of it.
- **PTY / terminal embedding rework** — chapter 7. The chapter 2
  phase 4 R-retry path corrupts the visible state when the previous
  child was a TUI (Claude, opencode). The handover between two
  alt-screen-using programs in the same xterm requires careful state
  management (force-exit alt screen, clear viewport, send SIGWINCH
  to nudge the new child to redraw, handle Windows ConPTY's quirk
  of keeping the reader pipe open after child exit). VS Code's
  terminal and wezterm have solved this; lift their patterns. Study
  `vscode/src/vs/platform/terminal/node/terminalProcess.ts` and
  wezterm's `pty` crate.
