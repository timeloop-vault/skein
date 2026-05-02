# Chapter 3 — Skein you can leave open all day

After chapter 2 the prototype runs the way the design promised: real
PTYs that survive tab switches, real worktrees, real diffs, harnesses
that resume their conversations on restart. The shell is honest about
what's mounted and persists what matters.

It still doesn't *feel* like an app you'd reach for instead of VS
Code. Specifically:

1. **No window controls.** `decorations: false` in `tauri.conf.json`
   was paired with a fake macOS traffic-light decoration; chapter 2
   phase 1 deleted the decoration without restoring real controls. The
   only way to close Skein is Alt+F4 / taskbar right-click.
2. **Chrome doesn't scale.** Phase 2 made the *terminal* font
   adjustable (12–18 pt) but left the rest of the UI at fixed 10–11 px.
   On a 4K monitor the titlebar wordmark, settings group, session-tab
   subtext, and status bar are all squint-small.
3. **No keyboard navigation.** Switching sessions, opening a new one,
   adding a harness — every common action requires the mouse. The
   empty state advertises ⌘N / ⌘⇧H / ⌘K but none of those are wired.
4. **Nothing keyboard-first to reach for.** No command palette, no
   fuzzy switcher across sessions and harnesses.

Chapter 3 closes those four gaps. The output isn't a wow-screenshot
feature — it's the difference between "demo you can run" and "thing
you live in."

## Phase 1 — Real min/max/close

**Goal:** the window has standard controls and behaves like every
other desktop app.

- Render Tauri-driven controls inside our custom titlebar:
  `getCurrentWindow().minimize()`, `.toggleMaximize()`, `.close()`.
- Keep `decorations: false` so the in-titlebar settings group stays.
  (The alternative — flipping decorations on — gets us OS-native
  buttons but loses the room for our controls and breaks the
  drag-region layout. Custom keeps everything together.)
- Three small SVG / glyph buttons, sized to match the 30 px titlebar
  height, opting out of `data-tauri-drag-region` so clicks register.
- Standard Windows ordering (min, max, close — close on the far
  right; close hover state in red). macOS ordering is a future
  platform detection if we get there.
- Keyboard parity: `Ctrl+W` close-session is wired in phase 3 below;
  Alt+F4 keeps working as the OS-level close.

## Phase 2 — UI scale preference

**Goal:** Skein's chrome is readable on a 4K monitor without
sacrificing the dense layout on a 1366×768 laptop.

- Add a `uiScale` preference (default `1.0`, range ~`0.85`–`1.4` in
  `0.05` steps). Persisted alongside theme/density via `usePersistedState`.
- Apply as a CSS variable multiplier on the static `font-size` /
  `height` values in `styles.css`. Either:
  - `--ui-scale` on the root and rewrite affected sizes as
    `calc(11px * var(--ui-scale))`, or
  - Set `font-size` on `.sk-app` and convert chrome sizes to `em`.
  Pick whichever rewrites fewer lines.
- Affected: titlebar (height, wordmark, settings group), session
  tabs (row + subtext), harness tabs, status bar, modal labels,
  empty-state hints. The terminal font is independent — it has its
  own pref and stays untouched.
- Setting lives in the titlebar settings group as a `−` / value /
  `+` pair next to the existing terminal-font controls. Keep them
  visually distinguishable (label or icon — chrome vs term).

## Phase 3 — Keyboard shortcuts

**Goal:** the four common actions are reachable without the mouse.

The window-level keydown listener already exists for font size — we
extend it. None of these conflict with xterm input because the custom
key handler short-circuits on the modifiers we use (Ctrl + a number /
letter that xterm wouldn't normally consume meaningfully).

- `Ctrl+N` — open the New Session dialog.
- `Ctrl+Shift+H` — open the harness picker for the active session.
- `Ctrl+W` — close the active session (with the existing confirm).
- `Ctrl+Tab` / `Ctrl+Shift+Tab` — next / previous session.
- `Ctrl+1`–`Ctrl+9` — jump to session N (1-indexed in tab order).
- `Ctrl+K` — open the command palette (phase 4).

The empty-state hints already advertise some of these — update copy
to match what's actually wired. Document the full list inline in
`README.md` or a `docs/keybindings.md` (low effort, useful when we
forget which is which).

**Out of scope:** harness-internal shortcuts (jump to harness N
within a session). Add later if the friction shows up.

## Phase 4 — Command palette (⌘K / Ctrl+K)

**Goal:** one keystroke to switch to anything; one keystroke to do
anything common.

- Small modal, fixed-width (~520 px), single text input + filtered
  list. Up/Down navigates, Enter invokes, Esc dismisses.
- Items, in this order:
  - Every session — label like "session: kit · feat/x". Selecting
    activates that session.
  - Every harness across all sessions — label "harness: claude in
    kit · feat/x". Selecting activates the parent session and that
    harness.
  - Built-in commands: "New session", "Add harness", "Close session",
    "Toggle theme", "Focus terminal", "Focus status pane".
- Fuzzy filter: lowercase substring match against label is enough at
  prototype scale; if it ever matters, lift `fuzzysort` (~5 KB).
- Reuse the existing `.sk-modal*` classes for the chrome — the New
  Session dialog already covers the visual language.
- No-library implementation (~120 lines). `cmdk` and similar add
  bundle weight for ergonomics we don't need yet.

## Phase 5 — Polish pass

**Goal:** the long tail of "this looks like a prototype" details that
each took ten seconds to spot.

- **Status bar legibility.** Tokens, model, branch — all currently
  10 px. Bump to 11 with the chapter's UI-scale system, but also
  reconsider what's load-bearing. (Branch is. Tokens-from-the-design
  is hardcoded "0 tok" until we parse — do we hide it?)
- **Active-session name in the titlebar.** Right now the titlebar
  just says "skein". When you have 4 sessions open, surfacing the
  current task there is free orientation.
- **Cwd ellipsis behavior.** `LiveStatus`'s header truncates the cwd
  with text-overflow, but a long worktree path squeezes the Refresh
  button. Either right-align the cwd's start instead of its end, or
  reserve a min-width for the button.
- **Harness-tab close behavior with one harness.** Today clicking ×
  on the last harness in a session is a silent no-op (the handler
  returns early). Either disable / hide the × in that case, or wire
  it to "close session?" via the same confirm.
- **First-run hints.** Empty state advertises shortcuts; once we've
  wired them in phase 3, ensure the hints reflect what works.

## Out of scope for chapter 3

See [`backlog.md`](./backlog.md) — anything we considered but pushed
out has been merged there. Notably:

- **Terminal / PTY rework.** R-retry of TUI programs is fragile; the
  fix is lifting VS Code / wezterm's terminal embedding patterns
  rather than reinventing. Big enough to be its own chapter.
- **Settings panel proper** (API keys, default harness, permission
  mode) — the in-titlebar group stays minimal.
- **Phase 5b transparent resume** (Claude session-id capture, no
  picker on restart).
- **Cross-harness activity feed** and **syntax-highlighted diffs**.
