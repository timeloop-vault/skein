# skeind P2b: the daemon-owned terminal (#584)

This doc replaces the "snapshot, then raw live bytes" terminal flow of
[skeind-p2b-design.md](skeind-p2b-design.md) (its D2 consequences, its
section "The PTY channel and the snapshot", and slices 1 and 2). The
rest of that doc stands. A user-facing picture of the change is in
[explainers/daemon-owned-terminal.html](explainers/daemon-owned-terminal.html).

In one sentence: the daemon's terminal engine is the only thing that
ever parses harness PTY output, and a client only draws what the daemon
sends it. This is the tmux and mosh model.

## Why

The first P2b design kept a headless emulator in the daemon for the
re-attach snapshot and then forwarded the harness's raw bytes, which
the client's own engine (xterm.js) parsed. Two engines parsed the same
stream.

Slice 1 (#793) built a replay gate that compares the `vt100` crate with
`@xterm/headless` on a recorded Claude Code session and on a synthetic
scrollback session, 24 cut points in all. Its findings, received from
that room on 2026-11-18:

| # | Mismatch | Kind | Fixable from outside the engine |
| --- | --- | --- | --- |
| 1 | OSC 8 hyperlinks lost (16 cuts) | `vt100` gap, no per-cell link | No |
| 2 | No reflow on a width change; data past the narrowest width is lost | `vt100` gap | No |
| 3 | Wrap flag cleared on any width change (alternate buffer) | `vt100` gap | No |
| 4 | Stale wrap flag on a row rewritten after CR LF | Engine difference | No |
| 5 | A wide character pushed to the next row sets no wrap flag | `vt100` gap, cosmetic | No |
| 6 | Wide character straddling the last column after a shrink | xterm.js quirk | No |
| 7 | `?1049h` homes the cursor in `vt100`, keeps it in xterm.js | Engine difference | No |
| 8 | Modes 1004, 2026, insert, `?7`, `?45` not tracked | `vt100` gap | Flag only, through a callback |
| 9 | A snapshot inside a synchronized (2026) frame shows it half drawn | Both engines | No |

Items 4, 6 and 7 are not bugs in either engine. They are places where
two correct engines disagree, and more of them appear for as long as
two engines parse the same bytes. The same holds for any later client:
an iOS or Android terminal widget is a third engine.

The same room also found that the xterm.js serialize addon has its own
flaws (it never emits OSC 8, and it misplaces the cursor on an exactly
full last row), so "use xterm.js on both ends" does not close the gap
either.

## Decisions

Each was asked of the user and decided on 2026-11-18.

### T1. One engine, in the daemon

Decided by Stefan, 2026-11-18.

**Decision.** The daemon keeps one terminal engine per PTY. It is the
only parser of harness output. Clients receive what the daemon derives
from its own grid, never the harness's bytes.

**Reason.** The app is a dumb view and the daemon owns everything. Two
engines on one byte stream drift, and the drift cannot be fixed without
patching an emulator.

**Rejected.**
- Snapshot on attach, then raw live bytes (the earlier D2 flow).
- The same engine on both ends (a Node sidecar running
  `@xterm/headless`): it needs a Node runtime beside the daemon, and it
  binds every future client to xterm.js.

**Consequences.**
- D2's requirement stands: a re-attaching view gets full styled
  scrollback. Only its mechanism changes.
- The engine sits in the live path, so its gaps show always, not only
  on re-attach. Engine fidelity matters more than before (T3).

### T2. The client protocol is a VT redraw in a fixed subset

Decided by Stefan, 2026-11-18.

**Decision.** The daemon diffs its grid per viewer and sends each
change as a frame of VT sequences from a small fixed subset. Any
terminal widget can draw it: xterm.js on the desktop today, a native
widget on a phone later.

**Reason.** The desktop keeps xterm.js as its painter, with selection,
copy, links and font rendering as they are. The wire stays a byte
stream, so the socket slice (#796) barely changes. The encoder is
written once, in the daemon, for every client.

**Rejected.**
- Cell diffs drawn by a grid view we write (mosh's model taken all the
  way): zero drift, but a terminal renderer per platform plus our own
  selection, copy, link hit-testing, IME and accessibility, in the area
  where most shipped regressions have lived.
- Cell diffs on the wire with xterm.js as the painter: a new binary
  frame format to version, and the frame-to-VT translator repeated in
  every client.

**Consequences.**
- xterm.js still parses, but only sequences the daemon generates. Its
  state can no longer drift for good: a full redraw always resyncs it.
- Known residual: a client that measures a character's width
  differently from the daemon. The encoder re-positions the cursor
  after every wide or ambiguous-width cluster, so an error stays inside
  one cell run.
- Each frame is self-contained, so a second encoding of the same
  per-viewer diff (structured cells for a native view) can be added
  later without touching the engine.

### T3. The engine is `alacritty_terminal`, behind a trait, gated first

Decided by Stefan, 2026-11-18.

**Decision.** `alacritty_terminal` 0.26.0 is the engine. It sits behind
the engine trait, and the first slice runs the #793 fixtures through it
before anything depends on it.

**Reason.** It is the only candidate with a sourced answer for all
three hard gaps: per-cell hyperlinks, reflow of the main screen, and
synchronized output. It also has damage tracking, mode flags and
replies to device queries. It is pure Rust, on crates.io, and builds on
Windows.

**Rejected.**
- `vt100` (0.18.1), the #793 engine: no OSC 8, no reflow, no 2026, no
  query replies. Upstream closed reflow unmerged and has not reviewed
  the hyperlink change, so a fix means owning a fork.
- `libghostty-vt` (0.2.2): the highest fidelity claims, but a
  third-party binding over a pre-1.0 C API, Zig 0.17 on every build
  machine and in the release pipeline, and no proof found that it
  builds on Windows.
- `wezterm-term`: not on crates.io; git-only or a third-party fork.

**Consequences and what is verified.** Verified means read in the
crate's source, docs or changelog on 2026-11-18. Nothing here had been
compiled or run when this was written; the gate has since measured it,
see "T-1 gate results".

- Verified: hyperlinks per cell; `Term::resize` reflows the main grid
  and never the alternate grid; a `Term` plus the `vte` processor runs
  with no PTY; history through the scrolling-history setting; no
  serialiser of any kind, so the redraw encoder is ours.
- Verified: synchronized output is handled in the `vte` processor by
  buffering, up to 150 ms or 2 MiB. A frame is cut only when no
  synchronized update is pending.
- Verified: it pulls PTY and event-loop dependencies with no feature
  gate. They are unused here.
- Claimed, for the gate to confirm: a per-line damage API; device-query
  replies delivered through its event listener; kitty keyboard flags
  readable.
- Known gap: no grapheme-cluster mode. Wide and zero-width characters
  are stored per cell.
- Fallbacks if the gate fails, in order: a spike of `libghostty-vt`
  including a Windows build; then a patched `vt100`.

### T4. The new path lands beside the old one, behind a developer flag

Decided by Stefan, 2026-11-18.

**Decision.** The new terminal path lands on main slice by slice, off
by default. A developer flag turns it on; the flag is not in Settings.
Today's raw-bytes path stays the default and untouched until the new
one passes the gate and a dogfood pass. Then one slice flips the
default and deletes the old path.

**Reason.** Skein must work out of the box at every step on main. D1
rejected an opt-in switch with the default flipped later, for the
daemon mode. This is a deliberate, narrower exception, decided by the
user with D1 in view: the flag is for developers only, it covers the
terminal path only, and the slice that deletes it is planned from the
start.

**Rejected.**
- A long-lived integration branch: one large merge at the end, growing
  conflicts with the other #584 slices in `harness_runner/` and `pty/`,
  and no dogfooding from main.
- A per-harness switch in Settings: the second product path D1
  rejected.

**Consequences.**
- Proposed: the flag is an environment variable read once at daemon
  start, `SKEIN_TERMINAL=frames`. Unset means today's path. The spawn
  result tells the view which mode a harness runs in.
- Two terminal paths exist in the tree for the length of the work. The
  flip slice deletes the old one, and rollback is a revert of the flip.
- Proposed: the flip lands before the cutover (slice 11 of the P2b
  doc), so the socket only ever carries frames and no release ships
  detached mode with the raw path.

## The flow

Old, as built today and as the first P2b design kept it for live
output:

1. The harness writes bytes to its PTY.
2. The daemon's reader and coalescer batch them (16 ms, 64 KB).
3. The daemon forwards the raw bytes to the view.
4. xterm.js in the webview parses them and draws.
5. (First P2b design only.) A second engine in the daemon parses the
   same bytes, for the re-attach snapshot.

```mermaid
flowchart LR
    H["Harness"] -->|"1 bytes"| P["PTY reader and coalescer (2)"]
    P -->|"3 raw bytes"| X["xterm.js parses and draws (4)"]
    P -.->|"5 same bytes"| E["Daemon emulator, snapshot only"]
    E -.->|"snapshot on attach"| X
```

Figure 1: the old flow, two parsers on one byte stream. Renders the
numbered steps above; the text is authoritative (non-normative).

New:

1. The harness writes bytes to its PTY.
2. The daemon feeds them to that PTY's engine, the only parser. The
   engine answers device queries itself.
3. The engine marks what changed (damage). History rows that leave the
   screen stay in the engine.
4. The frame scheduler waits for the coalescing window and for any
   synchronized update to finish.
5. For each viewer, the encoder diffs the grid against what that viewer
   was last sent and writes one frame in the VT subset.
6. The client writes the frame into its terminal widget, which draws.
7. Daemon features (Hermes screen phase, prompt delivery, pane notices)
   read the engine, with or without a client.

```mermaid
flowchart LR
    H["Harness"] -->|"1 bytes"| G["Engine: grid, history, modes (2, 3)"]
    G -->|"query replies"| H
    G --> S["Frame scheduler (4)"]
    S --> N["Encoder, per viewer (5)"]
    N -->|"frame, VT subset"| A["Desktop: xterm.js draws (6)"]
    N -->|"frame, VT subset"| B["Later: phone widget draws (6)"]
    G --> F["Daemon features read the screen (7)"]
```

Figure 2: the new flow, one parser and dumb viewers. Renders the
numbered steps above; the text is authoritative (non-normative).

## The engine

One engine per PTY, created at spawn at the PTY's size, alive until the
harness record is removed. It outlives the child, so an exited pane
keeps its screen.

```text
trait TerminalEngine: Send {
    feed(bytes) -> replies      // bytes to write back to the PTY
    resize(rows, cols)
    frame(&self, &mut view) -> Option<bytes> // diff against that view's last frame
    screen_text() -> rows        // for daemon features
    modes() -> Modes             // bracketed paste, mouse, alt screen
    cursor() -> (row, col)
    sync_deadline() -> Option<Instant> // when an open sync update expires
    flush_sync() -> replies     // the daemon calls it at that deadline
}
```

The last two were added by the T-1 gate: the library never expires a
synchronized update on its own. `frame` joined the trait in T-2, as
`fn frame(&self, view: &mut ViewerState) -> Option<Vec<u8>>`. The
engine implements it by calling `pty::redraw::encode_frame`, a pure
function over a `GridSource` (what the encoder reads from the engine)
and the viewer's `ViewerState` (what that viewer was last sent).

- `frame` with a fresh view state is the attach snapshot. There is no
  separate snapshot code path.
- `frame` returns nothing while a synchronized update is pending, so no
  viewer ever sees a half-drawn frame (mismatch 9).
- `feed` returns the engine's replies to device queries (cursor
  position, device attributes). The daemon writes them to the PTY. This
  replaces both the fixed `1;1` answer the daemon gives with no view
  attached, or when a send to the view fails, and the answer xterm.js gives through the input path today.
- History is bounded at 5000 rows, the client's scrollback setting
  today (`HISTORY_ROWS` in the #793 work).
- Nothing but the harness's own output is ever fed to the live parser.
  A PTY chunk often ends in the middle of an escape sequence or a UTF-8
  character, so injected bytes abort the pending sequence and garble
  the next chunk (found by the #793 room, whose snapshot once did
  this). Reads, frames and notices work on the grid, never through
  `feed`.
- Every call is wrapped in `catch_unwind`. A caught panic drops that
  PTY's engine, rebuilds it empty at the current size, and marks every
  viewer for a full redraw. It does not kill the daemon.

## The frame

A frame is a byte string in this subset and nothing else:

- Cursor position, absolute (`CUP`).
- Colours and attributes (`SGR`), always starting from a reset.
- Printable text.
- Erase to end of line (`EL`), erase display and scrollback at a full
  redraw.
- Line feed at the bottom row, only to push rows into the client's
  scrollback.
- OSC 8 open and close around linked runs.
- Cursor visibility and shape.
- Mode mirrors, so the client encodes input correctly: application
  cursor keys and keypad, mouse tracking and its encoding, bracketed
  paste, focus reporting, kitty keyboard flags, alternate screen.
- Title (OSC 0/2) and bell, passed on as they happen.
- Synchronized-update brackets around the whole frame.

Modes and the title are state, not events. Each viewer's last-sent
state includes them, and a frame carries only the difference, so a
viewer that skipped frames still ends in the right modes. The bell is
an event: it is sent with the next frame and several bells in one
frame collapse to one.

Not in the subset: scroll regions, insert and delete line or character,
relative cursor moves, tabs, character sets, origin mode, any query.

Rules the encoder follows:

- Screen rows. For each damaged row that differs from what the viewer
  has: position, reset, write the runs, erase the rest.
- Wrapped rows. Proposed: where the engine marks a row as the
  continuation of the one above, the encoder writes through the right
  margin with autowrap on, so the client's widget sets its own wrap
  flag. Copy then joins the line and the file-path links still match
  across rows. The gate compares wrap flags. Decided in T-2 by
  measurement: write-through-margin is confirmed by the gate. A row
  that ends early because a wide character did not fit is written too:
  the encoder stops before the last column, so the client wraps the wide
  character itself and sets its own flag. This supersedes the "no wrap
  flag in the client (mismatch 5)" fallback.
- Erase first. Decided in T-2 by measurement: every row the encoder
  positions at column 1 starts with an erase: `CUP`, reset, `EL`, then
  its runs. xterm.js keeps a stale wrap flag after `CUP` and text, and
  only an `EL` at column 1 clears it.
- A wide character in the last column. Decided in T-2 by measurement:
  after a reflow the engine can leave a wide character starting in the
  last column (mismatch 6). No client can draw it there, so the frame
  sends one blank in its style. The stored row stays the engine's.
- Ambiguous width. Decided in T-2: a `CUP` follows any width-2
  character, any non-ASCII character whose `unicode-width` `width_cjk`
  differs from its `width`, and any cell holding more than one code
  point, so a client that reads the width differently is wrong by one
  cell and no more.
- History. The client keeps a local scrollback that only the daemon
  fills. The engine wrapper keeps a monotonic count of rows that have
  entered history, and each viewer's state holds the count at its last
  frame. The difference is k, the rows to push. A history-size delta is
  not used, because it stops growing once history is full. Only rows
  that enter history count: a scroll inside a scroll region, or an
  insert or delete line, is plain row damage. To push k rows: repaint
  the viewer's top k rows with their final content, send k line feeds
  at the bottom row, then repaint the damaged screen rows. More than a
  screenful goes in batches. If k is larger than the history bound, or
  cannot be derived, the viewer gets a full redraw. T-1 showed the
  engine can report rows entering history (see "T-1 gate results").
  As built in T-2: the newest k history rows are repainted over the
  viewer's top rows, only where they differ, then `CUP` to the bottom
  row and k line feeds, in batches of at most a screenful. A batch cut
  avoids splitting a wrapped line where it can. A push is trusted only
  when `history_len == min(prev_len + k, 5000)`. Otherwise, after a
  rebuild, or after any resize since the last frame (the engine keeps a
  resize counter), the viewer gets a full redraw. Decided in T-2 by
  measurement.
- Alternate screen. Each viewer's last-sent state holds both screens.
  On entry the mode mirror switches the client's buffer, and a full
  repaint of the alternate screen with an explicit cursor follows, so
  engine differences on entering it (mismatch 7) are overwritten. On
  exit the mode mirror switches the client back, the main screen is
  diffed against the viewer's stored main screen, and the cursor is set
  explicitly. No history is pushed while the alternate screen is
  active. The alternate screen has no history. Claude Code enters it at
  start and never leaves (#793 fixture), so its common case is plain
  row diffs. As built in T-2: entering sends `?1049h` and repaints
  every alternate row, with no `CSI 3 J`, which would wipe the main
  scrollback behind it. Exiting sends `?1049l`, pushes if needed, and
  diffs the main screen against the stored main. A viewer that attached
  on the alternate screen, or whose main went unknown through a resize
  or rebuild there, gets a full redraw with history on exit.
- Width change. The engine reflows. Every viewer then gets a full
  redraw: clear screen and scrollback, the history as flowing lines,
  the screen, modes and cursor. The client's own reflow result is never
  used. A width change resets the client's scroll position and
  selection; this is a known cost. A width change is as designed. A
  height-only change is a deviation, decided in T-2 by measurement: any
  size change is a full redraw, history included. The engine resizes its
  newer screen and the client its older one, so their scrollbacks
  diverge whenever output arrived since the last frame, and the encoder
  cannot tell whether a viewer is in sync. A later slice could cut a
  frame just before the resize and relax this. Note for T-3.
- Frame layout, decided in T-2, in order: `?2026h`; `CSI 0 m`;
  `?1049h` or `?1049l`; for a full redraw, `CSI H`, `CSI 2 J`,
  `CSI 3 J` and the history flow; the rows; `OSC 2`; the modes; `BEL`;
  a pen reset; `CUP` to the cursor; cursor shape and visibility;
  `?2026l`.
- Per-viewer diffing, decided in T-2: the encoder compares rows against
  the rows stored for that viewer. The engine's damage is not used. Using
  it is a possible optimisation for T-3.
- Bell and links, decided in T-2: a bell from before a viewer's first
  frame is not rung. OSC 8 is sent without `id=`, because the dump has
  none.

Framing and rate:

- The wire is the existing PTY data event, now carrying frames. A
  frame is never split across events.
- The scheduler has a leading edge: the first output after a quiet
  period is cut at once, so a keystroke echo does not wait. After that
  it cuts at most one frame per 16 ms per PTY, the window the coalescer
  uses today, and output inside the window lands in the next frame. A
  synchronized update holds the frame until it ends, at most the
  engine's 150 ms; that delay is the harness's own request.
- Per viewer the daemon keeps the last-sent screen and one pending
  frame at most. A slow viewer skips intermediate states and gets one
  larger diff. Nothing queues without bound.
- A PTY with no output costs nothing.

## Attach, reconnect, several viewers

- Attach adds the viewer and cuts its first frame under one lock. The
  first frame is a full redraw: history, screen, modes, cursor.
- Reconnect is an attach. The PTY stream is state, not a log: the
  event seq and replay of #757 cover events (rooms, phase, mail), and a
  terminal resyncs from the engine. A `gap` on the event stream needs
  no terminal action.
- A view can ask for a full redraw at any time. This replaces the
  one-row resize repaint.
- Several viewers share one PTY and one grid. The PTY has one size
  owner: the last active client, re-asserted when it becomes active
  again (unchanged from the P2b doc). Proposed: a viewer that is not
  the owner sets its widget to the grid's size and letterboxes or
  scrolls it; it becomes the owner on input. Each frame stream starts
  with the grid size and announces a change before the redraw.
  Proposed: the owner's resize is debounced (a short trailing delay)
  before the PTY and the engine are resized, so a window drag causes
  one reflow, not one per pixel. Ownership moves only when a viewer
  becomes active (focus and input) and its size differs. Two viewers of
  different sizes typing in turn will flip the size each time; this is
  accepted, as in tmux, and listed under Risks.
- Input from any viewer is written to the PTY.

1. The harness writes output with no viewer attached, and the engine
   parses it.
2. The engine answers the harness's device queries.
3. A view attaches and sends its size.
4. The daemon adds the viewer and cuts its first frame under one lock.
5. The daemon sends the grid size, then a full redraw.
6. More output arrives.
7. The daemon sends a diff frame.
8. The view sends input.
9. The daemon writes it to the PTY.

```mermaid
sequenceDiagram
    participant U as View
    participant D as skeind
    participant E as Engine
    participant H as Harness
    H->>E: 1 output, no viewer attached
    E->>H: 2 query replies
    U->>D: 3 attach harness, size
    D->>E: 4 add viewer and cut first frame, one lock
    D->>U: 5 grid size, then full redraw
    H->>E: 6 more output
    D->>U: 7 diff frame
    U->>D: 8 input
    D->>H: 9 input bytes
```

Figure 3: attach and live frames. Renders the numbered steps above;
the text is authoritative (non-normative).

## What the client still owns

| Concern | Who | How |
| --- | --- | --- |
| Input encoding (keys, mouse, paste, bracketed paste) | Client widget | It encodes from the mirrored modes and sends bytes, as today (`pty_write`). The daemon is the authority for the modes |
| Selection and copy | Client | Local, over the local screen and scrollback |
| Scrollback browsing | Client | Local scrollback filled by the daemon, 5000 rows. Rejected for now: daemon-paged scrollback (a round trip per scroll and a new UI). Worth revisiting for a remote or phone client |
| Resize | Size owner | The owner fits its pane and sends the size; the daemon resizes PTY and engine |
| Links (OSC 8) | Client | The daemon re-emits OSC 8; the widget's link handler is unchanged |
| URL and file-path links | Client | They read the client's buffer, as today. They need the wrap flags above |
| Bell, title | Daemon passes on | Neither is used by the app today |
| Focus events | Client widget | Sent when the mirrored focus mode is on |
| Device-query replies | Daemon engine | The client never sees a query |
| macOS wheel reports | Client | Read the mirrored mouse mode, as today |

What is lost or different:

- Inline images (sixel, kitty, iTerm): not drawn. The engine has none.
  Nothing is lost against today, because the app loads no image addon.
- Anything the engine does not model is invisible to every client. This
  is the price of one engine, and the reason for T3.
- Character width follows the daemon's engine, not the client's
  Unicode table.
- Local writes into the widget are no longer allowed, because they
  would differ from what the daemon thinks the viewer shows. See
  "Pane notices" below.

## Impact on features that read terminal output

Found by a code survey on 2026-11-18.

| Feature | Today | With the engine |
| --- | --- | --- |
| ConPTY cursor query | With no view attached, or when a send to the view fails, the daemon scans for the query and answers a fixed `1;1` (`pty/sink_slot.rs`); with a view xterm.js answers, and the view buffers that answer during spawn (`ptyInputBuffer.ts`) | The engine answers always, from its real cursor. Both mechanisms are deleted at the flip |
| Hermes screen phase | The webview reads its xterm buffer every second and reports a reading (`screenPhase.ts`, `setScreenReading`); `unknown` with no client | The daemon reads `screen_text` itself. The reader is ported to Rust and the client input is deleted at the flip (T-5: done behind the flag, see "T-5 as built") |
| Prompt delivery, paste mode | A tracker parses the bracketed-paste mode out of raw output (`prompt_delivery/paste_mode.rs`) | Reads the engine's mode flag. The tracker is deleted at the flip. The write path is unchanged. The webview's own read of the same mode off its widget keeps working through the mode mirror. (T-5: the gate reads the flag behind the flag, see "T-5 as built") |
| Composer check before a send | The webview reads the visible screen (`harnessInputRegistry.ts`) | Unchanged for now: the client's buffer holds the same screen. Proposed follow-up: the daemon's gate reads the engine |
| Pane notices ("Press Enter for shell", the opencode restart hint, the session notice) | Written by the webview into its own terminal, not the PTY | The daemon authors them, so every viewer sees them and no viewer's state diverges. They never go through the live parser: a notice after exit is appended once the stream has ended, and a notice on a running pane is a daemon-owned overlay row the encoder draws. The exact form is decided in T-5 (decided and built for the exit notices, see "T-5 as built") |
| Post-exit shell | Enter on an exited pane asks the daemon for a shell | Unchanged. The shell is a new child on the same engine or a fresh one; decided in the slice |
| Opencode shell follow (#517) | Output ticks in the view trigger a process scan | Unchanged: a frame is an output tick. It can move to the daemon later |
| Idle heuristic and waiting patterns | The daemon strips escapes from raw output into a tail and matches it (`harness_phase/io.rs`, `patterns.rs`) | Unchanged. It is a second, very small reader of raw bytes inside the daemon. Proposed follow-up: match engine rows instead |
| Re-attach repaint | A one-row resize and back (`terminalRepaint.ts`) | Deleted at the flip. The first frame is the repaint |
| Output gate in the view | Buffers and slices writes to xterm.js | Unchanged |
| Font size and zoom | The view refits its pane and sends a new size | Unchanged for the size owner. A non-owner's zoom changes only how large the shared grid is drawn |

## Delivery

The order keeps main working out of the box (T4):

1. The engine, the encoder and the gate land as library code. Nothing
   calls them in the product.
2. The fan-out and the frame path land behind the flag. With the flag
   unset every byte takes today's path.
3. The daemon learns to read its own screen and to author pane
   notices, then the view learns the frame mode, both only when the
   flag is on.
4. Dogfood with the flag on, on the dev and local-release profiles.
5. The flip: frames become the only path, and the raw path, the flag
   and the helpers listed above are deleted.
6. The socket work (slice 7) and the cutover (slice 11) follow the
   flip.

The flip's conditions: the gate is green with no unexplained cut, a
keystroke-echo measurement shows no regression against the raw path,
and the dogfood pass has run with Claude Code, opencode and a plain
shell on Windows and macOS.

## The gate, second version

The #793 gate stays, with a different second terminal:

- Terminal A: `@xterm/headless` fed the recorded raw stream up to the
  cut. This is what a user saw before the change.
- Terminal B: a fresh `@xterm/headless` fed what the daemon sends: the
  frames up to the cut, and separately a full redraw taken at the cut.
- Compared by category, as #793 does: active buffer, cursor, screen
  cells, history and its wrap flags, screen wrap flags, hyperlinks,
  modes.
- Expected differences are pinned per fixture, cut and category with
  their mechanism. An unexpected difference fails; so does a stale pin.
- Added checks: no frame is cut inside a synchronized update; a diff
  stream and a full redraw at the same cut give the same terminal; a
  skipped-frame viewer converges with one that saw every frame.

The fixtures are the two from #793: one real Claude Code session with
four resizes, and a synthetic scrollback session. The real one is
scrubbed, and approved by the user for the public repo on 2026-11-18.

## T-1 gate results

Measured 2026-11-18 with `alacritty_terminal` 0.26.0 and `@xterm/headless`
6.0.0. The engine passed the gate, with the conditions below, so T3
stands. The `vt100` implementation is not part of this change.

### T3's claims, one by one

| T3 item | Outcome | Note |
| --- | --- | --- |
| Hyperlinks per cell | Confirmed | OSC 8 URIs are exact, with query and fragment, BEL or ST. `id=` is kept inside the engine and not dumped. Links survive scrolling into history |
| `Term::resize` reflows main, never alternate | Confirmed with a condition | The main grid reflows even while hidden. While the alternate screen is active it cannot be read |
| `Term` plus the `vte` processor with no PTY | Confirmed | Every test and both replays run this way |
| History through the scrolling-history setting | Confirmed with a condition | The grid keeps rows but offers no scroll event, and history growth stops once full. Rows are counted by wrapping the handler (`counting.rs`) |
| No serialiser | Confirmed | The grid is dumped by `dump.rs`; the redraw encoder is still ours (T-2) |
| Synchronized output buffered, 150 ms or 2 MiB | Confirmed with a condition | Nothing expires on its own. The caller must call `flush_sync` at `sync_deadline` |
| PTY and event-loop dependencies, no feature gate | Confirmed | 19 packages added to the root Cargo.lock and 9 to app/src-tauri/Cargo.lock, none used |
| Per-line damage API | Confirmed with a condition | `Term::damage` needs `&mut self`, so `take_damage` is inherent, not on the trait. Full after resize, screen switch and any scroll; the cursor cell is always listed |
| Device-query replies through the event listener | Confirmed with a condition | Cursor position, primary and secondary DA, kitty flags, DSR, text size and DECRQM reply with exact bytes. Colour queries and a few others get none (below) |
| Kitty keyboard flags readable | Confirmed | Needs `kitty_keyboard = true` in the config. The stack is per screen |
| No grapheme-cluster mode (known gap) | Confirmed | A combining mark or ZWJ joins the previous cell; the next emoji after a ZWJ is its own cell |

### Comparison with xterm.js

The engine's grid was compared with `@xterm/headless` at 24 cuts over
the two fixtures. 19 cuts are identical and 5 differ. Differing cuts per
category: screenWrap 4, cursor 1, screenText 1, screenStyles 1.
activeBuffer, historyText, historyStyles, historyWrap, hyperlinks and
modes differ at none. 12 cuts skip the history categories because the
alternate screen is active and the engine cannot read the main screen
then. Three mechanisms explain every difference:

1. xterm.js reports underline on cells that carry an OSC 8 link even with
   no SGR 4. The engine keeps SGR separate. Link URIs agree at every cut.
   This is an xterm.js quirk, and it is not a pinned difference: the
   comparison excuses, one-sidedly, a cell that carries a link on both
   sides where xterm.js reports underline and the engine does not. It
   excuses exactly 186 cells and the test pins that count (rule in
   `app/src/emulatorGate/linkUnderline.ts`, tested in
   `linkUnderline.test.ts`).
2. Wrap flags: the two engines keep the flag in different places, so
   screenWrap differs at claude-session resize-100x30, resize-80x24,
   resize-120x30 and end-of-stream. Alacritty stores it on the previous
   row's last cell, xterm.js on the continuation row. Inherent, cosmetic.
3. The in-sync-frame cut of scrollback-session differs in cursor,
   screenText and screenStyles: the engine holds the open `?2026` frame
   back, xterm.js applies it at once. By design.

No difference is unexplained. No bug was found in our wrapper. The
expected differences are pinned in `app/src/emulatorGate/expected.ts`.

### What later slices must account for

- The sync timeout needs the daemon to drive it. The trait gained
  `sync_deadline()` and `flush_sync()`. The engine holds a clock and
  stamps the deadline (clock + 150 ms) itself when a feed call returns,
  so it can lag by one feed. Tests use a manual clock.
- While the alternate screen is active, the main screen and its history
  cannot be read. A frame or attach snapshot taken then carries the
  alternate grid only.
- Rows entering history are counted by wrapping the engine's handler.
  The count is exact as "rows pushed by output" but stops matching the
  history's contents after a resize that changes history. A panic
  rebuild empties history and keeps the count. Scrolls on the alternate
  screen are not counted.
- Colour queries (OSC 4, 10, 11), `CSI 14 t`, `CSI > q` and `CSI = c` get
  no engine reply. xterm.js answers the colour and size ones today.
  Primary DA answers as a VT102 (`?6c`).
- Only `?1049` switches screens; `?47`, `?1047` and `?1048` are ignored.
  Mouse tracking levels exclude each other, the latest wins.
- The engine never goes below two columns (and one row). A resize of a
  grid holding text to one column hangs inside the library, and
  `catch_unwind` cannot catch a hang. A wide character in a one-column
  grid panics. Both are avoided by the clamp.
- One real library panic was found and caught by the guard: the 4097th
  kitty keyboard push indexes the wrong stack. The guard rebuilt the
  engine empty at the same size and it stayed usable.
- History is unverified on real Claude Code output: version 2.1.327
  stays on the alternate screen from event 16 and never leaves. The
  synthetic fixture covers history, wrapped lines and wide cells.
- The history-counting wrapper forwards all 71 `vte::ansi::Handler`
  methods (vte 0.15.0). A test pins the vte and `alacritty_terminal`
  versions from Cargo.lock. Re-check it on any bump: a method missing
  from the forward list would be silently dropped.
- The `id=` of a link is not in the dump, so two adjacent runs with one
  URI differ only by id inside the engine.

### Where it is

- Engine: `crates/skein-daemon/src/pty/engine/` (`mod.rs` trait,
  `alacritty.rs`, `counting.rs`, `dump.rs`, `compact.rs`, `tests*.rs`,
  `replay_tests.rs`).
- Fixtures and their README: `crates/skein-daemon/src/pty/engine/fixtures/`.
  The real Claude Code recording was approved by the user for the public
  repo on 2026-11-18, after an extra scrub (plan name and status-line
  text replaced at the same byte length, checked by rendering every
  event).
- Rust side: `cargo test -p skein-daemon engine`, and
  `cargo test -p skein-daemon replay_tests` for the replay.
- JavaScript side: `cd app && npx vitest run src/emulatorGate`.
- Regenerate the engine pictures after an intended engine change and
  review the diff:
  `SKEIN_UPDATE_ENGINE_DUMPS=1 cargo test -p skein-daemon replay_tests`.

## T-2 gate results

Measured 2026-11-19 with `alacritty_terminal` 0.26.0 and `@xterm/headless`
6.0.0, on the two fixtures from T-1. The redraw encoder passed the gate.
The encoder lives in `crates/skein-daemon/src/pty/redraw/`, and the
generated frames in `fixtures/NAME.frames.json` hold what a viewer would
be sent: 172 frames for claude-session and 314 for scrollback-session.

### What was compared

At the same 24 cuts as T-1, a fresh `@xterm/headless` was fed three ways,
and each was compared with the next. Over 24 cuts each:

1. Raw xterm.js, fed the recording, against the frame stream: one viewer
   attached from the start, fed every frame in order.
2. The frame stream against a full redraw taken at the cut: a fresh
   viewer's attach snapshot.
3. The frame stream against a skipped-frame viewer: one framed only at
   the cuts, so its frame is one diff over everything since the last cut.

### Results

- Stream against full, and stream against skipped, are identical at
  every cut except scrollback-session `in-sync-frame`. There the full
  redraw is empty and the skipped viewer lags. Both are by design: no
  frame is cut inside an open `?2026` update, so a fresh or skipped
  viewer is sent nothing at that cut.
- Raw xterm.js against the stream differs at 5 cuts, each with its
  mechanism:
  - claude-session `resize-100x30` and `resize-120x30`, screenWrap: the
    alternate-screen resize wrap flags, the engine difference T-1
    measured at the same cuts.
  - claude-session `resize-80x24`, screenText and screenWrap: the grape
    emoji left in the last column after the 120 to 80 reflow (mismatch
    6), which the frame sends as a blank, and the wrap flag.
  - claude-session `end-of-stream`, screenWrap: the last screen row,
    where raw xterm.js keeps a wrap flag the engine does not. T-1
    measured screenWrap differing at the same cuts, and the frames follow
    the engine.
  - scrollback-session `in-sync-frame`, cursor, screenText, screenStyles
    and modes: raw xterm.js has applied the open update.
- The T-1 link-underline excuse excuses 0 cells, because both sides are
  xterm.js.
- The Rust side asserts that `frame()` returned `None` whenever sync was
  pending. That happens once, in scrollback-session.
- The differences are pinned per fixture, cut and category in
  `app/src/emulatorGate/expectedFrames.ts`. An unexpected difference
  fails, and so does a stale pin.

### Defects the gate found

Three encoder defects, all fixed in T-2:

- A ghost wide glyph: a wide character in the last column was wrapped by
  the client and left a copy on the next row. It is now one blank.
- A stale client wrap flag: a row rewritten by `CUP` and text kept the
  flag from an earlier autowrap. Every row positioned at column 1 now
  starts with an `EL`.
- A missing wrap flag after a wide character that did not fit. The row
  is now written short, so the client wraps the character itself.

### What xterm.js does with `?2026`

This closes the "Not verified" note in Sources. `@xterm/headless` 6.0.0
only records `?2026` (it shows in DECRQM and in `modes`) and never holds
writes back. `?2026l` fires a refresh. The encoder sends a frame as one
write, so this does not matter.

### What later slices must account for

- T-3 drives `frame()` after each scheduler cut, and must call
  `flush_sync` at `sync_deadline`.
- A height-only resize is a full redraw for now. A later slice could cut
  a frame just before the resize and relax this.
- A width change on the alternate screen marks the viewer's main screen
  unknown, so leaving the alternate screen is a full redraw.
- The stored rows cost one screen per screen per viewer.
- `frame()` takes `&self`, so T-3 can share-lock the engine and lock each
  viewer's state separately. A panic in the encoder is caught: the
  viewer is reset with `ViewerState::invalidate()` (its next frame is a
  full redraw), the call returns `None`, and the engine is kept, since an
  encoder panic does not mean the grid is corrupt. `invalidate()` is also
  how a view asks for a full redraw at any time.
- Diffing compares stored rows and ignores the engine's damage. T-3 can
  use the damage as an optimisation.
- The alacritty stand-in client in the Rust round-trip tests wraps a
  last-column wide character and drops wrap flags on rewrites, so the
  Rust tests compare wrap flags only after a full redraw. The xterm.js
  gate is the authority.

### Where it is

- Encoder: `crates/skein-daemon/src/pty/redraw/` (`mod.rs` docs,
  `encode.rs`, `rows.rs`, `history.rs`, `modes.rs`, `sgr.rs`,
  `source.rs`, `state.rs`, `tests*.rs`).
- Frames: `crates/skein-daemon/src/pty/engine/fixtures/NAME.frames.json`,
  regenerated with
  `SKEIN_UPDATE_ENGINE_DUMPS=1 cargo test -p skein-daemon replay` (review
  the diff).
- JavaScript side: `app/src/emulatorGate/frames.test.ts` and
  `expectedFrames.ts`, run with `cd app && npx vitest run src/emulatorGate`.

## T-3 as built

T-3 (#760) put the engine and the encoder behind the PTY. A PTY in
frames mode has one engine and any number of viewers, and each viewer
is sent frames cut from that engine. It is off unless `SKEIN_TERMINAL=
frames` is set in the daemon's environment at start. The variable is
read once, in the state construction of `SharedState::build`
(`daemon.rs`); unset, or any other value, keeps today's raw path. The
spawn result carries `terminalMode` (`"raw"` or `"frames"`), so a view
can tell which path it got. The raw path is unchanged.

### The flow

1. The PTY reader thread sends each chunk it reads to the frame
   scheduler thread.
2. The scheduler feeds every chunk to the hub at once. The hub's one
   engine parses it, and the engine's query replies go to the PTY
   writer.
3. The scheduler cuts at the leading edge (a chunk that comes 16 ms or
   more after the last cut is cut right away), and after that at most
   once per 16 ms. While a synchronized update is open it also wakes at
   the engine's sync deadline and flushes.
4. At each cut the observer (the phase machine) hears the decoded text
   fed since the last cut, once. Then the hub cuts one frame for every
   viewer, and each viewer's sink gets it as a `data` event.
5. Attach adds the viewer, sends `grid`, then a full redraw, under one
   lock, and an `exit` event too if the child is gone.
6. A resize from the size owner reaches the PTY and the engine. Then
   every viewer gets `grid` and a full redraw. A resize from any other
   viewer is only recorded.
7. Input from a viewer makes it the size owner, and its recorded size
   is applied.
8. At exit the hub cuts a last frame, sends `exit` to every viewer, and
   tells the observer once.

```mermaid
flowchart LR
    R["PTY reader thread"] -->|"1 chunks"| S["Frame scheduler thread"]
    S -->|"2 feed at once"| H["FrameHub: one engine"]
    H -->|"2 query replies"| W["PTY writer"]
    S -->|"3 cut, 16 ms"| H
    S -->|"4 decoded text"| O["Observer, phase machine"]
    H -->|"4 data frames"| V1["Viewer 1, size owner"]
    H -->|"4 data frames"| V2["Viewer 2, own state"]
    V1 -->|"5 attach"| H
    H -->|"5 grid, full redraw"| V1
    V1 -->|"6 resize, 7 input"| H
    H -->|"6 resize owner only"| W
    H -->|"8 exit"| O
```

Figure 5: the T-3 fan-out, scheduler, hub and viewers. Renders the
numbered steps above; the text is authoritative (non-normative).

### Rules, as built

- A viewer id is its attach id: monotonic per PTY, the first is 1.
- A viewer whose sink send fails is dropped. Nothing is killed.
- When the owner leaves or is dropped, ownership goes to the remaining
  viewer with the latest attach or input, and its recorded size is
  applied.
- With no owner, the first viewer to attach, resize or write with a
  viewer id becomes the owner.
- A resize with no viewer id (today's `pty_resize`) is applied and
  leaves ownership alone.
- A resize the PTY refuses leaves the engine and the grid alone. A zero
  height or width is refused the same way.
- A resize that equals the current grid does nothing.
- Sizes below the engine minimum (1 row, 2 columns) are refused, so the
  grid never differs from what the engine draws.
- After the child has exited, sizes are only recorded: the PTY and the
  exited screen are not resized, whoever attaches or becomes owner.
- The hub holds the PTY handle weakly, so killing the PTY closes it and
  ends its reader and scheduler threads.
- Exit keeps the engine and the viewers, so an exited pane keeps its
  screen and a later attach gets it, followed by `exit`.
- `detach` without an id removes every viewer. `detach` with an id
  removes only that viewer, and only if it is still attached.
- Daemon writes (prompt delivery, `write`) carry no viewer id, so they
  do not move ownership.
- The scheduler's last act at EOF is a final cut, so the last frame
  goes out before the exit the waiter sends.

### Deviations and open points

- The owner's resize is not debounced. The design proposed a short
  trailing delay; the hub applies each size as it arrives.
- Any resize is still a full redraw with history. The relaxation of
  cutting a frame just before the resize was not taken, and
  `pty/redraw/encode.rs` is unchanged.
- The engine's damage is still not used for diffing.
- Frames are cut under the hub's one lock, not under a shared engine
  lock plus a lock per viewer.
- There is no Tauri command for the redraw request yet.
  `PtyManager::request_redraw` exists. T-4 adds the command and makes
  the view pass its `attachId`: `pty_write` and `pty_resize` already
  accept an optional `attach_id`, but the webview does not send one.
- With the flag on and before T-4, the view still does its one-row
  repaint resize and writes some pane notices locally into the widget,
  so the flag is for development only. T-5 moved the exit banner and
  the opencode server stopped line to the daemon; the others are listed
  under "T-5 as built".
- In frames mode the idle and waiting pattern matching gets its text
  per cut, not per coalesced chunk.

### Where it is

- `crates/skein-daemon/src/pty/frames/`: `mode.rs` (`TerminalMode`),
  `scheduler.rs` (the pure timing loop), `hub.rs` and `state.rs`
  (`FrameHub`, the viewers and the owner), `driver.rs` (feeds the hub,
  tells the observer), and the `*_tests.rs` files.
- `crates/skein-daemon/src/pty/output_path.rs`: the Raw/Frames switch,
  the reader and the pump thread.
- The viewer API on `PtyManager` in `pty/mod.rs`: `with_terminal_mode`,
  `attach_viewer`, `write_from`, `resize_from`, `request_redraw`.
  `PtySpawnResult.terminal_mode` is in `pty/command.rs`.
- Tests: `cargo test -p skein-daemon frames`, and
  `cargo test -p skein-daemon frames_pty` for the real-PTY file
  `pty/frames_pty_tests.rs`.

## T-5 as built

T-5 (#809) made the daemon read its own screen. In frames mode only
(`SKEIN_TERMINAL=frames`), three things that a client used to supply now
come from the engine: the Hermes screen phase, the bracketed-paste mode
for the send gate, and the pane notices at exit. A raw PTY has no
engine, so every read below answers `None` and the old paths run
unchanged.

### The flow

1. `FrameHub::screen_text()` and `FrameHub::modes()` read the engine's
   rows and modes under the hub lock. `PtyManager::screen_text(id)` and
   `modes(id)` forward to them and answer `None` for a raw PTY.
2. After each cut the hub asks the observer `wants_screen()`. If it does,
   the hub takes the rows, releases its lock, and calls
   `screen(rows)`. Nothing is delivered while a synchronized update is
   pending.
3. `RunnerObserver` wants the screen only for a live Hermes harness. It
   reads the rows with `read_hermes` and skips a reading equal to the
   last one. A new reading goes to `PhaseService::client_input` as a
   `ScreenReading`, which wakes the deadline driver.
4. The send gate asks `PtyPort::bracketed_paste(harness_id)`. `gate_facts`
   uses the engine's flag when there is one and the `PasteModeTracker`
   otherwise.
5. At exit the hub flushes any open synchronized update and cuts the
   child's real output. It then feeds the exit notice (from the
   observer's `exit_notice(code)`) and the queued notices to the engine,
   and cuts one frame for every viewer.
6. The hub sends `exit` to every viewer, then tells the observer. The
   observer withdraws its screen reading.
7. A notice asked for with `append_notice` on a live PTY is queued until
   step 5. One that arrives after exit is fed and cut at once. A viewer
   that attaches late gets the notices in its redraw.
8. The view treats the first `grid` event as the sign of frames mode and
   then skips its own exit banner and sidecar line.

```mermaid
flowchart LR
    E["FrameHub: engine"] -->|"1 screen_text, modes"| M["PtyManager"]
    E -->|"2 rows after a cut"| O["RunnerObserver"]
    O -->|"3 read_hermes, ScreenReading"| P["PhaseService"]
    M -->|"4 bracketed_paste"| G["Send gate"]
    O -->|"5 exit_notice"| E
    N["Notice queue"] -->|"5 queued notices"| E
    E -->|"5 one frame, 6 exit"| V["Viewers"]
    D["Sidecar exit hook"] -->|"7 append_notice"| N
    V -->|"8 first grid event"| W["View: skips local notices"]
```

Figure 6: the T-5 screen reads and pane notices. Renders the numbered
steps above; the text is authoritative (non-normative).

### Decision: the form of a notice

The design left the form of a notice to T-5. A notice is engine content
appended only after the child's stream has ended. So the rule that
nothing but the harness's own output goes through the live parser holds
while the stream is live. No overlay row was added to the encoder,
because no notice in scope appears on a running pane.

### Rules, as built

- The hub calls the observer for the screen with its lock released, so
  the observer may call into the phase service.
- A screen is never read while a synchronized update is pending.
- A client `screenReading` input is dropped for a PTY in frames mode
  (`harness_runner/client_input.rs`). The webview still sends it.
- The observer withdraws the reading it delivered at exit, and again when
  the runner retires it (a restart or a stop), before the next process's
  observer exists. A respawn of the same harness starts with no reading and
  an old observer can never clear the new one's. Raw mode delivers
  nothing, so nothing is withdrawn.
- The exit banner is fed after a synchronized update is flushed, so a
  notice never lands inside one.
- Notices are queued in the order: exit notice, then the queued ones.
- The wording is in `harness_runner/notices.rs`, a byte copy of
  `app/src/terminalBanners.ts` and `opencodeSidecarExit.ts`. Change both
  until T-6 deletes the TS side. Tests pin the bytes.
- The paste tracker is still fed in both modes. It is deleted at the
  flip (T-6).
- A post-exit shell is a fresh PTY with a fresh hub, so it has no
  notices from the one before.

### Deviations and open points

- The #517 shell-follow hint and the #679 session notice stay React
  overlays in the view, shown only by the viewer that triggered them.
  They are not widget writes, so they do not block T-4. Moving them to
  the daemon is open.
- The start-failed and unknown-agent banners stay local writes, because
  no PTY and no engine exist then. T-4 must allow or replace them.
- The composer check before a send still reads the client's buffer.
- The idle and waiting patterns still read decoded output text.
- A Hermes pane reads its screen at every cut, with no throttle.
- The Hermes launch poll (`ClientPhaseInput::HermesLaunchPoll
  { screen_waiting }`) still takes `screen_waiting` from the webview's own
  read of its widget. It is not daemon-authored in frames mode. Moving it
  is open for T-4 or the flip.
- A client `screenReading` input is dropped for every kind while the PTY
  is live in frames mode, including a `null` withdrawal.
- `TerminalMode::force_on_this_thread` is a test-only override
  (`pty/frames/mode.rs`).

### Where it is

- `crates/skein-daemon/src/harness_phase/screen_reader.rs`:
  `read_hermes`, a port of `app/src/screenPhase.ts`.
- `crates/skein-daemon/src/pty/observer.rs`: `wants_screen`, `screen`,
  `exit_notice`. `pty/frames/hub.rs`: `screen_text`, `modes`,
  `append_notice`, and the exit order. `pty/mod.rs`:
  `PtyManager::screen_text`, `modes`, `append_notice_bound`.
- `crates/skein-daemon/src/harness_runner/phase_tap.rs`
  (`RunnerObserver`), `notices.rs` and `client_input.rs`.
- `crates/skein-daemon/src/prompt_delivery/daemon_env.rs` and
  `runner_pty.rs`: `PtyPort::bracketed_paste` and `gate_facts`.
- `app/src/terminalNotices.ts`, used by `app/src/useTerminalSpawn.ts`.
- Tests: `cargo test -p skein-daemon frames`, and
  `pty/frames/hub_screen_tests.rs`, `hub_notice_tests.rs`,
  `pty/frames_notice_pty_tests.rs`.

## Slices

These replace slices 1 and 2 of the P2b doc and add four. The other
slices keep their numbers there.

| Slice | Issue | Content |
| --- | --- | --- |
| T-1 | #793, rescoped | Engine trait, `alacritty_terminal` implementation, `catch_unwind`, the fixtures and gate harness from the #793 room. The `vt100` implementation is not landed |
| T-2 | new | Redraw encoder: grid to VT-subset frames, full and diff, wrap, history push, mode mirrors. Pure and table-tested, plus the gate's second version |
| T-3 | #760, rescoped | PTY subscribers, frame scheduler, per-viewer state, attach as a full redraw, one size owner, engine query replies. Behind the flag |
| T-4 | new | The view in frame mode: no local writes, no repaint hack, grid size from the daemon for a non-owner, redraw request. Only when the spawn result says frames |
| T-5 | new | The daemon reads its own screen: Hermes screen phase in Rust, paste mode from the engine, daemon-authored pane notices. Behind the flag |
| T-6 | new | The flip: frames by default; delete the raw path, the flag, the cursor-query scanner, `terminalRepaint.ts`, `ptyInputBuffer.ts`, the `setScreenReading` input, `screenPhase.ts`, `screenPhaseTick.ts` and `paste_mode.rs` |

Order: T-1, then T-2, then T-3, then T-5, then T-4. T-6 follows T-4
and the dogfood pass. T-5 comes before T-4 because T-4 forbids local
writes into the widget, and the pane notices are written locally until
T-5 makes the daemon author them.

```mermaid
flowchart TD
    T1["T-1 Engine and gate harness"]
    T2["T-2 Redraw encoder"]
    T3["T-3 Fan-out and frames, flagged"]
    T4["T-4 View in frame mode"]
    T5["T-5 Daemon reads its screen"]
    T6["T-6 Flip, delete raw path"]
    S3["3 Daemon start spawns open rooms"]
    S7["7 Event stream and PTY channels"]
    S11["11 Cutover"]
    T1 --> T2
    T2 --> T3
    T3 --> T5
    T5 --> T4
    T4 --> T6
    T3 --> S3
    T6 --> S7
    T6 --> S11
```

Figure 4: the terminal slices and where they meet the P2b slices.
Renders section "Slices"; the text is authoritative (non-normative).

### Proposed issue changes

Nothing has been edited on GitHub. These are for the user to approve.

| Issue | Change |
| --- | --- |
| #793 (P2b-1) | Rescope and retitle: "engine trait, `alacritty_terminal` implementation and the replay gate". Keeps the trait, fixtures, gate harness and `catch_unwind`; drops the `vt100` implementation and the snapshot-only trait |
| #760 (P2b-2) | Rescope and retitle: "PTY subscribers, per-viewer frames, attach as a full redraw, one size owner". Drops "then live bytes"; adds the scheduler and engine query replies; lands behind the flag |
| #759 (P2b-3) | Scope unchanged. It still follows the fan-out slice (now T-3), and the last known size on the record becomes the engine's first size |
| #796 (P2b-7) | Small change: the PTY channel carries frames, plus the grid size and the redraw request. Adds a dependency on T-6 |
| #800 (P2b-11) | Scope unchanged. Adds a dependency on T-6 |
| #801 (P2b-12) | Adds this doc, CLAUDE.md's PTY and Data flow text, and the stale terminal wording in docs/skeind-port-review.md and docs/skeind-p2-design.md to its list |
| #794, #795, #797, #798, #799 | No change |
| new, T-2 | "skeind P2b: redraw encoder, grid to VT-subset frames" |
| new, T-4 | "skeind P2b: the terminal view in frame mode" |
| new, T-5 | "skeind P2b: the daemon reads its own screen (Hermes phase, paste mode, pane notices)" |
| new, T-6 | "skeind P2b: flip to frames and delete the raw terminal path" |

No issue closes.

## Proposed decisions not put to the user

Each is Proposed and can be reopened.

- Local scrollback filled by the daemon, with a full resend on a width
  change, over daemon-paged scrollback.
- Wrap flags kept in the client by writing through the margin, subject
  to the gate.
- A viewer that is not the size owner shows the grid at the owner's
  size.
- The flag is an environment variable read at daemon start.
- The flip lands before the socket PTY channel and the cutover.
- The idle heuristic and the waiting patterns keep reading raw output
  for now.
- OSC 52 (clipboard writes from a harness) stays ignored, as today.

## Risks

- **The engine has not been run.** Every `alacritty_terminal` fact here
  is read, not measured. Mitigation: T-1 is the gate and nothing
  depends on the engine before it is green; fallbacks are named in T3.
  Now measured, see "T-1 gate results"; what remains is listed there.
- **Engine gaps are always visible.** There is no client engine to
  cover for the daemon's. Mitigation: the gate compares against what
  xterm.js showed from the raw stream, and the flip waits for it.
- **Character width differs between daemon and client.** Mitigation:
  the cursor is re-positioned after wide and ambiguous clusters; the
  fixtures hold CJK and emoji.
- **Wrap flags through the margin are fragile.** Mitigation: the gate
  compares them; the fallback loses joined copy and cross-row path
  links, nothing else.
- **Keystroke echo latency.** Parsing and encoding are added; the 16 ms
  window already exists. Synchronized updates can hold a frame up to
  150 ms, by the harness's own request. Mitigation: measure before the
  flip.
- **CPU and memory with many harnesses.** One grid and 5000 history
  rows per PTY, plus a last-sent screen per viewer. Mitigation: an idle
  PTY costs nothing; measure at the dogfood pass.
- **Two terminal paths until the flip.** Mitigation: the old path is
  untouched and default; the new one is reached only through the flag;
  T-6 deletes one of them.
- **Size flips with two active viewers.** Two viewers of different
  sizes typing in turn resize the PTY each time, and a width change is
  a full redraw for both. Mitigation: debounce, and ownership moves
  only on focus and input; revisit if it shows in use.
- **An exited pane keeps its engine.** The grid and history stay in
  memory until the harness record is removed. Mitigation: bounded at
  5000 rows per PTY; measure at the dogfood pass.
- **Unused dependencies.** `alacritty_terminal` brings PTY and
  event-loop crates. Mitigation: none needed beyond noting it in
  `docs/skeind-crates.md`.
- **The recorded fixture is not yet approved for a public repo.**
  Mitigation: the user reviews it before T-1 lands, or T-1 lands with
  the synthetic fixture only. Approved 2026-11-18 after an extra scrub,
  see "T-1 gate results".

## Sources

- The #793 room's handover, 2026-11-18: mismatch list, engine notes,
  fixtures and gate design.
- Engine research, 2026-11-18: crates.io metadata for `vt100`,
  `alacritty_terminal`, `termwiz` and `libghostty-vt`; the `vt100` and
  Alacritty changelogs; the wezterm `term` manifest; the
  `libghostty-rs` README.
- Not verified: the tmux and mosh mechanics are from general knowledge.
  xterm.js's handling of synchronized-update brackets was checked in T-2:
  `@xterm/headless` 6.0.0 only records `?2026` and holds nothing back
  (see "T-2 gate results").
