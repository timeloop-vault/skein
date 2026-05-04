# Chapter 7 — Terminal embedding rework (cross-platform)

Chapter 2 phase 4 wired Enter-for-shell and R-to-retry into the
post-exit prompt. Enter works reliably — the new child is a plain
shell, no alt-screen contention with whatever just exited. **R is
fragile when both the previous and the next child are TUIs**
(Claude → R → opencode in the same harness): the first TUI's alt
screen, cursor position, and SGR state linger; the second TUI
inherits a corrupted viewport. This has been the longest-standing
bug in the prototype and was deliberately parked for "its own
chapter" since chapter 2's close.

This chapter is **cross-platform from line one**. Mac, Windows, and
Linux are all first-class targets; Windows ConPTY in particular has
known quirks ([microsoft/vscode#71966](https://github.com/microsoft/vscode/issues/71966)
and friends) that VS Code worked around and Skein currently doesn't.

## Recon revised the scope

Phase 1 (`docs/chapter-7-recon.md`) read portable-pty's per-OS
contracts and VS Code's `terminalProcess.ts`, then re-read Skein's
`pty.rs` and `LiveTerminal.respawn`. Headline finding: **the
substrate is mostly already correct.** Chapter 2 phase 4 wired
the dual-thread reader+waiter pattern needed for Windows ConPTY,
and `respawn` already issues three of the four DEC reset sequences.
The remaining work is filling specific holes, not rewriting.

The original plan oversold this as a substrate rewrite. Below is the
revised scope. Numbers in `[brackets]` reference recon §5's gap
table.

## Phase 1 — Reconnaissance

**Done.** See [`chapter-7-recon.md`](./chapter-7-recon.md).
Phases 2-4 cite it.

## Phase 2 — Windows ConPTY reliability

**Goal:** close the two known-bad behaviours that VS Code works
around but Skein currently doesn't. Both are Windows-specific in
trigger, but applying them uniformly is simpler than per-OS
branching where the cost on Unix is negligible.

- **[1] Data-flush timeout in `pty.rs`'s waiter thread.** VS Code
  waits 250 ms after the child exits for trailing data to drain
  before firing its `_onProcessExit`. Skein currently emits
  `PtyEvent::Exit` the moment `child.wait()` returns, racing the
  reader thread on Windows ConPTY where the read pipe stays open
  until the master is dropped. Insert a `thread::sleep(250 ms)`
  between `child.wait()` and the `Exit` event — the reader keeps
  draining during the window, so any final TUI frame makes it to
  xterm before the prompt overwrites it.

- **[2] Kill/spawn throttle on Windows in `LiveTerminal.respawn`.**
  Mashing R immediately after a child dies triggers VS Code's
  workaround range (`microsoft/vscode#71966`, `#117956`,
  `#121336`) — ConPTY can hang the host between rapid kill and
  spawn calls. Add a 250 ms `setTimeout` between the `pty_kill`
  of the old PTY and the `pty_spawn` of the new, gated on
  `isWindows`. Same source pattern (`navigator.platform`) as
  chapter 4's `isMac` helper; add an `isWindows` export to
  `shortcuts.ts`.

Both changes are bounded — well under 50 lines combined. No new
Tauri commands, no schema changes, no frontend state.

## Phase 3 — Complete the xterm reset between PTY swaps

**Goal:** the next child writes onto a fully clean terminal.

`LiveTerminal.respawn` already issues `\x1b[?1049l` (exit alt
screen), `\x1b[2J` (clear viewport), and `\x1b[H` (cursor home).
Two pieces are missing.

- **[3] SGR reset (`\x1b[0m`).** Without this, the previous child's
  active colour / bold / underline / inverse state bleeds into the
  next child's first writes until *it* sets its own SGR. Append to
  the existing reset string. One byte per attribute slot, two-byte
  total addition.

- **[4] Explicit `pty_resize` after the new spawn.** Some TUIs only
  redraw on a window-size signal (the alt-screen entry sequence
  alone isn't enough). After `startPty(cmdToSpawn)` returns, fire
  a `pty_resize` to the current xterm dimensions. On Unix this
  becomes a SIGWINCH; on Windows it becomes
  `ResizePseudoConsole`. Both nudge the new child to issue a
  fresh full redraw.

Frontend-only, OS-agnostic — DEC sequences and Tauri invokes work
the same everywhere.

## Phase 4 — Remove R-retry, validate Enter-for-shell

The chapter started by trying to make R-to-retry work on every OS.
After landing phases 2 + 3, manual testing on Mac surfaced that R
on a Claude harness can't re-use the chapter-5-phase-2a
`--session-id <uuid>` form (Claude rejects with "Session ID is
already in use"); a workaround that translates to `--resume <uuid>`
on R got the spawn through but produced a blank viewport because
Claude's resume protocol doesn't replay reliably into a recycled
xterm. Each fix surfaced the next edge case.

**Decision:** drop the R-retry path entirely. It pre-dates chapter
5's resume work, and the same intent ("come back to my Claude
conversation in this pane") is now covered by Skein's
restart-resume flow plus `claude --resume <uuid>` from the
Enter-for-shell shell. The simplification is significant — no R
handler, no `respawn` retry-form transform, no test matrix for
TUI→TUI handover. Enter-for-shell stays and benefits from phases
2 and 3 (shell spawning into a Claude alt-screen would still need
the reset, even though shells aren't TUIs themselves).

What this phase actually does:
- Remove the R key handling from `LiveTerminal`'s post-exit
  prompt.
- Update the prompt text from "Press Enter for shell, R to retry"
  to "Press Enter for shell."
- Update file-level comments to drop the R reference.
- Update `docs/backlog.md` — remove the chapter-2 PTY-rework
  entry now that the alt-screen handover bug class is sidestepped
  rather than fixed.

Test matrix on macOS:
- Claude → `/exit` → Enter → shell prompt visible, no Claude
  artifacts in the viewport, can run `claude --resume <uuid>`
  manually to come back.
- opencode → `/exit` → Enter → shell prompt visible, can run
  `opencode --session <id>` manually.
- Phase 2's data-flush timeout: the final pre-exit frame
  (Claude's "Goodbye!" line, opencode's tear-down message) is
  visible above the "[skein] x exited" line, not truncated.
- Phase 3's reset is visible: shell prompt starts at the top of
  the viewport with default colour, not where Claude's cursor
  was, not coloured.

Backlog cleanup:
- Remove the chapter-2 PTY-rework entry. The alt-screen handover
  bug class doesn't need fixing because we no longer trigger it.

## Phase 5 — Cross-platform validation hand-off

**Goal:** be honest about what a Mac dev box can verify and what
needs to wait for chapter 8's actual Windows / Linux runs.

- Document the manual test plan from phase 4 in a form a Windows /
  Linux user can follow — short checklist, expected output,
  failure modes to flag.
- Capture the **specific** Windows behaviours phase 2 wrote code for
  so a Windows runtime test can confirm them empirically:
  - Trailing-frame TUI output makes it to xterm after natural
    child exit (test by quitting Claude with `/exit` and verifying
    the final goodbye line is visible above the [skein] line).
  - Mashing Enter-for-shell repeatedly doesn't hang the harness
    pane (the kill/spawn throttle does its job; same ConPTY bug
    that affected R applies to any rapid kill+spawn cycle).
  - `ResizePseudoConsole` propagates to the new shell after
    Enter-for-shell.
- Linux is mostly Unix-shaped, so the Mac validation likely
  applies — but the recon doesn't cover GTK / Wayland WebView
  rendering of xterm.js. Call out as "test on Linux when chapter 8
  builds an actual artifact."
- This phase is **mostly notes** — minimal code. The actual cross-
  OS verification overlaps with chapter 8 (distribution) where
  Windows / Linux builds become real artifacts.

## Out of scope for chapter 7

See [`backlog.md`](./backlog.md) — anything we considered but pushed
out has been merged there. Notably:

- **Switching off portable-pty.** It's wezterm's own crate and gives
  us the OS abstractions we need. node-pty (VS Code's choice) would
  mean adopting their workaround stack wholesale, including the
  bugs they've worked around. Stay on portable-pty.
- **Flow control (PTY pause/resume).** VS Code does this when
  unacknowledged char count exceeds a threshold. Prototype scale;
  revisit if a TUI ever overwhelms the channel.
- **`conptyInheritCursor` for initialText launches.** We don't seed
  initial text at spawn. Revisit if we ever do.
- **Multi-buffer / scrollback persistence across PTY swaps.** Today
  Skein clears the viewport between swaps; preserving the previous
  child's scrollback as history above the new child's prompt is a
  separate UX call, not a substrate question.
- **Windows-specific signal forwarding** beyond what ConPTY
  abstracts. SIGINT / SIGTERM / etc. mapping to Windows' control
  events is its own rabbit hole.
- **Chapter 8 — distribution.** Once chapter 7 lands, the runtime
  is solid on every OS we ship; chapter 8 is the packaging,
  signing, and notarization pass.
