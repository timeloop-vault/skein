# Epic #50 L2c — Harness-native event adapters

This is the L2c slice of epic
[#50](https://github.com/timeloop-vault/skein/issues/50): per-harness
native event sources that give a clean `running` / `waiting-on-input`
signal instead of pattern-matching the PTY output.

v0.1.8 shipped L1 + L2a + L3 + L4 + L5(a-e) — the notification
substrate works, but `waiting` is never written today because no
strategy reports it. L2c-1 (Claude) and L2c-2 (opencode) plug the
gap with authoritative per-harness signals. L2b becomes a small
generic fallback afterwards for harness kinds we don't have an
adapter for (`byoh`, `copilot`, shell).

## Why L2c-first instead of L2b-first

Originally the epic ordered the layers L2b → L2c. The case for
flipping it:

- **Claude Code is the harness that matters most** for "is it
  waiting on me?", and it already emits structured events on disk.
  Pattern-matching its TUI when the truth source is one JSONL file
  away is doing the harder thing for a worse answer.
- **Patterns rot.** Claude's prompt strings shift between releases;
  every shift is a silent regression that we'd only catch by
  someone noticing the dot stayed green.
- **Re-paint noise.** Claude redraws its full UI on every keystroke
  and every focus event. Reasoning about tail-of-buffer against a
  stream that's constantly re-rendering is fiddly in a way that
  L2c sidesteps.
- **The pattern fallback is only really needed for harnesses
  without a native source** — byoh / copilot / shell. After L2c-1
  and L2c-2 land, the L2b regex set is ~5 generic patterns
  (`(y/n)`, `[Y/n]`, `Press Enter`, `Password:`, etc.) covering
  exactly those kinds. Per-kind pattern tables go away entirely.

## Architecture (shared by L2c-1 and L2c-2)

**Separation of signal (Rust) and policy (JS).** Rust adapters
read the native event source and emit a semantic event enum over a
Tauri Channel. The frontend keeps the state-machine policy in
`harnessActivity.ts` and translates events to phase calls.

```
       native event source (JSONL / sqlite / HTTP)
                         │
                         ▼
   Rust adapter (per kind) — tailing / polling / subscribing
                         │
                         │  emits HarnessEvent over per-harness Channel
                         ▼
   Frontend translator (harnessEvents.ts)
                         │
                         │  translates event → setPhase("running" | "waiting")
                         ▼
       harnessActivity store (existing, becomes adapter-aware)
```

The frontend store gains an **"authoritative source attached" flag**
per harness. While set:

- The L2a idle tick *skips* this harness — only the adapter writes
  its phase. (Otherwise the 8s idle heuristic would fight the
  adapter for harnesses with quiet PTYs but active turns.)
- `recordOutput` still updates `lastOutputAt` for diagnostics but
  never flips phase.
- PTY exit still wins (`harnessActivity.exited`) — the file or
  HTTP source may not get a terminal event but the PTY dying is
  final.

If the adapter detaches (file vanishes, session archived), the
flag clears and L2a takes over.

This separation gives L7 (cross-harness activity feed) raw events
to feed off later, instead of having to back-derive them from
phase transitions.

# L2c-1 — Claude Code (JSONL tail)

## Event source

Claude writes a JSONL session log at
`~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`. Skein
pre-allocates `sessionId` at spawn time via the
`--session-id <uuid>` flag (chapter 5 work), so we know the path
without snapshot-and-diff.

Event types observed in a live JSONL (May 2026, Claude 2.1.x):

| `type` field        | Meaning |
|---------------------|---------|
| `assistant`         | One model invocation. Carries `message.stop_reason` — `tool_use` (more to come) or `end_turn` / `stop_sequence` / `max_tokens` (turn over). |
| `user`              | User prompt OR tool result (distinguished by presence of `toolUseResult`) |
| `last-prompt`       | Captures the user's just-submitted prompt for retry/edit. **Fires at the START of a Claude turn, not the end.** Misleading name. |
| `attachment`        | User attached a file / pasted |
| `permission-mode`   | Permission mode metadata |
| `ai-title`          | Auto-generated session title |
| `pr-link`           | PR linked metadata |
| `system`            | Session metadata (durationMs, messageCount, etc.) |

The "waiting on user input" signal is **`message.stop_reason`** on an
`assistant` row: `end_turn` / `stop_sequence` / `max_tokens` are
terminal, `tool_use` means another assistant row will follow when the
tool result returns. Frequency observed across three sessions:
~95% `tool_use`, ~5% `end_turn`, one `stop_sequence` in 100k+ rows.

**First-cut bug** (caught in v0.1.9-dev): treating `last-prompt` as
"awaiting prompt" kept the dot green after Claude finished — the
`last-prompt` row only fires on the next user prompt, so the
transition only happened at the START of the next turn. Fixed by
reading `stop_reason` instead.

## Why file tailing (and the alternative considered)

**The concern with tailing:** it's a slightly involved primitive
(notify watcher + position tracking + partial-line buffering),
and we're reading a file we don't own.

**Mitigations / why it's still right:**

- We *already* read this file in `resume.rs:85`
  (`claude_session_exists`). Tailing is the natural extension.
- The notify infrastructure (`notify-debouncer-mini`) is already
  vendored and used in `watcher.rs`. No new deps.
- Claude flushes JSONL after every event (verified by watching
  events appear in real time during recon). No buffering surprises.
- The state machine itself is small: a `BufReader` over a file
  handle, a `last_pos: u64`, and a string buffer for the trailing
  partial line.

**Alternative considered and rejected:** spawning `tail -f` as a
subprocess. Adds a Unix-only dep, an extra process per Claude
harness, and a child to clean up. The native approach is cleaner
and cross-platform (Windows JSONL paths work the same way through
notify).

**Alternative considered and deferred:** polling read every N ms.
Dead simple, but wakes up the CPU constantly across many idle
harnesses. Worth keeping as a fallback if notify is ever flaky on
a specific platform.

## Backend (Rust)

### New module: `app/src-tauri/src/harness_events_claude.rs`

Event enum exposed to the frontend:

```rust
#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaudeEvent {
    AssistantTurn { timestamp: i64 },     // one per turn boundary
    ToolUseStart  { name: String },       // for activity-feed v1
    ToolUseResult,
    UserPrompt    { timestamp: i64 },
    AwaitingPrompt { timestamp: i64 },    // from `last-prompt`
    Attachment,
    SessionEnd,                            // file deleted / archived
}
```

Adapter responsibilities:

1. **Path resolution.** Encode the cwd the same way `resume.rs`
   does (`/` → `-`) and join with `<sessionId>.jsonl`.
2. **Attach.** If file exists, open it and `seek(SeekFrom::End(0))`,
   record `last_pos`. If not, watch the parent directory for
   create, then attach when it appears.
3. **Tail loop.** On each notify event:
   - Open file (or reuse handle), seek to `last_pos`, read to EOF.
   - Update `last_pos`.
   - Split read on `\n`; hold any trailing partial-line in a buffer
     for the next tick.
   - Parse each complete line as `serde_json::Value`. Drop unknown
     `type` values silently.
4. **Event reduction.** A turn may span many `assistant` rows.
   Emit one `AssistantTurn` event at the *start* of a sequence of
   `assistant` rows (i.e. when the previous event was not
   `assistant`). Subsequent `assistant` rows in the same turn are
   coalesced — no need to re-flip phase to `running` every chunk.
5. **Tear down.** When the parent harness id is dropped (PTY
   killed), drop the watcher.

### New Tauri commands

```rust
#[tauri::command]
async fn claude_events_attach(
    harness_id: String,
    session_id: String,
    cwd: String,
    on_event: tauri::ipc::Channel<ClaudeEvent>,
) -> Result<(), String>;

#[tauri::command]
async fn claude_events_detach(harness_id: String);
```

Mirrors `pty_spawn` / `pty_kill` in shape and lifecycle.

### Watcher integration

Add a sibling manager next to `WatcherManager`. The worktree
watcher's 200 ms debounce is right for git-status; JSONL tailing
wants ~50 ms because notification UX cares about latency. Separate
manager keeps the debounce concerns from cross-pollinating.

## Frontend (TS)

### New module: `app/src/harnessEvents.ts`

```ts
export function attachClaudeEvents(
  harnessId: string,
  sessionId: string,
  cwd: string,
): () => void;
```

- Creates a Tauri `Channel<ClaudeEvent>` and invokes
  `claude_events_attach`.
- Calls `harnessActivity.attachAuthoritativeSource(harnessId)` so
  the L2a idle tick stands down for this harness.
- Translates each event to a phase call:
  - `AssistantTurn` → `setRunningFromAdapter`
  - `AwaitingPrompt` → `setWaitingFromAdapter`
  - `ToolUseStart` → `setRunningFromAdapter` (tool use means
    Claude is still on a turn)
  - `SessionEnd` → call `detachAuthoritativeSource` and unsub
- Returns an unsubscribe used by `LiveTerminal`'s cleanup.

### `harnessActivity.ts` API additions

```ts
attachAuthoritativeSource(id: string): void;
detachAuthoritativeSource(id: string): void;
setRunningFromAdapter(id: string): void;  // bypasses chunk throttle
setWaitingFromAdapter(id: string): void;
```

While `authoritative` is set on a harness, the tick (line 99-111)
skips it. PTY exit still calls `exited()` regardless.

### Wire-up site

`LiveTerminal.tsx`, in `startPty` after `pty_spawn` resolves
successfully and `sessionId` is known:

```ts
if (harnessKind === "claude" && sessionId) {
  detachAdapter = attachClaudeEvents(harnessId, sessionId, cwd);
}
```

Unsubscribed in the same `return ()` cleanup that runs `pty_kill`.
`harnessKind` and `sessionId` already flow through `LiveTerminal`
props (chapter 5 work).

## Permission-prompt handling

**Recon needed (not blocking v1):** when Claude's TUI shows the
"Allow this tool call?" prompt, what appears in the JSONL?

**Hypothesis:** an `assistant` event with `tool_use` content is
written, then nothing else until the user answers — at which
point a `user` event with `toolUseResult` is appended.

If that holds:

- Track outstanding `tool_use` blocks per harness.
- When the most recent assistant ends with un-resulted `tool_use`s
  AND no new event for 500 ms → emit synthetic
  `AwaitingPermission` → policy maps to `waiting`.

If it doesn't (Claude writes a distinct "prompt-displayed" marker),
use that directly. Either way the plumbing is in place from L2c-1
v1; permission-prompt distinction ships as a v1.1 patch.

## Edge cases

| Concern | Handling |
|---|---|
| File doesn't exist yet on first spawn | Watch parent dir for create; attach when JSONL appears. |
| Sub-agent JSONLs (`<id>/subagents/agent-*.jsonl`) | Ignore. Main session JSONL is the only one we track. |
| File renamed/rotated by Claude | Not observed in recon; treat as detach if it happens. |
| Partial-line writes (Claude flushes mid-line) | Tail reader buffers until `\n`. |
| Stale events on attach (file pre-existing on `--resume`) | Seek to EOF on attach. We don't replay history — only new events. |
| Two harnesses sharing a sessionId (e.g. user manually `--resume`s) | sessionId is the file key. Two attachments on the same file just both see the same events. Fine. |
| JSON parse error on a line | Drop the line, log at `trace`, continue. |

## Tests

Same pattern as `crates/skein-git/tests/`: tempdir, write fake
JSONL files line by line, assert the adapter emits the expected
events. Lives in `harness_events_claude.rs` as
`#[cfg(test)] mod tests` since it's not a public crate.

Cases:

- Single `last-prompt` line → `AwaitingPrompt`.
- Stream of `assistant` lines then `last-prompt` → `AssistantTurn`
  then `AwaitingPrompt`.
- Partial line write then flush → no event mis-parse.
- File doesn't exist at attach time → adapter waits for create.
- JSON parse error on a line → that line skipped, others still emit.

## Shape of the PR

Single PR, purely additive for Claude harnesses (other kinds
unaffected):

- `app/src-tauri/src/harness_events_claude.rs` — new, ~250 LOC incl. tests.
- `app/src-tauri/src/lib.rs` — register two Tauri commands, ~15 LOC.
- `app/src/harnessEvents.ts` — new, ~80 LOC.
- `app/src/harnessActivity.ts` — adapter API + authoritative flag, ~40 LOC.
- `app/src/LiveTerminal.tsx` — attach/detach on spawn, ~10 LOC.

PR title: `Epic #50 L2c-1: Claude Code event-stream adapter (waiting-on-input via JSONL)`.

# L2c-2 — opencode (sketch)

Lower-confidence than L2c-1 — needs a recon spike first.

## Two candidate sources

### A. HTTP event server

The epic mentions opencode exposes events over HTTP. If it's an
SSE/websocket stream of session events, it's the cleanest source
— push-based, no file tailing.

Recon questions:

- Is the HTTP server always running when opencode is, or only on
  demand?
- Port — fixed, per-session, or discoverable?
- Event schema?

### B. sqlite poll

`opencode.db` (documented in `docs/chapter-5-recon.md:87`) has
`session.time_updated`. Event detail likely lives in a `message`
table.

Recon questions:

- Schema of the message table (column for type/role, timestamp).
- Can we file-watch the sqlite WAL/journal and trigger reads on
  change? (sqlite's WAL touches mtime on commit; probably yes.)

## Architecture mirror

The architecture is **identical** to L2c-1. Only the data source
changes:

- Rust adapter emits `OpencodeEvent` enum over a Channel.
- Frontend `harnessEvents.ts` gains an opencode branch in the
  translator.
- `harnessActivity.ts` API is already kind-agnostic from L2c-1.

That's the dividend of getting L2c-1's architecture right — L2c-2
inherits the plumbing and is mostly a new adapter module.

## Recon spike first

Before writing L2c-2 in implementation detail:

1. Probe opencode's HTTP server. Present? Port? Schema?
2. If no HTTP, probe sqlite — what tables, what columns indicate
   "assistant turn" vs "awaiting user prompt"?
3. Does opencode signal permission prompts distinctly?

Output: a `docs/chapter-X-recon-opencode.md` (analogous to
`chapter-5-recon.md`). Then a tight L2c-2 plan + PR.

## Shape of the eventual PR

- HTTP source: ~200 LOC new adapter + ~5 LOC translator branch.
- sqlite poll: ~300 LOC (file watcher + sqlite query layer).

PR title (eventual): `Epic #50 L2c-2: opencode event-stream adapter`.

# Suggested order

1. **L2c-1 ship.** Self-contained, immediately useful (Claude is
   the heaviest-used harness), lands the adapter architecture.
2. **L2c-2 recon.** Half-day spike on the HTTP-vs-sqlite question,
   lands as a recon doc.
3. **L2c-2 ship.** Adapter only, reusing L2c-1's plumbing.
4. **L2b ship — much smaller now.** Just the generic `(y/n)` /
   `[Y/n]` / `Press Enter` / `Password:` regex set, applied to
   `byoh` / `copilot` / shell harnesses where no native adapter
   exists. Per-kind tables go away.
5. **L6 / L7** as originally scheduled.
