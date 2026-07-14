# Handoff: Files pillar — "Files is a harness" (issue #49, stage 2)

For a developer implementing this in the Skein codebase with Claude Code.

## Overview
A file browser + text editor for Skein. The pillar decision: **the file surface is
not an overlay, a separate window, or a fixed third column — it is a new harness
type.** A room already holds N harnesses (Claude Code, opencode, shell), each a tab
in the harness tab row, each a backing process, only one **body** visible at a time.
We add one more kind: **Files**. You add it from `+ harness` like a shell, you can
open several per room, you switch to it like any other harness tab.

## About the design files
The files in this bundle are **design references created in HTML/React-via-Babel** —
a prototype showing intended look and behaviour, **not production code to lift**.
The task is to **recreate this in Skein's real environment** (Tauri + React +
CodeMirror), using the app's established patterns (`app/src/styles.css` tokens, the
existing harness/room/Live-Context components). Read the prototype to lift exact
values (hex, spacing, type, class structure) and interaction logic — then build it
natively. The Babel/`window.*` component wiring is a prototyping convenience; do not
reproduce it.

## Fidelity
**High-fidelity.** Colours, typography, spacing, and interactions are final and use
the shipped Skein token system verbatim (no new tokens, no new colours). Recreate
pixel-close using the codebase's existing CSS variables and component conventions.
The one thing that is illustrative, not final: the incoming-edit **streaming
cadence** (§ Interactions).

## The core UX thesis (read this first)
1. **Reuse a concept the user already has.** "It's a harness, like a shell" is the
   entire mental model. No new placement mode, no floating window (that was the
   rejected interim overlay).
2. **The focus-leak bug is solved structurally, not with a guard.** The interim
   overlay floated over a live terminal, so editor keystrokes could leak into a PTY.
   Harness bodies are **mutually exclusive tabs** — when a Files body is showing, no
   terminal body is mounted-visible, so a keystroke *cannot* reach a hidden PTY. Keep
   the status-bar "who has the keyboard" readout, but the safety is the architecture.
3. **It scales down honestly.** One body at a time is right for a laptop; an
   ultrawide just gets a wider body plus Live Context Full. We are **not** building a
   side-by-side "everything at once" Column layout yet — that only makes sense once
   harnesses can *detach* into their own panes, which is future work. Nothing here
   blocks it: a Files harness is already a self-contained body a future pane can host.

## Screens / Views

### View: Files harness body
Occupies the harness column body (the slot a terminal would fill). Two regions:

**A. Sidebar tree** (left)
- Layout: fixed-ish column, `flex: 0 1 184px; min-width: 120px`, right border
  `1px var(--line)`, vertical scroll, `6px 4px` padding.
- Rows: `height: 21px`, `display:flex; gap:6px`, `padding: 0 8px`, `border-radius:3px`,
  font JetBrains Mono `11.5px`. Indent = `6 + depth*12` px via inline `padding-left`.
- Dir row: `▸`/`▾` chevron (`var(--fg-3)`, 9px) + name (`var(--fg-1)`). Click toggles.
- File row: 10px dot spacer + name (`var(--fg-2)`; touched → `var(--fg-0)`).
  Active file: `background: var(--bg-3); color: var(--fg-0)`. Agent-touched file: a
  right-aligned `●` pip in `var(--accent)`, pulsing (`sk-pulse 1.6s`).
- Hover: `rgba(255,255,255,0.04)` dark / `rgba(0,0,0,0.04)` light.
- Order: directories first, then files by most-recently-modified. Lazy-expand: each
  dir expand = one `list_dir` call. Watcher-refreshed.

**B. Editor** (right, `flex:1`)
- **Buffer tab row** (`height:32px`, `bg var(--bg-0)`, bottom border, `overflow-x:auto`,
  scrollbar hidden): one tab per open buffer. Tab = name (JetBrains Mono 11px) +
  dirty dot OR close ×. Active tab: `bg var(--bg-1)`, 2px accent top-bar. A tab whose
  file an agent is editing shows a pulsing 2px accent bottom-bar + a 6px accent live
  dot. Dirty state: `●` in `var(--fg-0)` replaces the × until saved.
