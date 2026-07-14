# Files pillar — implementation handover

Companion to:
- `uploads/files-pillar-design-brief.md` (issue #49, stage 2 — the questions this resolves)
- `Files Pillar Prototype.html` (interactive prototype; a right-side **director
  rail** drives the demo and is NOT part of the product)
- `fp-shell.jsx` — Skein chrome (titlebar, room tabs, **harness tab row + add
  menu**, terminal/shell bodies, Live Context stack, status bar)
- `fp-surface.jsx` — the file surface (tree, buffer tabs, editor, find, the live
  change split, quick-open)
- `fp-app.jsx` — harness model, per-Files-harness buffer state, keyboard, scenario
- `fp-data.jsx` — sample worktree + the agent-edit tape

Where this doc and the brief conflict, this doc wins (post-design follow-up).

---

## 0 · The decision — **Files is a harness**

A room already holds N **harnesses** — Claude Code, opencode, shell — each a tab
in the harness tab row, each a real backing process, only one **body** visible at
a time. **The file surface becomes another harness type: "Files."** You add it
from `+ harness` exactly like you add a shell; you can open **several** Files
harnesses in a room (each with its own open buffers); you close one with its tab
×; you switch to it like any other harness tab.

Live Context stays on the right and toggles **Full ⇄ Peek** (Full has room on an
ultrawide; Peek — a 46 px rail — is the laptop default).

We are **not** building the side-by-side "Column" layout (harness + files +
context all at once) yet. Column only makes sense once harnesses/shells/Files can
**detach** into their own panes, and that machinery isn't ready. Until then, "see
everything at once" is served by: Files-harness body + Live Context (Full on wide,
Peek on laptop), with the terminal one tab-click away.

### Why this is the right shape
- **It reuses a concept the user already has.** No new placement mode, no overlay,
  no floating window (the interim overlay this replaces). "It's a harness, like a
  shell" is the whole mental model.
- **It dissolves the focus-leak bug structurally.** The interim overlay floated
  over a live terminal, so keystrokes could leak into a PTY. Harness bodies are
  **mutually exclusive tabs** — when Files is showing, the terminal body isn't,
  so a keystroke *cannot* reach a hidden PTY. The fix is the architecture, not a
  guard we have to remember. (The status bar still names who holds the keyboard.)
- **It scales down honestly.** One body at a time is exactly right for a laptop;
  the ultrawide just gets a wider body + Live Context Full.

---

## 1 · Definition of done

1. A new harness kind **`files`** exists. `+ harness` offers Claude Code /
   opencode / shell / **Files**. Adding Files creates a Files harness tab and
   focuses it. Multiple Files harnesses per room are allowed.
2. A Files harness body renders: **persistent tree** (left) + **buffer tab row +
   editor** (right). It occupies the harness column body — the same slot a
   terminal would.
3. Tree: nested, lazy-expand (each expand = one `list_dir`), dirs first then
   most-recently-modified first, live watcher refresh, accent pip on any file an
   agent has touched this session.
