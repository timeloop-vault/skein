# Chapter 7 — Terminal embedding rework (cross-platform)

Chapter 2 phase 4 wired Enter-for-shell and R-to-retry into the
post-exit prompt. Enter works reliably — the new child is a plain
shell, no alt-screen contention with whatever just exited. **R is
fragile when both the previous and the next child are TUIs**
(Claude → R → opencode in the same harness). The first TUI's alt
screen, cursor position, and SGR state linger; the second TUI
inherits a corrupted viewport. This has been the longest-standing
bug in the prototype and was deliberately parked for "its own
chapter" since chapter 2's close.

Two flavours of root cause:

1. **PTY-side child-exit detection is OS-dependent.** Today
   `PtyManager` treats reader EOF as "child gone." On macOS / Linux
   that mostly correlates with the child actually exiting. On
   Windows ConPTY the reader pipe stays open after the child exits
   — Skein never notices, never offers Enter / R, and the harness
   pane appears frozen.
2. **xterm-side state isn't reset between PTY swaps.** When R
   respawns, we just point xterm at a new PTY's byte stream. xterm
   is still in alt-screen mode (DECSET 1049 from the previous
   child), still has cursor position from the previous child's last
   write, still has whatever SGR attributes were active. The new
   child writes its own setup but starts from a polluted base.

This chapter is **cross-platform from line one**. Mac, Windows, and
Linux are all first-class targets; phase 1 documents per-OS
contracts, phase 2 implements them, and phase 5 covers what
verification a Mac dev box can't do alone (with chapter 8 picking up
the rest as part of distribution).

We are not reinventing terminal embedding. VS Code's
`vscode/src/vs/platform/terminal/node/terminalProcess.ts` and
wezterm's `pty` crate have already worked through the corner cases
on each OS. **Phase 1's deliverable is reading their patterns and
documenting what we lift.** Skipping the recon and inventing risks
re-finding bugs they already fixed.

## Phase 1 — Reconnaissance

**Goal:** every assumption phases 2 / 3 / 4 make about how PTYs
behave on macOS, Linux, and Windows is verified against
already-working implementations and written down. Code-free spike;
chapter 5 phase 1's pattern.

- Read `vscode/src/vs/platform/terminal/node/terminalProcess.ts`
  end-to-end. Note the platform branches (`process.platform`),
  the child-spawn / signal / disposal lifecycle, and how alt-screen
  handover is staged.
- Read wezterm's `pty` crate (`unix.rs`, `win.rs`,
  `cmdbuilder.rs`). Same — note the platform-specific `Child`
  reaping, reader lifecycle, and ConPTY's `ResizePseudoConsole`
  / `ClosePseudoConsole` ordering.
- For each of macOS / Linux / Windows, document:
  - **Child-exit signal**: how does the host know the child
    process is gone? (SIGCHLD reap on Unix; `GetExitCodeProcess`
    polling on Windows.)
  - **Reader-pipe lifecycle**: does the reader EOF on child exit?
    (Yes on Unix; no on Windows ConPTY — pipe stays open.)
  - **Resize semantics**: how does a resize trigger a redraw?
    (`TIOCSWINSZ` ioctl + SIGWINCH on Unix;
    `ResizePseudoConsole` on Windows.)
  - **Alt-screen sequence**: which DEC mode codes the next child
    will expect, and what state we should reset *before* feeding it
    bytes.
- Capture the patterns we'll lift wholesale. Skein's PTY layer is
  thin and we don't need bespoke infra — we mostly need to wire up
  the right calls in the right order.
- Output: `docs/chapter-7-recon.md` — short, factual, phase-by-phase
  cross-references. Phases 2-4 cite it.

**No code in phase 1.** Same rationale as chapter 5: cheap, kills
unknowns, the alternative is debugging guesses through phase 2 on
three OSes.

## Phase 2 — Cross-platform child-exit detection

**Goal:** a single contract — `PtyEvent::Exit(code)` — that fires
on every OS regardless of which signal got us there. The frontend
stops having to care about platform.

- `PtyManager` (Rust) gains an OS-aware exit detector:
  - **macOS / Linux**: keep today's reader-EOF path as the primary
    signal. Pair it with a `child.try_wait()` poll on a short
    timer to avoid the rare cases where reader stays open after
    child exits (e.g. orphaned grandchildren).
  - **Windows (ConPTY)**: reader-EOF can't be relied on. Spawn a
    background task that polls `child.try_wait()` every ~100 ms;
    on exit, drain the reader for any final output, then emit the
    `Exit` event and close the pipe explicitly via
    `ClosePseudoConsole`.
