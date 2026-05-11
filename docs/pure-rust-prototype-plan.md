# Pure-Rust prototype plan

Research issue: #36

## Goal

Stand up a tiny pure-Rust app that runs `claude` in a native-rendered
terminal, with a git-status side pane, reusing the existing pure-Rust
foundations (`skein-git`, the PTY logic in `pty.rs`).

The output is *evidence*: does this substrate work for Skein, what
breaks, what surprises us. The prototype is throwaway — what we keep
is the answer to the question.

## What this prototype is *not*

- Not a rewrite. Daily-driver Skein keeps shipping unchanged on
  `main`. The spike lives entirely on this branch.
- Not a feature replica. Sessions, harnesses, splits, file tree,
  preview registry, command palette, modals, theming, IME, OSC 8
  hyperlinks, terminal search, persistence — all out of scope. They
  become the *rewrite* plan only if the spike succeeds.
- Not cross-platform-complete *during* the spike. macOS is where we
  develop; Windows and Linux/Wayland are smoke-tested at the end of
  phase 4. The *long-term* requirement is first-class on all three
  (macOS, Windows, Linux/Wayland) — that requirement drove the
  framework pick, but we don't gate the spike on it.

## Where it lives

`crates/skein-proto/` — new crate inside the existing workspace.
Depends on `skein-git` directly. `app/` and `app/src-tauri/` stay
untouched; nothing about the daily driver changes during the spike.

## Stack

- **GUI:** **Floem**. Locked in. Driven by the cross-platform
  requirement: Lapce ships on macOS, Windows, and Linux including
  Wayland — that proof point exists today, on the exact platforms we
  care about. Reactive (signals), Vello/wgpu rendering, cosmic-text
  for glyphs. Smaller maintainer team than Iced but contributions
  land fast and our needs (IDE-shape, Wayland polish) align with
  Lapce's, so any upstream work we push helps both projects.
- **Named fallback:** **Iced** — *not GPUI*. If Floem fights us
  materially in phase 3 (terminal widget) or phase 4 (claude
  rendering), pivot to Iced rather than push through. GPUI is
  excluded because Mac is its only first-class platform; the
  Windows + Wayland gap is exactly the constraint we picked Floem to
  satisfy. We do not bake-off — we commit, and switch only on hard
  evidence.
- **Terminal parser/buffer:** `alacritty_terminal`. Reference
  `lapce-terminal` for rendering and input glue.
- **PTY:** `portable-pty`, lifted from
  `app/src-tauri/src/pty.rs`. Replace `tauri::ipc::Channel<String>`
  with `tokio::sync::mpsc` (or a small trait we can swap later).
- **Git:** `skein-git` as a direct workspace dependency. No changes.

## Phases

### Phase 0 — Hello window

Workspace member added, Floem "Hello, Skein" window opens on macOS.
`cargo run -p skein-proto` works.

### Phase 1 — Git status pane

Single panel listing files from `skein-git::Repo::status()` for a
hardcoded repo path. Manual refresh button.

Validates: Floem's reactive layer composes with our existing crate.

### Phase 2 — PTY plumbing

Lift spawn / read / write from `app/src-tauri/src/pty.rs`, swap the
Tauri channel for `tokio::sync::mpsc`. Spawn a shell, log output to
stderr / a debug pane.

Validates: PTY bytes flow in pure-Rust land outside Tauri.

### Phase 3 — Terminal widget

`alacritty_terminal::Term` driven by PTY bytes. Render
`renderable_cells()` as glyphs in Floem. Forward keyboard input —
printable, arrows, enter, backspace, ctrl-modifier basics. Mouse
scrollback if cheap; skippable if not.

This is the load-bearing phase. Either we have a working terminal
or we don't.

### Phase 4 — Run `claude` in it (and cross-platform smoke test)

Spawn a shell, `cd` into a real project, run `claude`. See what
happens. Iterate on whatever is obviously broken (resize, scroll,
modifier keys, alt-screen, mouse reporting). Test specifically
against the friction that motivates the spike: #2 (onData replies),
#23 (claude TUI rendering), #27 (Shift+Enter).

