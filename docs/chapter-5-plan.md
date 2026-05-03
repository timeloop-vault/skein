# Chapter 5 — Transparent harness resume

Chapter 2 phase 5a wired naive resume on Skein restart:

- Claude → `claude --resume` (Claude shows a picker; user picks the
  right conversation each restart).
- opencode → `opencode --continue` (resumes most recent in cwd —
  picks the wrong one if you've ever had two opencode sessions in
  the same folder).
- gh copilot → no resume; fresh.
- Shell → fresh.

Phase 5b was deferred to backlog with a note: "capture the actual
session id (Claude writes to `~/.claude/sessions/`, opencode similar)
and resume directly to *this* harness's conversation — no picker."

Chapter 5 closes that loose end. The output isn't a new feature — it
removes the picker friction every Skein restart and stops opencode
from silently picking a different conversation than the one that was
running before.

A pre-chapter recon (this conversation, 2026-05-03) verified that
Claude and opencode both store enough on disk for transparent resume:

- Claude session files live at
  `~/.claude/projects/<path-encoded-cwd>/<uuid>.jsonl`. Path encoding
  replaces `/` with `-` and prepends a leading `-` (e.g. `/Users/x/y`
  → `-Users-x-y`). The filename UUID is the session id;
  `claude --resume <uuid>` is the documented way to resume it.
- opencode stores sessions in `~/.local/share/opencode/opencode.db`
  (sqlite, drizzle-managed). The `session` table has columns `id`,
  `directory`, `title`, `slug`, etc. `directory` matches our cwd.

Phase 1 confirms the *CLI argv shape* (does `claude --resume <id>`
actually attach? what's opencode's flag for "resume by id"?) and
captures the rest of the assumptions in writing before phase 2 builds
on them.

## Phase 1 — Reconnaissance

**Goal:** every assumption phase 2 makes about Claude / opencode is
verified end-to-end on this machine, and the findings are written
down so chapter 6 / 7 don't have to re-derive them.

- Verify `claude --resume <uuid>` against a fresh JSONL file under
  `~/.claude/projects/<encoded-cwd>/`: does it attach to that exact
  conversation, or to the most-recent regardless? What's the exit
  behaviour when the id no longer exists?
- Verify `opencode --session <id>` (or whatever the actual flag is)
  the same way against a row in `opencode.db`'s `session` table.
- Document the path-encoding scheme for non-trivial cwds — spaces,
  accents, symlinks (does Claude resolve before encoding?).
- Capture a small JSONL example showing the first few lines of a
  Claude session file, so phase 4's "is this session still alive?"
  check has a parsing target.
- Output: `docs/chapter-5-recon.md` — short, factual, ~one page.
  Phase 2-4 cite it.

**No code in phase 1.** It's deliberately a spike: cheap, kills
unknowns, the alternative is debugging guesses through phase 2.

## Phase 2 — Capture session ids on first spawn

Phase 1's recon revealed Claude has `--session-id <uuid>` for
*pre-allocating* a session id at spawn time, but opencode has no
equivalent. The two paths are different enough to split into 2a / 2b
— recon §5 / §7 has the details. Both phases share the new
`sessionId?: string` field on `Harness`; it round-trips through the
existing `db_save_sessions` JSON-blob path (phase 5a's stored `cmd`
already proves arbitrary fields persist that way).

### Phase 2a — Claude pre-allocate

**Goal:** every Claude harness gets a Skein-picked UUID at spawn
time, stored immediately on the harness.

- Generate a fresh UUID v4 (browser `crypto.randomUUID()`) when a
  Claude harness is created.
- `cmdForKind("claude", …)` becomes
  `["claude", "--session-id", <uuid>]`. Existing call sites (new
  session dialog, harness picker) flow the uuid into the harness
  record.
- `harness.sessionId = uuid` set in the same React state update —
  no follow-up effect, no async wait, no race.
- gh copilot and shell paths unchanged.

### Phase 2b — opencode capture

**Goal:** when an opencode harness spawns, Skein figures out which
session id opencode just created and stores it.

