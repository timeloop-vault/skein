# Chapter 7 — Recon notes

Verified on 2026-05-04 against:

- `portable-pty` 0.8.1 (Skein's PTY layer; same upstream as wezterm's
  `pty` crate — they're the same project)
- VS Code's
  [`vscode/src/vs/platform/terminal/node/terminalProcess.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/terminal/node/terminalProcess.ts)
  (fetched fresh via `gh api`)
- Skein's own `app/src-tauri/src/pty.rs` and
  `app/src/LiveTerminal.tsx` (chapter 2 phase 4 baseline)

The headline finding: **chapter 7 is smaller than the plan assumed.**
Skein already does most of phase 2's work (dual-thread reader + waiter)
and roughly half of phase 3's work (alt-screen + clear-viewport reset
in `respawn`). The real gaps are narrower and concrete.

---

## 1. portable-pty per-OS contracts

### Unix (macOS, Linux) — `src/unix.rs`

- **PTY creation**: `posix_openpt` + `grantpt` + `unlockpt`. Master /
  slave fds returned via `PtyPair`.
- **Resize**: `ioctl(TIOCSWINSZ)`. Setting via the master fd
  automatically delivers `SIGWINCH` to the foreground process group on
  the slave side, which is how TUIs know to redraw.
- **Spawn**: `cmd.pre_exec` calls `setsid` + `ioctl(TIOCSCTTY)` so the
  child becomes session leader of the pty. Without TIOCSCTTY,
  `SIGWINCH` won't reach the child on resize.
- **Child exit detection**: returns `std::process::Child` directly —
  `child.wait()` and `child.try_wait()` are stdlib `waitpid` wrappers.
  Reliable.
- **Reader EOF**: when child exits, the kernel closes its end of the
  slave pty. Master reader sees EOF on the next read. *Almost always
  correlates with child exit*, with two known caveats:
  1. If the child has spawned grandchildren that still hold the slave,
     the reader stays open after the original child exits.
  2. If the child has dup'd its stdout to other places, those copies
     keep the slave alive too.

### Windows (ConPTY) — `src/win/conpty.rs`, `src/win/mod.rs`

- **PTY creation**: `CreatePseudoConsole` (Windows 10 1809+). Caller
  owns the read/write pipes and the `HPCON` handle.
- **Resize**: `ResizePseudoConsole`. Triggers a redraw notification
  inside ConPTY which the child sees as if its window resized.
- **Spawn**: `CreateProcessW` with the ConPTY attached via
  `STARTUPINFOEX.lpAttributeList`. portable-pty stores the resulting
  process handle in `WinChild`.
- **Child exit detection**: `WinChild::try_wait` calls
  `GetExitCodeProcess` and checks for `STILL_ACTIVE`. `wait` blocks on
  `WaitForSingleObject(proc, INFINITE)`. **Reliable.**
- **Reader EOF**: **DOES NOT FIRE on child exit.** The ConPTY object
  holds the writer end of the read pipe; the read side stays open
  until either:
  - The `PsuedoCon` Drop runs, which calls `ClosePseudoConsole` and
    closes the writer end → reader gets EOF;
  - Or you explicitly drop the master.

  **This is the load-bearing fact for chapter 7's Windows handling.**
  A reader-only thread will block forever after the child exits unless
  someone separately notices the child is gone.

### Implication for Skein

The portable-pty / wezterm pattern is "spawn returns a `Child`; you
own the wait loop yourself." Both `try_wait` and the blocking `wait`
work uniformly across OSes. The OS difference is **whether reader EOF
correlates with child exit** (Unix: usually; Windows: never).

## 2. VS Code's `terminalProcess.ts` patterns

VS Code's terminal layer wraps `node-pty` rather than portable-pty
directly, but the lessons translate. Things they do that Skein
currently doesn't:

### 2a. Data-flush timeout after exit (`DataFlushTimeout = 250 ms`)

```ts
this._register(ptyProcess.onExit(e => {
    this._exitCode = e.exitCode;
    this._queueProcessExit();
}));
// ...
private _queueProcessExit() {
    if (this._closeTimeout) clearTimeout(this._closeTimeout);
    this._closeTimeout = setTimeout(() => {
        this._closeTimeout = undefined;
        this._kill();
    }, ShutdownConstants.DataFlushTimeout); // 250 ms
}
```

After `onExit` fires, VS Code waits **250 ms** for any further data
events (the timer resets on each one), then forces a kill and fires
its own `_onProcessExit`. The bug they're working around: node-pty on
Windows ConPTY can lose trailing data if the process is killed before
the reader has drained.

**Skein equivalent**: today `pty.rs` emits `PtyEvent::Exit` the moment
the waiter thread sees `child.wait()` return, regardless of whether
the reader has caught up. On Windows this is a known race — the last
frame of TUI output can be lost.

### 2b. Kill/spawn throttle on Windows ConPTY (`KillSpawnThrottleInterval = 250 ms`)

```ts
private async _throttleKillSpawn(): Promise<void> {
    if (!isWindows || !hasConptyOption(...) || !this._ptyOptions.useConpty) return;
    if (this._ptyOptions.useConptyDll) return; // fixed in newer conpty.dll
    while (Date.now() - TerminalProcess._lastKillOrStart < Constants.KillSpawnThrottleInterval) {
        await timeout(...);
    }
}
```

Killing then immediately re-spawning on Windows ConPTY can hang the
host. VS Code throttles to a minimum 250 ms between kill and the
next spawn. Tracked across [microsoft/vscode#71966](https://github.com/microsoft/vscode/issues/71966),
[#117956](https://github.com/microsoft/vscode/issues/117956),
[#121336](https://github.com/microsoft/vscode/issues/121336).

**Relevant for Skein's R-retry**: if the user mashes R quickly on
Windows, we kill then immediately spawn — exactly the pattern VS Code
throttles. This is a likely source of the "R is fragile" symptom on
Windows specifically.

### 2c. `conptyInheritCursor` on launch with initialText

```ts
this._ptyOptions = {
    // ...
    conptyInheritCursor: useConpty && !!shellLaunchConfig.initialText
};
```

Forces ConPTY not to redraw the whole viewport on launch when
`initialText` is set. Out of scope for Skein right now — we don't
seed initial text — but worth knowing if we ever pre-write content
before spawning.

### 2d. Flow control via pause/resume

Pause the PTY when unacknowledged char count exceeds a threshold.
Out of scope for Skein at prototype scale; can revisit if a
chatty TUI ever overwhelms the channel.

## 3. Skein's existing PTY manager (what's already correct)

Re-read `app/src-tauri/src/pty.rs` against the patterns above:

- ✅ **Dual-thread setup** (reader + waiter). The doc-comment at the
  top explicitly calls out the Windows ConPTY pipe-stays-open quirk
  ("Two threads is load-bearing on Windows"). The waiter blocks on
  `child.wait()` and emits `PtyEvent::Exit` regardless of reader
  state. Phase 2's "child-exit detection" is mostly already done.
- ✅ **Slave dropped immediately after spawn** (`drop(pair.slave);`).
  This is what lets Unix reader EOF correlate with child exit — without
  it, the master would still hold a slave reference and reader would
  never EOF.
- ✅ **Killer cloned and stored** in `Pty.killer`, separate from the
  child handle, so `kill()` works even after the waiter consumed the
  child via `wait()`.

Things missing relative to VS Code:

- ❌ **No data-flush timeout** between waiter signaling exit and the
  Exit event reaching the frontend. If a TUI prints a final frame
  the moment before quitting, on Windows ConPTY the reader may not
  have drained it before the waiter says "gone."
- ❌ **No kill/spawn throttle** for the R-retry path on Windows.
- ❌ **No explicit `ClosePseudoConsole`-equivalent on exit** — we
  rely on Drop running through portable-pty when the manager entry
  is removed. Should be sufficient because `pty_kill` removes the
  entry, but worth verifying the Drop actually fires the close in
  the order VS Code's pattern expects.

## 4. Skein's existing xterm reset (what's already correct)

Re-read `LiveTerminal.tsx`'s `respawn` function. It already writes
**three** of the four DEC sequences chapter 7 phase 3 was going to
add:

```ts
//   \x1b[?1049l — exit the alternate screen buffer if we
//                 were left in it (some TUIs terminate
//                 without restoring main screen).
//   \x1b[2J     — clear the visible viewport (does NOT touch
//                 main-screen scrollback).
//   \x1b[H      — home the cursor.
term.write("\x1b[?1049l\x1b[2J\x1b[H");
term.scrollToBottom();
```

Things still missing relative to phase 3's plan:

- ❌ **`\x1b[0m` — SGR reset.** Without this, the previous child's
  active colour / bold / underline state bleeds into the next child's
  first writes until *it* sets its own SGR.
- ❌ **Explicit `pty_resize` after spawn to trigger SIGWINCH /
  ResizePseudoConsole.** We rely on the new child issuing its own
  redraw on startup; some TUIs (particularly older alt-screen
  programs) only redraw on a window-size signal.

## 5. Identified gaps and remaining work

The chapter 7 plan was sized for "rewrite the substrate." The recon
finds the substrate is mostly there. The actual remaining work:

| # | Gap                                                  | Where        | Phase |
|---|------------------------------------------------------|--------------|-------|
| 1 | Data-flush timeout after `child.wait()` returns      | Rust `pty.rs` | 2    |
| 2 | Kill/spawn throttle on Windows ConPTY                | Rust `pty.rs` or LiveTerminal `respawn` | 2 |
| 3 | SGR reset (`\x1b[0m`) in `respawn`                   | `LiveTerminal.tsx` | 3 |
| 4 | Explicit `pty_resize` after spawn during R-retry     | `LiveTerminal.tsx` | 3 |
| 5 | Verify alt-screen DECRST 1049 actually fires on TUIs that died unclean | Manual / phase 4 | 4 |
| 6 | Test matrix: Claude → R → Claude / opencode / shell  | Manual       | 4 |
| 7 | Document Windows-runtime-test plan for #1 + #2       | Notes        | 5 |

This is significantly less than the plan suggested. The substrate
work was effectively done in chapter 2 phase 4 — the bug isn't that
the substrate is wrong, it's that it has a few **specific** holes
(data flush, throttle, SGR reset, post-spawn resize).

## 6. Implications for the chapter 7 plan

Phase 2 should be re-scoped to "fill the two specific Windows-side
gaps" — data-flush timeout and kill/spawn throttle. Both are bounded
changes to `pty.rs` (or, for the throttle, LiveTerminal's respawn
path).

Phase 3 should be re-scoped to the **two missing reset bytes** plus
the post-spawn resize. The bulk of the reset block already exists.

Phase 4 stays as drafted — the test matrix is the point.

Phase 5 stays as drafted — Windows runtime verification.

Net: chapter 7 is closer to a 200-line patch than a thousand-line
rewrite. The rewrite framing came from the chapter 2 backlog being
written before the chapter 2 phase 4 fix landed. Phase 4 already
delivered most of what the backlog item asked for; phase 7 is the
finish.

## 7. Out of scope

- **Switching off portable-pty.** It's wezterm's own crate and gives
  us the OS abstractions we need. Replacing with node-pty (VS Code's
  choice) would mean adopting VS Code's specific pattern lock,
  stock — including the bugs they've worked around. Stay on
  portable-pty.
- **Flow control (pause/resume).** Prototype scale; revisit if a TUI
  ever overwhelms the channel.
- **`conptyInheritCursor`**. We don't seed initialText at spawn.
  Revisit if we ever do.
