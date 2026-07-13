# Files pillar — design brief (issue #49, stage 2)

Handoff to the design pass, 2026-07-13. Self-contained on purpose:
you (claude-design) have the original Skein design bundle as context,
but a lot has shipped since — the delta below is what you need before
the questions make sense. The implementation follows whatever this
pass decides, same as the Live Context pass before it.

## Where Skein is today (delta since the original design)

All 8 build chapters shipped; Skein is daily-driven on macOS and
Windows via auto-updating releases (v0.2.5). The shipped anatomy:

- **Room tabs** along the top (a room = task + git worktree + cwd).
- Inside a room: **harness columns** — each harness (Claude Code,
  opencode, shell) is a real PTY/terminal pane. All harnesses of all
  active rooms stay mounted; switching hides with `display:none`, so
  conversations survive tab switches.
- **Right pane = Live Context card stack** (this replaced the old
  Files/Status tabbed pane from the original design): three
  drag-resizable cards — **Diff** (live worktree diff, per-file
  tabs), **Plan** (agent todos), **Activity** (per-tool-call feed).
  The old FileTree/preview pane was deleted when this landed.
- **Overlays**: command palette (Mod+K), settings, reopen-room. A
  transient **Files overlay** (Mod+E) exists as of this week — see
  below. Visual language is unchanged from the design bundle (same
  tokens: bg/fg scale, mono font, accent).

## The problem this pass designs

The owner's daily struggle: no way to view or edit files inside
Skein. As a stopgap, stage 1 shipped a **Files overlay** (Mod+E):
left = one-level directory listing rooted at the room's cwd
(breadcrumb navigation, dirs first, mtime-recent first, live
watcher refresh), right = **raw text view** with find-in-file.

Two direction decisions are already locked from daily-driving it:

1. **Raw first.** No rendered previews — a `.md` file shows markdown
   *source*, images/binaries just get a notice. The target feel is
   VS Code and other text editors: raw view, then raw **edit**
   (save, dirty state, undo/redo, find/replace). Rendered previews
   may return much later as an explicit opt-in; they are out of
   scope here.
2. **In-pane, not an overlay.** The overlay is interim housing. The
   file surface should be part of the room's pane layout. *How* is
   the core open question of this pass.

## Fixed constraints (not up for redesign)

- Raw text only; VS Code-familiar interaction idioms.
- Scope: the room's cwd (worktree). Files outside the room are a
  later stage (explicit OS-picker grants).
- The editor component will be CodeMirror 6 or Monaco — treat
  "line numbers, syntax highlighting, find/replace, undo/redo,
  multi-cursor" as available primitives either way. (CM6 is the
  lean default; Monaco buys literal VS Code feel at ~10× weight.
  A recommendation from the design perspective is welcome.)
- Keyboard: primary modifier is ⌘ (mac) / Alt (win). Only
  letter/digit keys — Swedish layout makes punctuation chords and
  AltGr combos unusable. Mod+E currently opens the overlay; Mod+K
  palette, Mod+N/W/J/L, Mod+arrows, Mod+1-9 are taken. All bindings
  get agreed with the owner before shipping.
- Terminals must keep working while the file surface is visible —
  focus discipline is a real constraint (a focus bug in the interim
  overlay let keystrokes leak into a live agent's PTY; whatever the
  design does, "who has the keyboard" must always be obvious).

## The design questions

1. **Placement.** "Part of the pane" — but which shape? Candidates
   to explore (non-exhaustive, tradeoffs welcome):
   - a) A files **mode/tab of the center area**: the file surface
     appears where terminal panes live, as a peer the user flips to
     (per room). Terminals keep running underneath.
   - b) A **third region**: tree + editor as a column between the
     harness columns and the Live Context stack (or replacing the
     stack temporarily).
   - c) An **expandable Files card** in the Live Context stack that
     can take over the pane when active.
   Consider: a room can have 1-N harness columns already competing
   for width; the Live Context stack is load-bearing for "what is
   the agent doing"; screens are often laptop-sized.
2. **Browse ↔ edit relationship.** Persistent tree beside the
   editor (VS Code sidebar) vs summon-on-demand (quick-open first,
   tree secondary)? One open file at a time, or multiple open
   buffers with their own tab row? (A fuzzy quick-open — issue #48 —
   is planned as the keyboard entry point and should fit whatever
   you design.)
3. **The Skein-specific scenario: the agent edits the file you have
   open.** This is the product's normal case, not an edge case. A
   200 ms filesystem watcher already exists per room. Design the
   semantics + surface: clean buffer silently follows disk? dirty
   buffer gets a "changed on disk" banner with reload/keep/compare?
   How does this relate to the Diff card, which already shows the
   worktree diff — should "compare" jump there, or inline?
4. **Dirty state + save.** Save affordance, unsaved indicator,
   what happens on room close / Skein quit with dirty buffers
   (rooms archive rather than delete; PTYs die with the process).
5. **Entry points.** Today: Mod+E + palette. Planned (stage 3):
   click a file in the Diff card → open at that hunk; jump from an
   Activity row to the file at line; open-at-line from terminal
   output. The design should show where those land so stage 3 has
   a home.
6. **Distinction from the Diff card.** The Diff card shows *changes*
   (hunks); the file surface shows *files*. Keep them clearly
   different or begin unifying them? (A larger diff/review rethink
   exists separately — don't design it here, just don't paint over
   its door.)

## Substrate you can assume (already built)

- Scoped fs commands: one-level `list_dir`, `read_file_text`
  (256 KB cap today — flag if the design needs more), both
  restricted to room cwds with symlink-escape protection. A write
  command does not exist yet; stage 2 adds it.
- Watcher (200 ms, recursive, per room) feeding the Diff card;
  reusable for external-change detection.
- The stage-1 tree (breadcrumbs, mtime-first, watcher-refreshed)
  and find-in-file — reusable pieces, not sacred ones.

## Deliverable

Same shape as previous passes: an interactive prototype + a written
handover that resolves the questions above into buildable decisions
(placement, interaction model, external-change semantics, open-buffer
model, entry points), landing in `docs/design/` for implementation
to follow.