- New Tauri command `opencode_list_sessions(cwd) -> Vec<String>`:
  rusqlite query against `~/.local/share/opencode/opencode.db`,
  `SELECT id FROM session WHERE directory = ? AND time_archived IS NULL
  ORDER BY time_created DESC`. Same crate / pattern as Skein's
  existing `db.rs`.
- Capture flow on opencode spawn:
  1. Before spawn, snapshot the current set of session ids for this
     cwd via the list command.
  2. Spawn the harness as today.
  3. After the PTY emits its first chunk of output (proxy for "the
     binary is up and has written its session row"), poll the list
     every ~250 ms for up to ~5 s.
  4. The first new id that wasn't in the snapshot is this harness's
     session id. Store it on the harness; fire the existing
     auto-save effect to persist.
  5. Timeout with no new id → leave `sessionId` undefined; phase 3
     falls back to phase-5a's `--continue`.
- Snapshot-then-diff handles the case where the user already had
  conversations in this cwd from outside Skein. "Most recent row"
  is not robust against pre-existing rows; the diff is.
- Out of scope: real-time file / db watcher. 5 s of polling after
  spawn is enough; we don't need a watcher's lifetime cost.

## Phase 3 — Targeted resume on restart

**Goal:** `resumeCmd` passes the captured id directly so the harness
re-attaches to *its* conversation with no picker.

- `resumeCmd` becomes session-id-aware:
  - Claude with `sessionId`: `["claude", "--resume", id]`.
  - Claude without: `["claude", "--resume"]` (existing fallback —
    picker; should be rare after 2a since fresh harnesses always
    pre-allocate).
  - opencode with `sessionId`: `["opencode", "--session", id]`
    (verified in recon §2).
  - opencode without: `["opencode", "--continue"]` (existing
    fallback — most-recent-in-cwd).
- The signature changes from `(kind, cmd)` to `(harness)` so it can
  read `sessionId`. Minor refactor — single call site in the boot
  effect.
- gh copilot and shell (byoh) unchanged.

## Phase 4 — Stale-session recovery

**Goal:** if a stored `sessionId` points to a session that no longer
exists (Claude pruned it, opencode db got rebuilt, user wiped
`~/.claude/projects/`), Skein falls back to picker / fresh and
re-captures next spawn instead of erroring.

- Two new Tauri commands:
  - `claude_session_exists(cwd, id) -> bool` — file presence check
    at `~/.claude/projects/<encoded-cwd>/<id>.jsonl`. Phase 2a means
    we know the exact filename (we picked the UUID), so the path-
    encoding edge cases from recon §3 don't bite us.
  - `opencode_session_exists(id) -> bool` — sqlite point lookup
    against the same db as phase 2b's lister.
- On boot, before applying phase 3's targeted resume, verify the
  stored id exists. If not, drop it from the harness and fall through
  to phase-5a's flag-only resume. Phase 2's capture flow then re-runs
  on the next spawn.
- This is the only error-handling phase in the chapter and it's
  deliberately small — the alternative is propagating "session
  vanished" errors all the way up to a UI surface that doesn't exist
  yet.

## Out of scope for chapter 5

See [`backlog.md`](./backlog.md) — anything we considered but pushed
out has been merged there. Notably:

- **Real-time invalidation of in-flight session ids.** If the user
  deletes a session file *while Skein is running*, our in-memory
  `sessionId` goes stale. Detect at next spawn via phase 4; don't
  bother watching during the session.
- **gh copilot resume.** It has no resume mode. Skipped.
- **Cross-tool session migration** ("port this Claude conversation
  to opencode") — way out of scope.
- **Distribution / packaging.** Chapter 7's job.
- **Terminal embedding rework.** Chapter 6's job — chapter 5's
  resume work is independent of the PTY layer (it operates on argv
  before spawn and on stored state, not on the live terminal).
- **Windows / Linux verification.** Claude and opencode store files
  under different roots there. Chapter 7 (distribution) likely
  revisits this; for now chapter 5 targets macOS.