4. **Multiple open buffers per Files harness** with a tab row; per-buffer dirty
   dot; close ×. `Mod+P` fuzzy quick-open opens into the active (or first, or a
   new) Files harness (#48).
5. Editor = CodeMirror 6: line numbers, syntax highlight, find/replace, undo/redo,
   multi-cursor. Raw text only, no rendered previews.
6. **External-change (the hero) works** per §4: when an agent edits a file open in
   a Files harness, that buffer shows an inline split with the incoming version
   streaming in; clean vs dirty resolve differently. If you're on a *different*
   harness, the Files tab pips (§5).
7. Live Context stays on the right and toggles **Full ⇄ Peek**; Peek is a labelled
   rail with live pips, click to expand.
8. Dark and light both legible with existing tokens.

Explicitly **out of scope this stage:** the Column side-by-side layout and any
pane-detach mechanism (see §11).

---

## 2 · Design system

No new tokens. Everything is the shipped Skein system (`app/src/styles.css`):
bg/fg scale, `--accent` terracotta, `--ok/--warn/--err/--waiting`, JetBrains Mono
for code + chrome metadata, Inter for prose, harness chip colours. The Files
harness introduces **no new colours**: dirs use `▸/▾`, the Files tab/kind uses a
`◇` glyph in `--accent` (deliberately *not* a coloured harness chip, since Files
isn't a process kind), agent-touched files a `●` pip, the incoming-edit side the
accent, diff tint reuses `--diff-add / --diff-del`. Lift the file-surface + harness
CSS verbatim from `Files Pillar Prototype.html` (`.fp-surface`, `.fp-tree`,
`.fp-buftabs`, `.fp-code`, `.ed-split`, `.ed-changebar`, `.fp-qo-*`,
`.sk-harness-menu`, `.lc-peek`).

---

## 3 · The harness model, in detail (Q1)

### 3.1 Adding / switching / closing
- `+ harness` → menu `Claude Code · opencode · shell · — · Files`. Selecting Files
  creates a `files` harness and focuses it (an empty one shows the editor empty
  state + a `⌘P to jump` hint).
- A Files tab looks like other harness tabs: state dot + `◇` glyph + name + ×.
- Closing the tab closes that Files harness (prompt if it has dirty buffers — §8).

### 3.2 The body slot
The harness column body shows the active harness. For `files`, that body **is** the
file surface (tree + buffer tabs + editor). Switching harness tabs swaps the body;
the others stay mounted (`display:none`) so PTYs keep running and editor state
survives — matching how harness/room switching already works.

### 3.3 Multiple Files harnesses
Allowed, even if unusual — same as opening two shells. Each keeps its own buffer
set + active file (`filesState[harnessId]` in the prototype). Two Files harnesses
can even have the same file open; treat them as independent views (last-save-wins
on disk; the watcher keeps both honest).

### 3.4 Live Context Full ⇄ Peek (Q7)
Live Context never leaves the right side. **Full** = the card stack. **Peek** =
a 46 px rail (`LIVE CONTEXT` label + `● 3 diff · 5 act` pips), click to expand.
Default: Full on wide, Peek on laptop; persist the user's choice per room. This is
how "what is the agent doing" stays in view while you're in a Files harness on a
small screen.

---

## 4 · The agent edits a file you have open (Q3 — the hero)

Decision: **always show it live** (the owner's pick). A 200 ms watcher event on a
file that is the active buffer of some Files harness, attributed to a harness,
triggers an **inline split** in that buffer — the user's version left, the agent's
incoming version right, streaming in with `--diff-add` tint on new lines. A change
bar sits above.

Resolution depends on buffer state:
- **Clean buffer:** after the incoming edit lands, the buffer **auto-follows disk**
  (split collapses to the new content) with a brief `clean buffer followed disk ·
  Undo` affordance. No modal.
- **Dirty buffer:** the split **persists** with `Keep mine · Take theirs ·
  Compare ↗`. Nothing is silently overwritten. "Compare" jumps to the Diff card
  (§5), not a third merge surface.

Streaming: reveal progressively so the user perceives the agent typing; coalesce a
burst of edits to the same file into one pass. State is per buffer — switching
harness tabs away and back preserves the split.

This is deliberately its own surface, not the Diff card: Diff reviews *worktree
changes*; this shows *the file you're editing changing under you*.

---

## 5 · Cross-harness awareness + the Diff card (Q6)

**When the edited file is open in a Files harness you're *not* looking at:** that
Files **tab pips** (accent dot, `an agent is editing a file open here`), and the
Live Context Diff tab flickers as it does today. Click the Files tab to land on the
live split. This is the harness-native replacement for the old flip-switch nudge.

**Diff card ↔ file surface stay separate.** Different jobs (worktree hunks vs.
files). The only coupling is one-way navigation: clicking a Diff tab (stage 3: a
hunk) opens that file in a Files harness at that line (`onOpenFileAtHunk` in the
prototype). No shared component, no merge UI here.

---

## 6 · Focus / keyboard ownership (the fixed constraint, now free)

Because harness bodies are mutually exclusive, "who owns the keyboard" == "which
harness tab is active." Keep it *visible* anyway:
- The status bar names the holder (`⌨ editor · Files` vs `⌨ Claude Code ·
  terminal`).
- The PTY only receives keys when a terminal harness is the active body — which,
  by construction, is never true while a Files body is showing. The interim
  overlay's leak is gone by design.

---

## 7 · Browse + buffers (Q2)

- **Tree** persistent + nested (supersedes the stage-1 one-level list). Lazy
  expand via `list_dir`; dirs first, mtime-recent first; watcher-refreshed;
  agent-touched pip.
- **Quick-open (#48)** is the fast path: `Mod+P`, fuzzy over the room's files,
  ↑↓/↵/esc, live pip on files an agent is touching. Opens into the active Files
  harness, or the first one, or creates one if none exists.
- **Buffers:** multiple open files per Files harness with a tab row. Dirty dot
  replaces the × until saved. Reopening a file focuses its existing buffer. Cap
  the open set per harness (LRU past ~12 — tune in dogfood).

---

## 8 · Dirty state, save, lifecycle (Q4)

- **Save** = `Mod+S`; clears the buffer's dirty dot. (The write command lands this
  stage — this is its first consumer.) 256 KB read cap is fine for v1; over it,
  show a "large file — view only" notice, don't truncate silently.
- **Close a dirty buffer / Files harness / room** → prompt `Save · Discard ·
  Cancel`.
- **Quit with dirty buffers** anywhere → one prompt listing them; save-all /
  discard-all / cancel. Rooms archive (recoverable); unsaved *buffer* text is only
  in memory — treat it as precious.

---

## 9 · Entry points (Q5)

| Trigger | Result | Stage |
|---|---|---|
| `+ harness → Files` | new Files harness, focused | now |
| `Mod+E` | jump to a Files harness (create if none) ⇄ back to last terminal | now |
| `Mod+P` | fuzzy quick-open → buffer in a Files harness | now (#48) |
| click a Diff tab | open that file in a Files harness | now |
| `Mod+S` / `Mod+F` | save / find-in-file | now |
| Diff **hunk** click | open file at that line | stage 3 |
| Activity row (edit/read) | open file at that line | stage 3 |
| terminal path `file:line` | open file at that line | stage 3 |

All land in the same buffer model, so stage 3 is "add a source + line target."

---

## 10 · Keyboard (fixed constraint: ⌘/Alt + letter/digit only)

`Mod+E` Files (toggle to/from a Files harness) · `Mod+P` quick-open · `Mod+S` save
· `Mod+F` find-in-file. All letters — safe on the Swedish layout, no punctuation /
AltGr chords, no collision with the taken set (K/N/W/J/L, arrows, 1–9). Confirm F
with the owner before shipping.

---

## 11 · Deferred: Column + detach (explicitly not now)

The owner's ultrawide instinct — harness *and* files *and* context all at once — is
real and worth building, but the honest version of it is **detachable panes**: any
harness (terminal, shell, Files) can pop out of the tab stack into its own split.
Column-as-a-fixed-third-region is a shortcut that we'd have to unwind once detach
exists. So: ship Files-as-a-harness now; revisit multi-pane once detach is on the
table. Nothing here blocks that — a Files harness is already a self-contained body
that a future pane can host.

---

## 12 · Editor engine recommendation

**CodeMirror 6.** Covers every primitive the brief lists, ~10× lighter than
Monaco. The one place worth Monaco — a rich merge editor — we explicitly avoided
(§4/§5). Keep the live split a thin custom layer over two CM6 instances so the
incoming pane can stream.

---

## 13 · State variations to verify (map to the prototype)

- **Add Files harness** from `+ harness`; empty state shows `⌘P to jump`.
- **Multiple Files harnesses** coexist, independent buffers.
- **Files harness active:** tree + buffer tabs + editor; touched-file pip.
- **Agent edits your open file, you're on that Files harness:** live split.
- **…you're on a terminal harness:** Files tab pips + Diff tab flickers.
- **Live split, clean:** streams in, then auto-follows disk with Undo.
- **Live split, dirty:** persists with Keep mine / Take theirs / Compare.
- **Live Context Full vs Peek**; Peek default on laptop width.
- Dark + light.

## 14 · Explicitly NOT in v1

Column / multi-pane layout · pane detach · rendered previews · files outside the
room cwd · a merge editor · unifying Diff and the file surface · point-in-time
diffs · search-across-files (find-in-file only).

## 15 · Open questions

- Does `Mod+E` create a Files harness if none exists, or no-op? (Prototype creates.)
- Per-Files-harness buffer LRU cap.
- Streaming cadence for the incoming split vs. snapping tiny edits.
- Whether clean-buffer auto-follow needs the Undo affordance, or the Diff card is
  enough of a net.
- Full-vs-Peek default: purely width-driven, or remembered per room regardless of
  width?