After macOS works: smoke-test on Windows and Linux/Wayland — does
it build, open a window, run `claude`? Not "polished on all three";
just "the substrate isn't broken on any of them." Anything beyond
that goes in the rewrite plan, not the spike.

Validates: the spike's actual question. Does the substrate solve the
xterm.js territory, or surface different-but-equally-bad problems?
And: do the other two platforms look tractable, or is there a
showstopper hiding?

### Phase 5 — Reflect

Append findings to this doc. Decision: write a real rewrite plan, or
close as "no-go, here's why" and feed the lessons back into
Tauri-side fixes.

## Decision points

After each phase: still tractable? A short stuck-doc and stop is
*also* a successful spike — the verdict is the value, not the
deliverable.

If phase 3 takes a wildly disproportionate amount of code or fights
us on basic input/rendering, that's a signal to pivot to **Iced**
before declaring no-go. We do that pivot once, not iteratively.

## What we keep regardless of the verdict

**If the spike succeeds and we go:**
- The lifted PTY logic in `skein-proto/` becomes the seed for a
  proper `crates/skein-pty`. The same extraction pattern (already
  applied to `skein-git`) extends to watcher, db, resume — Tauri
  code becomes a thin shell over those crates whether or not we ever
  leave Tauri.

**If the spike fails:**
- We know precisely *what* fails, which sharpens whatever we do
  next inside Tauri — e.g. swapping xterm.js for a different
  WebView-side terminal renderer, or driving native terminal
  rendering via a `<canvas>` over IPC.
- The throwaway crate gets deleted; the daily driver is untouched.

## Out of scope (parking lot)

- Native menus, tray, notifications, opener
- Window state restoration, multiple windows
- Settings UI, theming
- File preview pipeline
- Anything that the daily driver does beyond "PTY + git status"

These belong in the rewrite plan, not the spike.

---

## Phase 5 — Reflection (2026-05-11)

### Verdict: **GO**

