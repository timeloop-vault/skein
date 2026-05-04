# Chapter 7 — Terminal embedding rework (cross-platform)

Chapter 2 phase 4 wired Enter-for-shell and R-to-retry into the
post-exit prompt. Enter mostly worked; **R was fragile when both the
previous and the next child were TUIs** (Claude → R → opencode in
the same harness). The first TUI's alt screen, cursor position, and
SGR state lingered; the second TUI inherited a corrupted viewport.
The bug had been parked since chapter 2's close as "its own chapter."

This chapter is **cross-platform from line one** — Mac, Windows, and
Linux are all first-class targets. Windows ConPTY in particular has
known quirks ([microsoft/vscode#71966](https://github.com/microsoft/vscode/issues/71966)
and friends) that we needed to get right rather than later.

## What actually shipped

The chapter changed shape twice. Phase 1's recon found the substrate
was mostly correct (chapter 2 phase 4 already had the dual-thread
reader+waiter pattern needed for Windows ConPTY, and `respawn`
already issued most of the DEC reset sequences). Phases 2-3 added
the small remaining fixes. Then phase 4's manual testing on Mac
surfaced a deeper problem: even Enter-for-shell into a fresh shell
left a blank viewport with a blinking cursor. The alt-screen
handover bug class wasn't TUI-specific — it was about reusing one
xterm across PTY swaps at all.

The fix that actually worked was the React-idiomatic one: derive
LiveTerminal's `mountKey` from cmd content. When the cmd changes
(Enter-for-shell), React unmounts the old LiveTerminal (cleanup
disposes the xterm and pty_kills the child) and mounts a fresh one
(new xterm, new PTY). No state to reset, no scrollback to preserve.
That refactor superseded most of phases 2-3, leaving only the
pty.rs-side data-flush timeout standing.

## Phase 1 — Reconnaissance

Done. See [`chapter-7-recon.md`](./chapter-7-recon.md). Confirmed
per-OS contracts (Unix reader EOF correlates with child exit;
Windows ConPTY reader pipe stays open until master Drop), audited
VS Code's `terminalProcess.ts` and Skein's existing `pty.rs`,
identified four specific gaps. Two of those gaps shipped (#1 below);
the other two were rendered obsolete by phase 3's refactor.

## Phase 2 — Data-flush timeout in `pty.rs`

VS Code waits 250 ms after the child exits for trailing data to
drain before firing its `_onProcessExit`. Skein was emitting
`PtyEvent::Exit` the moment `child.wait()` returned, racing the
reader thread on Windows ConPTY where the read pipe stays open
until the master is dropped. A `thread::sleep(250 ms)` between
`child.wait()` and the `Exit` event lets the reader keep draining
during the window, so Claude's "Goodbye!" / opencode's tear-down
line make it to xterm before the `[skein] x exited` line overwrites
the cursor row.

Mirrors VS Code's `ShutdownConstants.DataFlushTimeout` (see
[microsoft/node-pty#72](https://github.com/microsoft/node-pty/issues/72)).
Latency cost is on the natural-exit path only — running output is
unaffected.

This is the only chapter-7 substrate change that survived all
revisions.

## Phase 3 — Remount LiveTerminal on cmd change

The original phase 3 added two more reset sequences (SGR reset and
post-spawn `pty_resize`) to `LiveTerminal.respawn`'s alt-screen
handover. Phase 4 testing showed the handover was beyond saving —
even non-TUI shells coming after a TUI ended up with a blank
viewport. The fix wasn't more reset code; it was removing the
handover entirely.

The remount approach:

- `HarnessBody` (App.tsx) derives `LiveTerminal`'s mountKey from
  cmd content: `${harness.id}:${harness.cmd.join("\x00")}`. Joining
  the array gives a value-equal string across renders so a re-render
  with content-identical cmd doesn't churn.
- When `onCmdChange(shell)` fires (Enter-for-shell), App's room
  state updates, harness.cmd changes, mountKey changes, React
  unmounts the old LiveTerminal and mounts a fresh one.
- The unmount cleanup already pty_killed the old child and
  disposed the xterm. The fresh mount opens a new xterm, spawns
  the new PTY, and renders cleanly.

This deletes:
- `LiveTerminal.respawn` (~50 lines).
- The DEC reset sequences (`\x1b[?1049l\x1b[2J\x1b[H\x1b[0m`).
- The Windows kill/spawn throttle (had been added in the original
  phase 2 part 2).
- The post-spawn `pty_resize` nudge.
- `isWindows` from shortcuts.ts (no remaining caller).

Cost: scrollback from before Enter-for-shell is gone. The user
explicitly accepted this trade when they suggested moving away
from in-pane retry — they want a usable shell more than they want
the previous TUI's history.

## Phase 4 — Drop R-retry, validate Enter-for-shell

R-retry pre-dates chapter 5's resume work. After chapter 5 ships
session-id pre-allocation, R can't re-use the spawn cmd
(`["claude", "--session-id", uuid]`) because Claude refuses a
UUID whose JSONL file already exists. A workaround that translated
to `--resume <uuid>` got the spawn through but Claude's resume
protocol didn't replay reliably into a recycled xterm.

Removed:
- The R key handler in `LiveTerminal`'s post-exit prompt.
- The "R to retry" prompt text — now just "Press Enter for shell."
- All R-related comments and the chapter-2-phase-4 framing.

The same intent ("come back to my conversation in this pane") is
now covered by Skein's restart-resume flow (chapter 5) plus
`claude --resume <uuid>` / `opencode --session <id>` from the
Enter-for-shell shell.

Validation on Mac:
- Claude → `/exit` → Enter → clean shell prompt. ✓
- Multi-harness in one room (one Claude per tab, distinct UUIDs)
  no longer collides — fixed by the same `useEffect` deps cleanup
  that removed `cmd` from the dep array.

Backlog cleanup: removed the chapter-2 PTY-rework entry. The
alt-screen handover bug class is sidestepped (we don't reuse the
xterm) rather than fixed.

## Phase 5 — Cross-platform validation hand-off

The Mac dev box can verify the Mac path. Windows and Linux runtime
verification is real work that needs an actual machine — folded
into chapter 8 (distribution) where building Windows / Linux
artifacts is already on the agenda.

Test plan a Windows / Linux user can follow:

1. **Multi-harness in one room.** Open a room, spawn Claude in the
   first harness, type something. Add a second harness in the same
   room (Claude or opencode). Both should keep running side by side.
   *Expected fail mode:* the first harness exits with "Session ID
   is already in use" → useEffect deps regression.

2. **Enter-for-shell after natural exit.** In a Claude harness,
   `/exit`. The "[skein] claude exited (0)" line should appear
   *below* Claude's last frame (not overwriting it). Press Enter.
   The xterm should briefly clear, then show a fresh shell prompt
   in your default shell. *Expected fail modes:* Claude's last
   frame missing → phase 2 data-flush regression on Windows.
   Blank viewport with blinking cursor → React isn't unmounting
   on cmd change.

3. **Resume from shell.** From the Enter-for-shell shell, run
   `claude --resume <uuid>` (UUID lives in the harness's stored
   sessionId; future polish could surface it in the [skein] line).
   The conversation should re-attach.

4. **Skein restart resume.** Quit Skein. Reopen. The harness's
   captured sessionId is in `skein.db`; chapter 5's resume flow
   spawns Claude with `--resume <uuid>` automatically. The
   conversation re-attaches with no picker.

Linux notes (untested):
- PTY layer is `posix_openpt` like macOS, behaviour should match.
- xterm.js renders the same in any WebView backend; the only
  open question is GTK / Wayland font rendering, which isn't
  PTY-related.
- Treat Linux as "should work, confirm when chapter 8 produces
  an artifact."

Windows notes (untested):
- ConPTY's reader-pipe-stays-open-after-child-exit is the
  failure-relevant quirk. Phase 2's data-flush timeout addresses
  the trailing-frame race; verify Claude's goodbye line is fully
  visible.
- Mashing Enter-for-shell rapidly: each press triggers an
  unmount + remount, which kills the old PTY then spawns a new
  one. The chapter started by adding a 250 ms ConPTY throttle
  (microsoft/vscode#71966 mitigation) and removed it as part of
  the remount refactor — the React unmount/mount sequence is
  itself slower than 250 ms in practice, and ConPTY hangs were
  documented around tight kill/spawn loops in the same JS thread.
  If Windows users see ConPTY hangs, restoring the throttle is a
  one-line fix.

## Out of scope for chapter 7

See [`backlog.md`](./backlog.md). Notably:

- **Switching off portable-pty.** It's wezterm's own crate and
  gives us the OS abstractions we need. node-pty (VS Code's
  choice) would mean adopting their workaround stack wholesale.
- **Flow control (PTY pause/resume).** Prototype scale; revisit
  if a TUI ever overwhelms the channel.
- **Scrollback preservation across cmd changes.** Today the
  remount discards the previous xterm's scrollback. Could be
  recovered by writing the previous buffer's last N lines into
  the new xterm before the spawn — a polish item, not a
  correctness one.
- **Surfacing the Claude UUID in the `[skein] exited` line** so
  users don't have to dig for it before running `--resume`.
  Trivial polish, defer.
- **Windows ConPTY kill/spawn throttle.** Phase 3's refactor
  removed it; restoration is a one-liner if a Windows user
  reports hangs.
- **Chapter 8 — distribution.** Once chapter 7 lands, the runtime
  is solid on every OS we ship; chapter 8 is the packaging,
  signing, and notarization pass.