- Single Rust event sent over the existing
  `Channel<PtyEvent>` so the frontend's existing `handleExit`
  path continues to work — the contract changes, the wire format
  doesn't.
- Tests where feasible: spawn `echo hi`, observe `Exit` fires
  within ~250 ms on each OS. portable-pty's API may not be
  test-friendly for ConPTY; prefer integration smoke tests in
  the `pty` module.
- Out of scope: arbitrary signal forwarding (SIGTERM / SIGKILL
  semantics across OSes). Today's `pty_kill` stays as is.

## Phase 3 — xterm state reset between PTY swaps

**Goal:** the next child writes onto a clean terminal, regardless of
what the previous child left behind.

This phase is purely frontend (xterm.js + Tauri invoke). The same
DEC sequences work on all OSes; xterm.js is OS-agnostic.

- Before mounting the new PTY's `onData`, write a deterministic
  reset sequence to the existing `Terminal` instance:
  - `\x1b[?1049l` — exit alt screen (DECRST 1049). If the prior
    child already exited cleanly out of alt screen this is a
    no-op; if not, it forces us back to the main buffer.
  - `\x1b[H` — cursor home.
  - `\x1b[2J` — erase entire viewport.
  - `\x1b[0m` — reset SGR (colour, bold, underline, etc.).
- After the reset, immediately `pty_resize` to xterm's current
  dimensions. On Unix this fires SIGWINCH; on Windows this fires
  `ResizePseudoConsole`. Either way the new child sees a "fresh
  size" signal and (per terminal convention) issues its own
  redraw.
- The reset block is a tiny helper function in `LiveTerminal.tsx`
  that respawn / R-retry / Enter-for-shell all funnel through.

## Phase 4 — R-retry of TUIs, end-to-end

**Goal:** the original test case works on every OS we can build for.

- Wire phase 2 (clean exit detection) and phase 3 (clean xterm
  reset) into chapter 2 phase 4's existing R / Enter handlers.
  Same UX surface; the substrate is now solid.
- Test matrix on macOS (primary dev box):
  - Claude → R → Claude (same TUI re-spawn).
  - Claude → R → opencode (TUI → TUI handover — the headline bug).
  - Claude → R → shell (TUI → non-TUI).
  - shell → R → Claude (non-TUI → TUI).
- Verify each: viewport clean, cursor positioned where the new
  child wants it, no leftover SGR colour bleed, no "ghost" prompt
  from the previous child.
- Update `docs/backlog.md` to remove the chapter-2 entry now that
  the fix has shipped.

## Phase 5 — Cross-platform validation hand-off

**Goal:** be honest about what a Mac dev box can verify and what
needs to wait for chapter 8's actual Windows / Linux runs.

- Document the manual test plan from phase 4 in a form a Windows /
  Linux user can follow — short checklist, expected output,
  failure modes to flag.
- Capture the *specific* ConPTY quirks phase 2 wrote code for so a
  Windows runtime test can confirm them empirically:
  - Child exits, `Exit` event fires within budget.
  - Reader doesn't hang Skein after child exit.
  - `ResizePseudoConsole` propagates to the child.
- Linux is mostly Unix-shaped, so the same Mac validation likely
  applies — but call out anything we'd want to double-check on
  GTK/X11 / Wayland (nothing PTY-related, more "does the WebView
  render xterm.js identically").
- This phase is **mostly notes** — minimal code. The actual
  cross-OS verification overlaps with chapter 8 (distribution)
  where Windows / Linux builds become real artifacts.

## Out of scope for chapter 7

See [`backlog.md`](./backlog.md) — anything we considered but pushed
out has been merged there. Notably:

- **Multi-buffer / scrollback persistence across PTY swaps.** Today
  Skein clears the viewport between swaps (phase 3 makes this
  deterministic); preserving the *previous* child's scrollback as
  history above the new child's prompt is a separate UX call.
- **Mouse / paste protocol passthrough between PTY swaps.** Today
  xterm handles its own mouse / clipboard bindings. Children in
  alt-screen mode that grab mouse events keep working; this
  chapter doesn't change that contract.
- **TUI-aware line buffering** (e.g. detecting when the child
  expects bracketed paste vs. raw mode). Out of scope; xterm and
  the child negotiate this directly.
- **Windows-specific signal forwarding** beyond what ConPTY
  abstracts. SIGINT / SIGTERM / etc. mapping to Windows' control
  events is its own rabbit hole.
- **Chapter 8 — distribution.** Once chapter 7 lands, the runtime
  is solid on every OS we ship; chapter 8 is the packaging,
  signing, and notarization pass.