The substrate works. `claude` renders cleanly in the prototype on
macOS — alt-screen mode, colors, bold/dim, box-drawing, selection
highlighting, the works. The friction that motivated this research
(#2 onData drops, #23 TUI rendering glitches, #27 Shift+Enter) is
fixed by going native; none of those issues exist when the parser
is `alacritty_terminal` rather than xterm.js in a WebView.

### What actually shipped in the spike

`crates/skein-proto/` — ~815 LoC of Rust on top of `skein-git`:

| File | Lines | Role |
| --- | --- | --- |
| `src/main.rs` | ~155 | App shell, two-pane layout, PTY ↔ view wiring |
| `src/term.rs` | ~50 | `alacritty_terminal::Term` + `vte::ansi` parser wrapper |
| `src/term_view.rs` | ~370 | Custom Floem `View` — paint, events, keys, scroll, selection |
| `src/term_colors.rs` | ~100 | ANSI/indexed/truecolor → floem `Color` |
| `src/pty.rs` | ~170 | Lifted from `app/src-tauri/src/pty.rs`, Tauri-free |

Features actually working end-to-end on macOS:
- Spawn and run `bash -li` (and `claude` inside it)
- Full color (16 ANSI + xterm 256 + truecolor)
- Bold / dim / inverse cell flags
- Cursor rendering (filled when focused, hollow when not)
- Mouse-wheel scrollback (smooth, follows system natural-scroll)
- Click-drag selection + selection background painting
- Right-click → copy selection to clipboard
- Cmd+C → copy if selection present
- Cmd+V → paste clipboard contents into PTY
- Keyboard table covers printable, ctrl-letter, arrows, enter,
  tab, escape, backspace, delete, shift-enter (#27), alt-letter
  as ESC-prefix, space, home/end/pgup/pgdn

### What surprised us

- **Floem composes cleanly with existing crates.** `skein-git`
  required zero changes. The custom `View` lives next to ordinary
  Floem widgets in the same tree, styled via the same decorators.
- **`alacritty_terminal` does the heavy lifting for free.**
  Alt-screen mode, scrollback grid, complex escape sequences, OSC
  responses, mouse reporting state — all of it is the parser's
  problem, not ours. We render a flat grid + run a key table.
- **Iteration speed is fine.** First build ~25s; incremental
  on the proto layer 0.5–2s. The "Rust GUI iteration is slow"
  fear didn't materialize at this scale. (May still bite as the
  surface grows.)
- **The `crossbeam_channel` → `create_signal_from_channel` →
  `create_effect` bridge is the correct primitive** for getting
  PTY bytes from a worker thread into Floem's reactive layer.
  Felt natural, not fought-with.

### What we had to work around

- **Floem 0.2 doesn't re-export `Renderer`.** Needed
  `floem_renderer = "0.2"` as a direct dep. (Newer Floem on git
  fixes this.)
- **`Event::FocusGained` / `FocusLost` don't reach
  `View::event_before_children`** in Floem 0.2 — they dispatch
  only through `.on_event(EventListener::FocusGained, ...)`
  decorators. Worked around with an `Arc<AtomicBool>` the View
  reads at paint and the decorators update on focus changes.
- **`PaintCx` in Floem 0.2 doesn't expose `app_state()`**, so
  we can't query `is_focused(id)` at paint time. Same workaround.
- **`vte::ansi::Processor::advance` takes a slice in 0.26**;
  Lapce's reference code is against an older API where it took a
  single byte.

These are all *Floem-version churn* issues, not substrate
problems. None changed our judgement on Floem; all would be
fixed by tracking a newer Floem revision (the same one Lapce
uses).

### What we *did not* test

- **Windows / Linux / Wayland.** macOS only. Stack proof point
  exists (Lapce ships on all three), but actual smoke testing is
  deferred to the rewrite plan.
- **Performance under load.** Single-character paint per glyph,
  no batching. `claude` is interactive — long-output streaming
  (`cargo build`, `find /`) wasn't measured. Lapce's `paint_line_content`
  batches contiguous-bg runs; we'd port that pattern if rendering
  perf becomes a bottleneck.
- **IME (CJK input).** No testing. Floem has IME events but the
  spike doesn't handle them.
- **OSC 8 hyperlinks**, **mouse reporting** (for vim et al.),
  **terminal search**, **resize stress test**.
- **Cross-process clipboard / drag-and-drop / native menus / tray
  / window state.** All parking-lot items; the rewrite plan
  decides their order.

### Keyboard table is incomplete

F-keys, Insert, keypad, modifier-matrix on Home/End/PgUp/PgDn —
all missing. Each is a `Key::Named(...) => Some(b"...")` line.
Tedious but not architectural. Will populate incrementally as
TUIs surface gaps.

### Decision: write a rewrite plan, parallel-track

The right shape — daily-driver Skein on Tauri keeps shipping;
the pure-Rust version grows in parallel under `crates/skein-proto/`
(eventually renamed). Switch the user-facing default when parity
on the features they actually use is reached.

**Next concrete steps (the rewrite plan):**

1. **Extract pure-Rust crates from `app/src-tauri/src/`**, in this
   order: `skein-pty`, `skein-watcher`, `skein-db`, `skein-resume`,
   `skein-fs`. Each replaces its in-tree counterpart in the Tauri
   build *and* serves as the foundation for the rewrite. **This
   work is valuable regardless of the rewrite outcome.**
2. **Track Floem from git, not crates.io** — pin to the same rev
   Lapce uses. Buys us the API improvements (Renderer re-export,
   PaintCx::app_state, etc.) and keeps us in step with the
   biggest user of the framework.
3. **Build out the rewrite incrementally**: terminal pane →
   sessions/harnesses → command palette → file tree → preview.
   Each step has a clear "feature parity with the Tauri version"
   gate.
4. **Smoke-test on Windows + Linux/Wayland early.** Don't wait
   until parity to find platform-specific surprises.
5. **Keep the keyboard-input table honest.** Port Lapce's full
   table when it's needed; don't reinvent it.

### What we keep from the spike

- All five files in `crates/skein-proto/` — these become the
  starting point for the rewrite's terminal pane and color
  module.
- The lifted `pty.rs` becomes the seed for `crates/skein-pty`.
- The findings in this section, on the record.

### What gets thrown away

- The standalone `skein-proto` binary stops mattering once the
  rewrite has its own binary. The crate transitions from
  "throwaway spike" to "first crate of the new app."