- **Slim head** (`height:30px`, `bg var(--bg-0)`): breadcrumb of the active path
  (segments in `var(--fg-2)`, `/` separators in `var(--fg-3)`, leaf in `var(--fg-0)`);
  right-aligned find toggle `⌕`. When no file open: `no file open · ⌘P to jump`.
- **Code area** (`fp-code`): line-number gutter `flex:0 0 46px; text-align:right;
  padding-right:14px; color var(--fg-3); 10.5px`, source `white-space:pre; 12px;
  line-height:1.55; color var(--fg-1)`. Syntax colours (dark):
  keyword `#c97163`, fn/decl `#d4a657`, string `#7aa37f`, number `#b08bd4`,
  comment `var(--fg-3)`. (Light variants in the CSS.) Find-hit line:
  `background: color-mix(in srgb, var(--warn) 16%, transparent)`.
- **Find bar** (`⌘F`, `height:34px`): glyph + input (`bg var(--bg-2)`, focus border
  `var(--accent)`) + "N lines" count + ↑ ↓ × .
- **Empty state**: centered `◇` glyph + "No file open" + `⌘P` hint.

### Component: harness tab row (modified existing component)
- The existing tab row (`.sk-harness-tabs`, `height:34px`) gains a **Files** tab kind.
- Structure: a **horizontally-scrollable tab list** (`flex:1; min-width:0;
  overflow-x:auto; scrollbar hidden`) followed by a **pinned** `+ harness` button
  (`flex:0 0 auto`, left divider `1px var(--line)`). This pinning is load-bearing —
  an earlier version clipped `+ harness` off-screen at laptop width. The active tab
  auto-scrolls into view (set `scrollLeft`, do **not** use `scrollIntoView`).
