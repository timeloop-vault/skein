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
