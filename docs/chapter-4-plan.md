# Chapter 4 — Cross-platform parity

After chapter 3 the prototype has window controls, scaled chrome,
keyboard navigation, and a command palette — but every line of UI work
so far happened on Windows. On macOS Skein *runs* (PTYs spawn, git
worktrees create, sqlite persists) but it looks and feels Windows-
flavoured:

1. **Shortcuts gate on Ctrl.** macOS convention is ⌘ for app
   shortcuts, and reusing Ctrl conflicts with terminal control codes
   (Ctrl+C, Ctrl+W in the shell). Today on macOS, hitting Ctrl+W to
   delete-word in `bash` instead closes your session.
2. **Window controls are made-up buttons on the right.** macOS
   convention is OS-drawn traffic lights on the left. We're competing
   with the OS on something the OS already does well.
3. **No macOS app menu.** Mac users expect at least ⌘Q and an Edit
   menu (Cut/Copy/Paste/Select All). Without one Skein feels
   web-shimmed.
4. **Settings strip in the titlebar carries five controls.** Theme,
   density, font −/+, scale −/+ — fine on a wide laptop, cramped
   alongside whatever phase 2 puts on the left.

Chapter 4 closes those four gaps. The output isn't a new feature —
it's the difference between "runs on macOS" and "feels like a Mac app
on macOS, a Windows app on Windows."

We use the OS where the OS already has a strong convention (traffic
lights, app menu, Cmd vs Ctrl). We don't invent cross-platform
abstractions for things that work fine as-is (PTYs, paths, sqlite).
The principle is *use OS, not made-up* — but only where the cost of
made-up is high.

## Phase 1 — Platform mod key

**Goal:** ⌘ does on macOS what Ctrl does on Win/Linux.

- New `isModKey(e)` helper next to `isAppShortcut` in
  `app/src/shortcuts.ts`. Returns `e.metaKey` on Mac, `e.ctrlKey`
  elsewhere. Single source of platform detection in the frontend; a
  `usePlatform()` hook can wrap `navigator.platform.startsWith("Mac")`
  if more places need it.
- `shortcuts.ts` swaps every `e.ctrlKey` for `isModKey(e)`. Same with
  `App.tsx`'s font-size handler and any other ctrl-gated checks.
- Empty-state hint copy and the keybindings doc render `⌘` on Mac,
  `Ctrl` on Win/Linux. Same trick: a tiny formatter that picks the
  glyph by platform.
- Frees up Ctrl on Mac so terminal control codes (Ctrl+C, Ctrl+W in
  shells) reach the PTY without competing with app shortcuts.

**Out of scope:** rewriting `xterm`'s custom key handler. It already
checks `isAppShortcut`, which is the seam we're patching.

## Phase 2 — OS-native window chrome

**Goal:** macOS draws traffic lights; the OS we're running on decides.

- `tauri.conf.json` gets a per-platform window block:
  - **macOS:** `titleBarStyle: "Overlay"` and `hiddenTitle: true`. The
    OS draws standard traffic lights at the upper-left of our custom
    titlebar, no chrome of its own.
  - **Windows / Linux:** unchanged. `decorations: false` + the custom
    three-button block from chapter 3 phase 1 stay.
- `.sk-titlebar` reserves ~70 px left padding on macOS so the wordmark
  and the traffic lights don't overlap. CSS does this with a
  `[data-platform="mac"]` selector applied at app boot from the
  `usePlatform` helper.
- The custom min/max/close block in `App.tsx` renders only when
  `!isMac`. One conditional, no other branching.
- Drag region (`data-tauri-drag-region`) keeps working — overlay mode
  on macOS still lets the OS handle window dragging on the rest of
  the titlebar.

**Out of scope:** custom traffic-light positioning beyond macOS
defaults, Windows-style overlay buttons, Linux GNOME headerbar
integration.

## Phase 3 — macOS app menu

**Goal:** Mac users get the menu bar Mac users expect.

- Tauri v2 menu builder in `app/src-tauri/src/lib.rs`:
  - **Skein** → About Skein, Preferences… (⌘,), Hide Skein (⌘H),
    Hide Others, Show All, Quit Skein (⌘Q)
  - **Edit** → Undo, Redo, Cut, Copy, Paste, Select All
  - **View** → reserved for later (zoom, full-screen if we want)
- Edit menu uses Tauri's predefined `MenuItem`s so they target the
  focused element automatically — xterm selection, modal text input,
  command-palette query, etc. No per-surface wiring.
- Preferences… emits a `skein://open-settings` event the frontend
  listens for and opens the phase-4 settings modal.
- Cmd+Q quits via the standard menu item — no custom binding.
- **Windows / Linux:** no menu added. Their convention is in-window
  controls, which Skein already has via the command palette and
  keyboard shortcuts.

**Out of scope:** Window menu, Help menu, dock context-menu items,
status-bar icons, anything outside the standard application menu pair
(Skein + Edit).

## Phase 4 — Settings panel

**Goal:** the in-titlebar settings strip becomes a proper modal.

- New `<SettingsModal>` in `app/src/SettingsModal.tsx`, reusing the
  `.sk-modal*` chrome the new-session and command-palette dialogs
  already share. Sections:
  - **Appearance:** theme, density, UI scale.
  - **Terminal:** font size.
  Same controls as today, just relocated.
- Triggered three ways:
  - Cog icon in the titlebar, replacing the current settings cluster.
  - `Mod+,` keyboard shortcut, registered via the phase-1 helper.
  - macOS app menu **Preferences…** item from phase 3, via the
    `skein://open-settings` event.
- Titlebar after this phase: `[traffic lights on Mac]  Skein ·
  session-name  [drag region]  [cog]  [min/max/close on Win/Linux]`.
- **Out of scope (still parked in `backlog.md`):** API keys per
  harness kind, default starting harness, default worktree placement,
  permission mode. This phase moves *existing* settings to a proper
  home; it doesn't design a settings system.

## Out of scope for chapter 4

See [`backlog.md`](./backlog.md) — anything we considered but pushed
out has been merged there. Notably:

- **macOS code signing / notarization / installers.** Skein still
  runs via `cargo run` / `npm run tauri dev`. Distribution is a
  separate chapter.
- **Linux as a polished target.** Linux runs (GTK + wayland) but
  inherits Windows-style chrome with no convention work — basically
  "Windows behaviour on a different OS." If/when a Linux user actually
  picks Skein up, that becomes its own pass.
- **App icon** — still the Tauri default. Branding is its own chapter.
- **Settings panel scope creep** — see phase 4.