- Files tab: state dot + `◇` glyph in `var(--accent)` (deliberately **not** a coloured
  harness chip — Files isn't a process) + name + ×. Active Files tab tints
  `color-mix(in srgb, var(--accent) 8%, var(--bg-1))`.
- Cross-harness nudge: when an agent edits a file open in a Files harness you are
  **not** viewing, that tab shows a pulsing accent live-pip.

### Component: `+ harness` menu (portalled)
- Items: `Claude Code · opencode · shell · — · Files` (Files tagged "editor"). Selecting
  Files creates a Files harness and focuses it. Must be portalled to `document.body`
  (fixed-position via the button's `getBoundingClientRect`) so the column's
  `overflow:hidden` can't clip it.

### Component: Live Context (existing, unchanged placement) — Full ⇄ Peek
- Stays on the right. **Full** = the card stack (Diff / Plan / Activity). **Peek** =
  a 46px rail (`LIVE CONTEXT` vertical label + `● 3 diff · 5 act` pips), click to
  expand. Default: Full on wide, Peek on laptop; persist per room.

## Interactions & Behavior

### The hero: an agent edits a file you have open
Skein's normal case, not an edge case (a 200ms watcher already fires). When the
watcher reports a change to a file that is the **active buffer of some Files
harness**, attributed to a harness:
- That buffer shows an **inline split**: user's version left ("Yours"), agent's
  incoming version right ("<harness> · incoming"), incoming lines tinted
  `--diff-add`, streaming in progressively so the user perceives typing. A change
  bar sits above with a `LIVE` pill.
- **Clean buffer** → after the edit lands, the buffer **auto-follows disk** (split
  collapses to new content) with a brief `clean buffer followed disk · Undo`.
- **Dirty buffer** → split **persists**; change bar shows `Keep mine · Take theirs ·
  Compare ↗`. Never silently overwrite. "Compare" jumps to the Live Context Diff card
  (do not build a third merge surface).
- If the edited file is open in a Files harness you're **not** viewing: that tab
  pips + the Diff tab flickers; clicking the tab lands on the live split.
- Coalesce a burst of edits to one file into a single streaming pass.
- **Streaming cadence is illustrative** — the prototype reveals ~1 line/120ms after a
  380ms lead-in. Tune to feel right against real watcher timing; snap tiny edits.

### Other flows
- **Add / switch / close** harnesses: identical to existing harness/shell behaviour;
  Files is just another kind. Closing a Files tab with dirty buffers prompts
  `Save · Discard · Cancel`.
- **Save** `⌘S` clears the buffer dirty dot (first consumer of the stage-2 write cmd).
- **Find in file** `⌘F`. **Quick-open** `⌘P` — fuzzy over the room's files, ↑↓ / ↵ /
  esc, live pip on agent-touched files; opens into the active Files harness (or first,
  or creates one).
- Bodies of inactive harnesses stay mounted (`display:none`) so PTYs keep running and
  editor state survives tab switches — match the existing harness-switch behaviour.

## State management
- New harness kind `files`. Harness list already exists; extend it.
- Per Files harness: `{ buffers: string[], active: string | null }` (independent —
  two Files harnesses can hold the same file as separate views).
- Global-by-path: `dirty: Set<path>`, and the on-disk content (what the watcher/agent
  writes). The live split reads user-buffer vs incoming-disk.
- Live Context: `full | peek` per room.
- Watcher event → if path is the active buffer of any Files harness, enter split mode
  for that buffer; else just pip the relevant tab.

## Keyboard (constraint: ⌘/Alt + letter/digit only — Swedish layout, no punctuation)
`⌘E` jump to a Files harness (create if none) ⇄ back to last terminal · `⌘P`
quick-open · `⌘S` save · `⌘F` find-in-file. No collision with the taken set
(K/N/W/J/L, arrows, 1–9). Confirm `⌘F` with the owner before shipping.

## Design tokens
All from `app/src/styles.css` — **no new tokens**. Used here: `--bg-0..3`, `--bg-card`,
`--line`, `--line-strong`, `--fg-0..3`, `--accent`, `--ok`, `--warn`, `--err`,
`--waiting`, `--diff-add`, `--diff-del`, `--st-running/waiting/idle/error`, harness
chip colours `--h-claude/opencode/copilot/byoh`. Fonts: JetBrains Mono (code + chrome
metadata), Inter (prose). Radii 3–10px, as in the prototype CSS.

## Editor engine
**CodeMirror 6.** Covers line numbers, syntax highlight, find/replace, undo/redo,
multi-cursor at ~1/10th Monaco's weight. Keep the live split a thin custom layer over
two CM6 instances so the incoming pane can stream. We deliberately avoided the one
Monaco-worthy feature (a rich merge editor).

## Explicitly NOT in v1
Column / multi-pane layout · pane detach · rendered previews (markdown/image) · files
outside the room cwd · a merge editor · unifying Diff + the file surface · point-in-
time diffs · search-across-files (find-in-file only).

## Open questions for the owner
- Does `⌘E` create a Files harness if none exists, or no-op? (Prototype creates.)
- Per-Files-harness buffer LRU cap (~12?).
- Clean-buffer auto-follow: keep the Undo affordance, or is the Diff card enough?
- Full-vs-Peek default: purely width-driven, or remembered per room?

## Files in this bundle
- `Files Pillar Prototype - bundled.html` — the self-contained prototype (open in a
  browser; drives every state via the right-side **director rail**, which is
  prototype-only scaffolding and NOT part of the product).
- `Files Pillar - Implementation Handover.md` — the fuller spec (Q&A-mapped rationale).
- Prototype source (reference for exact values/logic; do not port the Babel wiring):
  `Files Pillar Prototype.html` (all CSS lives here), `fp-shell.jsx` (chrome + harness
  tabs), `fp-surface.jsx` (tree/editor/split/quick-open), `fp-app.jsx` (harness + buffer
  state, keyboard, scenario), `fp-data.jsx` (sample data + the agent-edit tape).
