//! Claude Code event-stream adapter — epic #50 L2c-1.
//!
//! Claude writes every session event to a JSONL log at
//! `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`. Skein
//! pre-allocates `sessionId` via the `--session-id <uuid>` flag
//! (chapter 5), so we know the path without any snapshot-and-diff.
//!
//! This module tails that file and emits a small `ClaudeEvent` enum
//! the frontend translates into harness-activity phase transitions.
//! The authoritative "Claude is awaiting user input" signal is
//! `message.stop_reason` on an `assistant` row — `end_turn`,
//! `stop_sequence`, or `max_tokens` mean Claude is done and the next
//! event will be a user prompt. `tool_use` means there's more to
//! come. Reading this off the JSONL is shorter and sharper than the
//! L2b pattern-matching strategy and survives Claude TUI text
//! changes.
//!
//! Lifecycle: `ClaudeEventsManager::attach` starts watching for the
//! given (harnessId, sessionId, cwd). `detach` (or dropping the
//! manager) stops the watcher. The adapter is purely additive — if it
//! fails to attach, the harness falls back to the L2a idle heuristic
//! and nothing user-visible breaks.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_mini::notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer, notify::RecursiveMode};
use parking_lot::Mutex;
use serde::Serialize;

use crate::db::Database;
use crate::harness_actions_claude::ActionExtractor;

/// Tighter than the worktree watcher's 200 ms — notification UX cares
/// about latency, and a JSONL append produces exactly one event we
/// want to react to quickly.
const DEBOUNCE_MS: u64 = 50;

/// Semantic events emitted to the frontend. The translator in
/// `harnessEvents.ts` maps these to phase calls. We deliberately keep
/// this slightly *richer* than what the state machine needs today —
/// L7 (cross-harness activity feed) will want `ToolUseStart` /
/// `ToolUseResult` for "h1b just used the Edit tool" lines.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaudeEvent {
    /// Start of a new assistant turn. We coalesce the many `assistant`
    /// rows that make up a single turn (one per streamed chunk) into a
    /// single start event — the policy layer doesn't need to see every
    /// chunk to know Claude is `running`.
    AssistantTurn,
    /// Claude initiated a tool call. Carries the tool name so the
    /// activity feed (L7) can display it.
    ToolUseStart { name: String },
    /// Tool finished and the result was appended to the session.
    ToolUseResult,
    /// User-authored message arrived (typed prompt, not a tool result).
    UserPrompt,
    /// Claude finished its turn and is awaiting the next user prompt.
    /// This is the "waiting on input" signal.
    AwaitingPrompt,
    /// User attached a file or pasted content.
    Attachment,
    /// Session log was deleted or otherwise vanished — fall back to
    /// L2a heuristics.
    SessionEnd,
}

#[derive(Debug)]
pub struct ClaudeEventsError(pub String);

impl std::fmt::Display for ClaudeEventsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClaudeEventsError {}

impl ClaudeEventsError {
    fn from_err<E: std::fmt::Display>(e: E) -> Self {
        Self(e.to_string())
    }
}

/// Claude's path-encoding scheme: every `/` in the cwd becomes `-`.
/// Documented and verified against 17 real project dirs in
/// `docs/chapter-5-recon.md` §3. We use the encoded path to compute
/// the JSONL file location directly — chapter 5 also pre-allocates
/// the session uuid, so we never have to scan project dirs at
/// attach time.
fn encode_cwd(cwd: &str) -> String {
    cwd.replace('/', "-")
}

/// `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`. Returns
/// `None` when `HOME` isn't set (exotic environments) so the caller
/// can no-op cleanly.
fn session_jsonl_path(cwd: &str, session_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".claude")
            .join("projects")
            .join(encode_cwd(cwd))
            .join(format!("{session_id}.jsonl")),
    )
}

/// Per-harness adapter handle. Only role is to keep the debouncer
/// alive — dropping it stops the watcher (which in turn drops the
/// closure that holds the shared `TailState` Arc, so all per-harness
/// state goes with it).
struct Adapter {
    _debouncer: Debouncer<RecommendedWatcher>,
}

/// Mutable state shared with the watcher callback. The callback runs
/// on the debouncer thread — every field it touches lives in here.
struct TailState {
    path: PathBuf,
    /// Byte offset into the file we've already consumed. Bumped on
    /// every read so the next tick only sees fresh bytes.
    last_pos: u64,
    /// Trailing partial line carried across ticks — Claude flushes
    /// after each event but the OS may still split a write across
    /// what `notify` reports as separate events.
    partial: String,
    /// Have we observed `attached` yet? If false, the file didn't
    /// exist when `attach` was called and we're waiting for create.
    attached: bool,
    /// Tracks whether the previous emitted event was inside an
    /// assistant turn — used to coalesce streamed `assistant` rows
    /// into one `AssistantTurn` event per turn boundary.
    in_assistant_turn: bool,
    /// Action persistence sink. `None` for path-injected tests that
    /// only care about phase events. Populated in production by
    /// `attach_at_with_actions`. Lives in `TailState` so the watcher
    /// callback can both extract and persist on each tick. Issue #80.
    actions: Option<ActionPersistence>,
}

/// Action-extraction context bundled per attached harness. The
/// `extractor` holds the pending-tool-use buffer between rows; `db`
/// is the persistence sink; `harness_id`/`room_id` are stamped on
/// every row before insert.
struct ActionPersistence {
    extractor: ActionExtractor,
    db: Arc<Database>,
    harness_id: String,
    room_id: String,
}

/// Manager — registry of live Claude adapters keyed by harness id.
/// Mirrors the shape of `PtyManager` / `WatcherManager`.
pub struct ClaudeEventsManager {
    inner: Mutex<HashMap<String, Adapter>>,
    /// Shared with the per-attach persistence sink so each adapter
    /// can write `harness_actions` rows directly from its tick
    /// thread. Issue #80.
    db: Arc<Database>,
}

impl ClaudeEventsManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            db,
        }
    }

    /// Start tailing the JSONL for `harness_id`. `on_event` fires on
    /// every parsed event from the debouncer's flush thread. Replaces
    /// any prior adapter for the same harness id (caller-driven
    /// reattach during respawn — fine to be idempotent).
    ///
    /// `room_id` is stamped on every `harness_actions` row this
    /// adapter persists (issue #80). The Live Context cards query
    /// per-room.
    pub fn attach<F>(
        &self,
        harness_id: String,
        room_id: String,
        session_id: &str,
        cwd: &str,
        on_event: F,
    ) -> Result<(), ClaudeEventsError>
    where
        F: Fn(ClaudeEvent) + Send + Sync + 'static,
    {
        let Some(path) = session_jsonl_path(cwd, session_id) else {
            return Err(ClaudeEventsError("claude_events attach: HOME unset".into()));
        };
        let persistence = Some(ActionPersistence {
            extractor: ActionExtractor::new(),
            db: Arc::clone(&self.db),
            harness_id: harness_id.clone(),
            room_id,
        });
        self.attach_at(harness_id, path, on_event, persistence)
    }

    /// Path-injected variant — used by tests to point the adapter at
    /// a tempdir without touching `HOME`. The production path goes
    /// through `attach()` above, which resolves the JSONL path from
    /// `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
    ///
    /// `actions` is the persistence sink; pass `None` from phase-only
    /// tests to skip the `harness_actions` table entirely.
    fn attach_at<F>(
        &self,
        harness_id: String,
        path: PathBuf,
        on_event: F,
        actions: Option<ActionPersistence>,
    ) -> Result<(), ClaudeEventsError>
    where
        F: Fn(ClaudeEvent) + Send + Sync + 'static,
    {
        // Determine starting position. Three cases:
        //   • File doesn't exist (fresh spawn before Claude has
        //     written anything) — start at 0; watcher's create event
        //     will trip the attached=true branch in `tick`.
        //   • File exists (resume after Skein restart) — read it
        //     once to derive the *current* phase, then seek to EOF
        //     for live tailing. Without this probe, Claude harnesses
        //     all show green on restart because the existing
        //     end_turn row was already in the file before we
        //     attached, so we never see it via tail. Bug found in
        //     v0.1.9-dev — see `determine_initial_state` below.
        //   • Error other than NotFound — treat as "fresh" (file
        //     will appear later, or it really doesn't exist).
        // Backfill (issue #80): before seeking to EOF for live tail,
        // extract every action from the file's existing content and
        // persist anything newer than what we've already seen. On
        // first-ever attach for this harness, that's the whole file;
        // on re-attach after Skein restart, we only insert rows newer
        // than the largest persisted timestamp. The phase-side
        // initial-event probe is unchanged; both consume the same
        // file read.
        let mut actions = actions;
        let (last_pos, attached, initial_event) = match fs::read_to_string(&path) {
            Ok(content) => {
                let init = determine_initial_state(&content);
                if let Some(ap) = actions.as_mut() {
                    backfill_actions(ap, &content);
                }
                let len = u64::try_from(content.len()).unwrap_or(u64::MAX);
                (len, true, init)
            }
            Err(_) => (0, false, None),
        };
        // Derive the parent before moving `path` into TailState.
        // Watching the parent (not the file directly) is two-for-one:
        // (1) attaching pre-create still notices the create event.
        // (2) some platforms (Linux/inotify) lose the watch when the
        //     file is replaced atomically — watching the parent
        //     survives that.
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ClaudeEventsError("session path has no parent".into()))?;
        let state = Arc::new(Mutex::new(TailState {
            path,
            last_pos,
            partial: String::new(),
            attached,
            in_assistant_turn: false,
            actions,
        }));
        let cb_state = Arc::clone(&state);
        let on_event = Arc::new(on_event);
        let cb_on_event = Arc::clone(&on_event);
        // Create the parent dir if it doesn't exist yet. Claude
        // creates project dirs lazily on first spawn for that cwd;
        // if Skein attaches before Claude has written anything, the
        // dir may not be there yet. notify refuses to watch a
        // missing path, so create it ourselves (it's harmless if
        // Claude does the same later).
        if !parent.exists() {
            fs::create_dir_all(&parent).map_err(ClaudeEventsError::from_err)?;
        }
        // notify needs a reference. The PathBuf is dropped at the
        // end of this scope; the watcher captures the path internally.
        let parent_ref: &Path = &parent;

        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| {
                // notify can deliver an Err when the queue overflows
                // (rare, but possible during heavy filesystem activity).
                // Treat that exactly like a normal tick — we'll read
                // up to the current EOF and catch up. Better
                // stale-but-honest than silently miss events.
                let _ = result;
                tick(&cb_state, cb_on_event.as_ref());
            },
        )
        .map_err(ClaudeEventsError::from_err)?;
        debouncer
            .watcher()
            .watch(parent_ref, RecursiveMode::NonRecursive)
            .map_err(ClaudeEventsError::from_err)?;

        // Emit the synthetic initial event from the history probe
        // *after* arming the watcher — so if the file grows between
        // probe and arm, the tick that follows picks up the delta
        // (no race window where new events get lost). Emitting
        // before `tick` also means consumers see initial-state
        // first, live events second; that ordering matches their
        // expectation.
        if let Some(event) = initial_event {
            on_event(event);
        }

        // One immediate tick to catch anything written between the
        // read_to_string above and the watcher arming. In the steady
        // case this seeks to last_pos == file length and reads 0
        // bytes — cheap no-op.
        tick(&state, on_event.as_ref());

        // The `state` Arc isn't held in `Adapter` — the closure
        // inside the debouncer holds one clone, and that's enough to
        // keep it alive for the watcher's lifetime. Dropping the
        // debouncer drops the closure drops the Arc.
        drop(state);
        self.inner.lock().insert(
            harness_id,
            Adapter {
                _debouncer: debouncer,
            },
        );
        Ok(())
    }

    /// Stop the adapter for `harness_id`. No-op if unknown.
    pub fn detach(&self, harness_id: &str) {
        self.inner.lock().remove(harness_id);
    }
}

/// One tick of the tail-reader. Reads any bytes appended since
/// `last_pos`, splits into lines, parses each as a Claude event, and
/// emits `ClaudeEvent`s. Called from the debouncer's flush thread.
fn tick(state: &Arc<Mutex<TailState>>, on_event: &(dyn Fn(ClaudeEvent) + Send + Sync)) {
    let mut s = state.lock();
    // If we previously hadn't attached (file didn't exist), check
    // again. The watcher fires for any change in the parent dir, so
    // this is the moment we'd notice the create.
    if !s.attached {
        if s.path.exists() {
            s.attached = true;
            // Start at 0 — file is fresh, all bytes are new.
            s.last_pos = 0;
        } else {
            // Still not there. Maybe the watcher fired for an
            // unrelated file in the project dir; keep waiting.
            return;
        }
    }

    // Open + seek + read-to-end. Rolling a long-lived File handle
    // would be a tiny optimisation but risks holding a stale fd if
    // Claude ever rotates the file. Reopening every tick is robust
    // and the file sizes we're dealing with (kilobytes of JSON per
    // event) are trivial to seek into.
    let Ok(mut file) = fs::File::open(&s.path) else {
        // File vanished — Claude was uninstalled mid-session, or
        // the user wiped ~/.claude. Surface as SessionEnd once and
        // detach from this run (the adapter stays alive in case the
        // file comes back, but we don't keep re-emitting).
        if s.attached {
            s.attached = false;
            drop(s);
            on_event(ClaudeEvent::SessionEnd);
        }
        return;
    };

    // Defensive: file size dropped below last_pos (rotation /
    // truncation). Reset and read from 0 — better to replay than to
    // silently miss events.
    if let Ok(meta) = file.metadata()
        && meta.len() < s.last_pos
    {
        s.last_pos = 0;
        s.partial.clear();
        s.in_assistant_turn = false;
    }

    if file.seek(SeekFrom::Start(s.last_pos)).is_err() {
        return;
    }
    let mut buf = String::new();
    let Ok(bytes) = file.read_to_string(&mut buf) else {
        // UTF-8 decode failed somewhere mid-file. JSONL is ASCII for
        // the keys + UTF-8 content; the only way this fires is if
        // we landed in the middle of a multi-byte char. Bump
        // last_pos by what we did read (zero in this case) and
        // wait for the next tick to pick up a full line. Logged as
        // trace so it's diagnosable without being noisy.
        tracing::trace!(path = %s.path.display(), "claude_events: utf8 mid-line; retrying next tick");
        return;
    };
    let advance: u64 = u64::try_from(bytes).unwrap_or(u64::MAX);
    s.last_pos = s.last_pos.saturating_add(advance);

    // Prepend any leftover partial line from last tick, then split on
    // newlines. The trailing chunk (anything after the last '\n') is
    // partial — carry it forward.
    s.partial.push_str(&buf);
    // Drain the partial buffer locally so we don't borrow `s.partial`
    // while we walk lines (and also so the next tick starts clean).
    let drained = std::mem::take(&mut s.partial);
    let mut lines = drained.split('\n').peekable();
    let mut events = Vec::new();
    let mut in_assistant_turn = s.in_assistant_turn;
    while let Some(line) = lines.next() {
        if lines.peek().is_none() {
            // Final segment — either the last line was incomplete
            // (no trailing newline) and this is the partial to
            // carry forward, or the input ended with '\n' and this
            // is an empty string. Either way, stash it.
            s.partial.push_str(line);
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Parse once, fan out to both consumers: phase (the
        // pre-existing path) and actions (issue #80). Action
        // persistence is best-effort — a single failed insert
        // shouldn't kill the tail loop.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if let Some(event) = parse_value(&value, &mut in_assistant_turn) {
            events.push(event);
        }
        if let Some(ap) = s.actions.as_mut() {
            let extracted = ap.extractor.ingest(&value);
            persist_extracted(ap, extracted);
        }
    }
    s.in_assistant_turn = in_assistant_turn;
    drop(s);

    for event in events {
        on_event(event);
    }
}

/// One-shot historical scan of the JSONL — runs once on attach
/// before the watcher arms. We walk every row in order so the
/// extractor's `tool_use` buffer can join with its result row even if
/// they fall on different lines. Insert only rows whose timestamp
/// is newer than the largest one already persisted for this harness
/// (`max_persisted_ts_ms`) — that's how a re-attach after Skein
/// restart avoids duplicating rows. On first attach the max is 0,
/// so every row goes in.
fn backfill_actions(ap: &mut ActionPersistence, content: &str) {
    let max_ts = max_persisted_ts_ms(&ap.db, &ap.harness_id);
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let extracted = ap.extractor.ingest(&value);
        let fresh: Vec<_> = extracted
            .into_iter()
            .filter(|a| a.timestamp_ms > max_ts)
            .collect();
        persist_extracted(ap, fresh);
    }
}

/// Insert every action in `extracted` into `harness_actions`. Logs
/// at trace on insert failure (sqlite locked, disk full) but doesn't
/// propagate — the tail loop continues. The user's recourse is to
/// check the log; nothing actionable from inside the adapter.
fn persist_extracted(
    ap: &ActionPersistence,
    extracted: Vec<crate::harness_actions_claude::ExtractedAction>,
) {
    for action in extracted {
        if let Err(e) = ap.db.record_harness_action(
            &ap.harness_id,
            &ap.room_id,
            action.timestamp_ms,
            action.kind,
            &action.payload,
            action.source.as_deref(),
        ) {
            tracing::trace!(harness_id = %ap.harness_id, kind = %action.kind, error = %e,
                "claude_events: record_harness_action failed");
        }
    }
}

/// Query the largest `timestamp_ms` already persisted for this
/// harness. Returns 0 when the harness has no rows yet (first
/// attach). Read errors fall back to 0 — re-inserting rows is
/// recoverable, missing the backfill entirely is not.
fn max_persisted_ts_ms(db: &Database, harness_id: &str) -> i64 {
    db.recent_harness_actions_by_harness(harness_id, -1, 1)
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .map_or(0, |r| r.timestamp_ms)
}

/// Scan a JSONL session log (entire content) and return the event
/// that represents the current phase. Used on adapter attach to
/// handle `--resume`: a session that ended with `end_turn` already
/// in the file needs to start in `waiting`, not `running`. Without
/// this, every Claude harness shows green on Skein restart until
/// the user types something — Claude doesn't write any new row on
/// resume, so the watcher never has a transition to observe.
///
/// Returns:
/// - `Some(AwaitingPrompt)` if the last assistant row had a terminal
///   `stop_reason` (`end_turn` / `stop_sequence` / `max_tokens`).
/// - `Some(AssistantTurn)` if the last assistant row was non-terminal
///   (`tool_use`) — session ended mid-turn, `claude --resume` will
///   pick it up; treat as running so the dot doesn't immediately
///   flip blue (the waiting indicator).
/// - `Some(UserPrompt)` / `Some(ToolUseResult)` if the last event
///   was a user/tool row — Claude was in the middle of consuming
///   input; running.
/// - `None` if there's nothing meaningful to derive state from
///   (empty file, only metadata rows). The first PTY chunk that
///   arrives will flip to running via the normal path.
fn determine_initial_state(content: &str) -> Option<ClaudeEvent> {
    let mut last: Option<ClaudeEvent> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if value
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(ty) = value.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };
        match ty {
            "assistant" => {
                let stop_reason = value
                    .get("message")
                    .and_then(|m| m.get("stop_reason"))
                    .and_then(serde_json::Value::as_str);
                if matches!(
                    stop_reason,
                    Some("end_turn" | "stop_sequence" | "max_tokens")
                ) {
                    last = Some(ClaudeEvent::AwaitingPrompt);
                } else {
                    last = Some(ClaudeEvent::AssistantTurn);
                }
            }
            "user" => {
                last = if value.get("toolUseResult").is_some() {
                    Some(ClaudeEvent::ToolUseResult)
                } else {
                    Some(ClaudeEvent::UserPrompt)
                };
            }
            // Metadata rows don't shift phase; skip.
            _ => {}
        }
    }
    last
}

/// Parse one JSONL row into at most one `ClaudeEvent`. Returns `None`
/// for rows we don't surface (metadata, sub-agents, unknown types).
///
/// The authoritative "Claude is done, awaiting user" signal lives on
/// the `assistant` row as `message.stop_reason`. Observed values
/// across recent sessions:
///
/// - `"tool_use"` — Claude wants to call a tool; another assistant
///   row will follow once the tool result returns. Still running.
/// - `"end_turn"` — Claude finished its turn cleanly. Awaiting user.
/// - `"stop_sequence"` — hit a configured stop string. Awaiting user.
/// - `"max_tokens"` — hit the context limit mid-thought. Effectively
///   awaiting user (they need to /clear or continue manually).
///
/// `last-prompt` rows — despite the suggestive name — fire when
/// Claude *captures the user's new prompt* (the leaf uuid is for
/// retry/edit). They appear at the *start* of a Claude turn, not
/// the end. Treating them as "awaiting input" was the bug that
/// kept the dot green until the L2a 8 s idle timeout finally fired.
fn parse_value(value: &serde_json::Value, in_assistant_turn: &mut bool) -> Option<ClaudeEvent> {
    let ty = value.get("type")?.as_str()?;

    // Sub-agent rows carry isSidechain=true. The main session is
    // what reflects the user-facing harness state; sub-agents are
    // their own internal flow and shouldn't drive the dot.
    if value
        .get("isSidechain")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        // Reset the turn flag if a sub-agent interrupts so the next
        // main-session assistant row starts a fresh turn.
        *in_assistant_turn = false;
        return None;
    }

    match ty {
        "assistant" => {
            let stop_reason = value
                .get("message")
                .and_then(|m| m.get("stop_reason"))
                .and_then(serde_json::Value::as_str);
            // Terminal stop reasons → turn is over → emit
            // AwaitingPrompt regardless of mid-turn coalescing
            // state. Non-terminal (tool_use, null, unknown) means
            // more rows will follow — emit AssistantTurn for the
            // first row of the turn, coalesce thereafter.
            let terminal = matches!(
                stop_reason,
                Some("end_turn" | "stop_sequence" | "max_tokens")
            );
            if terminal {
                *in_assistant_turn = false;
                return Some(ClaudeEvent::AwaitingPrompt);
            }

            // Non-terminal row. Look for a tool_use block — useful
            // for the L7 activity feed (which tool, which file).
            // Today the policy just needs "running"; tool_event
            // therefore only matters during a *coalesced* (mid-turn)
            // row, where AssistantTurn has already fired and we'd
            // otherwise emit nothing.
            let mut tool_event = None;
            if let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(serde_json::Value::as_array)
            {
                for block in content {
                    if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                        if let Some(name) = block.get("name").and_then(serde_json::Value::as_str) {
                            tool_event = Some(ClaudeEvent::ToolUseStart {
                                name: name.to_owned(),
                            });
                        }
                    }
                }
            }
            if *in_assistant_turn {
                // Mid-turn — AssistantTurn already fired for this
                // turn. Surface the tool call if there is one;
                // otherwise this row is redundant for the policy.
                tool_event
            } else {
                *in_assistant_turn = true;
                Some(ClaudeEvent::AssistantTurn)
            }
        }
        "user" => {
            *in_assistant_turn = false;
            // `user` rows with `toolUseResult` are tool replies; the
            // ones without are real user-typed prompts. The
            // distinction matters for the activity feed (L7) but
            // both keep the harness in `running` from the state
            // machine's perspective.
            if value.get("toolUseResult").is_some() {
                Some(ClaudeEvent::ToolUseResult)
            } else {
                Some(ClaudeEvent::UserPrompt)
            }
        }
        "attachment" => {
            // No turn-flag reset: an attachment doesn't end an
            // assistant turn (it's a user-side action between turns).
            Some(ClaudeEvent::Attachment)
        }
        // Everything else — `last-prompt` (user-prompt capture, see
        // doc above), `permission-mode`, `ai-title`, `pr-link`,
        // `system`, future row types — doesn't drive the activity
        // dot. Keep `in_assistant_turn` as-is so a metadata row in
        // the middle of a turn doesn't re-open the turn boundary.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    /// In-memory Database for tests that want to construct a
    /// `ClaudeEventsManager` without touching disk. We don't use the
    /// action sink in these phase-focused tests (each `attach_at`
    /// passes `None`); the manager just needs *a* db to hold.
    fn test_manager() -> ClaudeEventsManager {
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
        // Leak the TempDir — only needed for the duration of the
        // test, and not worth a per-test handle.
        std::mem::forget(dir);
        ClaudeEventsManager::new(Arc::new(db))
    }

    /// Drive `attach_at` against a tempdir, returning a receiver the
    /// test collects events from. The manager is returned so the test
    /// can hold it (dropping ends the watch).
    ///
    /// We use the path-injected variant rather than the env-var-driven
    /// `attach()` so tests stay hermetic — `unsafe_code = forbid`
    /// means `std::env::set_var` is off the table in this crate, and
    /// it would also race across the test binary's parallel threads.
    fn make_adapter(dir: &TempDir) -> (ClaudeEventsManager, PathBuf, mpsc::Receiver<ClaudeEvent>) {
        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        let path = dir.path().join("session.jsonl");
        manager
            .attach_at(
                "harness-1".into(),
                path.clone(),
                move |event| {
                    tx.send(event).unwrap();
                },
                None,
            )
            .unwrap();
        (manager, path, rx)
    }

    /// Collect events that arrive within a 2 s window. Returns
    /// whatever we got. The 50 ms debounce gives us sub-100 ms ideal
    /// latency, but macOS `FSEvents` has a ~500 ms-1 s coarse delivery
    /// floor — especially for second-modifications on a file the
    /// kernel just saw activity on. 2 s is the headroom we need so
    /// flakiness doesn't bite on CI.
    fn drain(rx: &mpsc::Receiver<ClaudeEvent>) -> Vec<ClaudeEvent> {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(ev) => out.push(ev),
                Err(_) => break,
            }
        }
        out
    }

    /// Variant for assertions of *absence*: we only want to wait long
    /// enough to be sure nothing fires. 300 ms is comfortable margin
    /// over the debounce without dragging out tests that should fail
    /// fast when an event leaks through.
    fn drain_brief(rx: &mpsc::Receiver<ClaudeEvent>) -> Vec<ClaudeEvent> {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(ev) => out.push(ev),
                Err(_) => break,
            }
        }
        out
    }

    #[test]
    fn awaiting_prompt_emitted_for_assistant_end_turn() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected AwaitingPrompt, got {events:?}"
        );
    }

    #[test]
    fn awaiting_prompt_emitted_for_assistant_stop_sequence() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"stop_sequence","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected AwaitingPrompt, got {events:?}"
        );
    }

    #[test]
    fn last_prompt_row_does_not_emit_awaiting_prompt() {
        // `last-prompt` fires when Claude captures the user's new
        // prompt at the START of a turn — opposite of awaiting.
        // The first L2c-1 cut treated it as "done" which kept the
        // dot green until L2a's 8s idle timer fired.
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"last-prompt","sessionId":"x","lastPrompt":"hi","leafUuid":"u"}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain_brief(&rx);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "last-prompt must not emit AwaitingPrompt, got {events:?}"
        );
    }

    #[test]
    fn streamed_assistant_rows_coalesce_to_one_turn_event() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        // Two assistant rows with non-terminal stop_reason (tool_use)
        // followed by a terminal end_turn row. The first two should
        // coalesce to a single AssistantTurn; the third triggers
        // AwaitingPrompt.
        for _ in 0..2 {
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"text","text":"hi"}}]}}}}"#
            )
            .unwrap();
        }
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        let turn_count = events
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::AssistantTurn))
            .count();
        assert_eq!(turn_count, 1, "should coalesce, got {events:?}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected AwaitingPrompt, got {events:?}"
        );
    }

    #[test]
    fn tool_use_block_emits_tool_use_start() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        // First non-terminal assistant row → AssistantTurn.
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"text","text":"thinking"}}]}}}}"#
        )
        .unwrap();
        // Mid-turn assistant row carrying the actual tool_use block.
        // AssistantTurn is already emitted, so this one surfaces
        // ToolUseStart with the tool name.
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Edit","id":"t1","input":{{}}}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"x","toolUseResult":"ok","message":{{"content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::ToolUseStart { name } if name == "Edit")),
            "expected ToolUseStart(Edit), got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::ToolUseResult)),
            "expected ToolUseResult, got {events:?}"
        );
    }

    #[test]
    fn sidechain_rows_are_ignored() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"sessionId":"x","message":{{"content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"last-prompt","isSidechain":true,"sessionId":"x"}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain_brief(&rx);
        assert!(
            events.is_empty(),
            "sidechain rows should be ignored, got {events:?}"
        );
    }

    #[test]
    fn pre_existing_file_seeks_to_eof_and_only_emits_new_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        {
            let mut f = fs::File::create(&path).unwrap();
            // Historical end_turn — we should NOT replay this.
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at(
                "h1".into(),
                path.clone(),
                move |e| {
                    tx.send(e).unwrap();
                },
                None,
            )
            .unwrap();

        // Append a fresh end_turn row. Scope the file handle so the
        // OS closes it before we drain — macOS FSEvents holds modify
        // events on an open handle until close, even after fsync.
        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }

        let events = drain(&rx);
        let prompt_count = events
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::AwaitingPrompt))
            .count();
        assert_eq!(
            prompt_count, 1,
            "should only see the new end_turn row, got {events:?}"
        );
    }

    #[test]
    fn partial_line_carries_across_ticks() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        // Write the line in two halves with a delay between to force
        // the watcher to fire twice. The adapter must not parse the
        // half-line and must concatenate before parsing. We scope
        // each write so the handle closes between them — macOS
        // FSEvents won't deliver modify events for an open file
        // until close, regardless of fsync.
        {
            let mut f = fs::File::create(&path).unwrap();
            f.write_all(br#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"#)
                .unwrap();
            f.sync_all().unwrap();
        }
        thread::sleep(Duration::from_millis(150));
        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"\"end_turn\",\"content\":[]}}\n").unwrap();
            f.sync_all().unwrap();
        }

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected one AwaitingPrompt after concatenation, got {events:?}"
        );
        // And exactly one — not one per half-line.
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, ClaudeEvent::AwaitingPrompt))
                .count(),
            1
        );
    }

    #[test]
    fn malformed_json_line_skipped_others_still_emit() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "not-json-at-all").unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "valid row after garbage should still emit, got {events:?}"
        );
    }

    #[test]
    fn attach_before_file_exists_then_create() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at(
                "h1".into(),
                path.clone(),
                move |e| {
                    tx.send(e).unwrap();
                },
                None,
            )
            .unwrap();

        // File doesn't exist yet — adapter should be waiting on the
        // parent-dir watcher. Create it now.
        thread::sleep(Duration::from_millis(50));
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected AwaitingPrompt after late create, got {events:?}"
        );
    }

    #[test]
    fn attach_to_pre_existing_session_ending_in_end_turn_starts_in_waiting() {
        // The resume bug: every Claude harness shows green on Skein
        // restart because the existing end_turn row was written
        // before we attached. Probe-on-attach fixes it by emitting
        // a synthetic AwaitingPrompt immediately.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        {
            let mut f = fs::File::create(&path).unwrap();
            // Realistic shape: a user prompt, a few tool_use rounds,
            // ending with an end_turn.
            writeln!(
                f,
                r#"{{"type":"user","sessionId":"x","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Read","id":"t1","input":{{}}}}]}}}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"type":"user","sessionId":"x","toolUseResult":"ok","message":{{"content":[]}}}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at("h1".into(), path, move |e| tx.send(e).unwrap(), None)
            .unwrap();

        let events = drain_brief(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "history-probe should fire AwaitingPrompt synthetically, got {events:?}"
        );
    }

    #[test]
    fn attach_to_pre_existing_session_mid_turn_starts_in_running() {
        // Mid-turn resume: last assistant row had tool_use, so the
        // session was interrupted while a tool was being called.
        // claude --resume picks up, but we shouldn't claim it's
        // awaiting input — it's still running.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        {
            let mut f = fs::File::create(&path).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at("h1".into(), path, move |e| tx.send(e).unwrap(), None)
            .unwrap();

        let events = drain_brief(&rx);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "mid-turn session must not start in waiting, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AssistantTurn)),
            "mid-turn session should emit AssistantTurn to signal running, got {events:?}"
        );
    }

    #[test]
    fn determine_initial_state_handles_empty_and_metadata_only() {
        assert!(determine_initial_state("").is_none());
        assert!(determine_initial_state("\n\n").is_none());
        let metadata_only = concat!(
            r#"{"type":"permission-mode","sessionId":"x"}"#,
            "\n",
            r#"{"type":"ai-title","sessionId":"x"}"#,
            "\n"
        );
        assert!(determine_initial_state(metadata_only).is_none());
    }

    #[test]
    fn determine_initial_state_skips_sidechain_rows_when_picking_final() {
        // Real layout: main session ends in end_turn, then sub-agent
        // writes more rows. The sub-agent rows must not override the
        // main session's terminal state.
        let log = concat!(
            r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"end_turn","content":[]}}"#,
            "\n",
            r#"{"type":"assistant","isSidechain":true,"sessionId":"x","message":{"stop_reason":"tool_use","content":[]}}"#,
            "\n",
            r#"{"type":"user","isSidechain":true,"sessionId":"x","message":{"content":[]}}"#,
            "\n"
        );
        let result = determine_initial_state(log);
        assert!(
            matches!(result, Some(ClaudeEvent::AwaitingPrompt)),
            "main session's end_turn must win over sub-agent rows, got {result:?}"
        );
    }

    #[test]
    fn encode_cwd_matches_claude_scheme() {
        // Verified against real project dirs in chapter-5-recon §3.
        assert_eq!(encode_cwd("/Users/foo/bar"), "-Users-foo-bar");
        assert_eq!(encode_cwd("/foo-bar/baz"), "-foo-bar-baz");
    }

    // ── action persistence + backfill (issue #80) ────────────────

    /// Build a manager wired to a real on-disk Database in `dir`, and
    /// return the path used so the caller can re-open it. Action sink
    /// is enabled — call sites populate JSONL via `attach_at` with a
    /// `Some(ActionPersistence)`.
    fn make_persisting_adapter(
        jsonl: PathBuf,
        db_path: &Path,
        harness_id: &str,
        room_id: &str,
    ) -> ClaudeEventsManager {
        let db = Arc::new(crate::db::Database::open(db_path).unwrap());
        let manager = ClaudeEventsManager::new(Arc::clone(&db));
        let persistence = Some(ActionPersistence {
            extractor: ActionExtractor::new(),
            db,
            harness_id: harness_id.into(),
            room_id: room_id.into(),
        });
        manager
            .attach_at(harness_id.into(), jsonl, |_event| {}, persistence)
            .unwrap();
        manager
    }

    /// `Tool_use` + `tool_result` rows already in the file at attach
    /// time are backfilled into `harness_actions` on first attach.
    #[test]
    fn backfill_persists_existing_tool_calls() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let db_path = dir.path().join("test.db");

        // Pre-seed the JSONL with a tool_use + result pair.
        {
            let mut f = std::fs::File::create(&jsonl).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#
            ).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"stdout":"x"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}}]}}}}"#
            ).unwrap();
            f.sync_all().unwrap();
        }

        let _manager = make_persisting_adapter(jsonl, &db_path, "h1", "r1");

        let db = crate::db::Database::open(&db_path).unwrap();
        let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        assert_eq!(actions.len(), 1, "expected 1 backfilled action");
        let a = &actions[0];
        assert_eq!(a.kind, crate::db::action_kind::TOOL_CALL);
        assert_eq!(a.harness_id, "h1");
        let payload: serde_json::Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["tool"], "Bash");
    }

    /// Re-attaching to the same session (Skein restart) does not
    /// re-insert rows already in `harness_actions`. Only rows with
    /// a strictly newer `timestamp_ms` are persisted.
    #[test]
    fn second_attach_skips_already_persisted_rows() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let db_path = dir.path().join("test.db");

        {
            let mut f = std::fs::File::create(&jsonl).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#
            ).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"stdout":"x"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}}]}}}}"#
            ).unwrap();
            f.sync_all().unwrap();
        }

        // First attach: backfill = 1 row.
        let manager1 = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
        drop(manager1);

        // Re-attach to same JSONL + same DB. Should be a no-op for actions.
        let _manager2 = make_persisting_adapter(jsonl, &db_path, "h1", "r1");

        let db = crate::db::Database::open(&db_path).unwrap();
        let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        assert_eq!(actions.len(), 1, "expected no duplicate on re-attach");
    }

    /// Rows that appear in the file after the first backfill (Skein
    /// closed, Claude wrote more, Skein re-opens) ARE persisted.
    #[test]
    fn second_attach_picks_up_rows_added_while_skein_was_down() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let db_path = dir.path().join("test.db");

        // First batch (before Skein "closes").
        {
            let mut f = std::fs::File::create(&jsonl).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Read","input":{{"file_path":"/x"}}}}]}}}}"#
            ).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"file":"x","type":"file"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}}]}}}}"#
            ).unwrap();
            f.sync_all().unwrap();
        }
        let manager1 = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
        drop(manager1);

        // Append a second tool call (Skein was down, Claude kept working).
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&jsonl)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","uuid":"a2","timestamp":"2026-05-15T21:17:00.000Z","message":{{"content":[{{"type":"tool_use","id":"toolu_2","name":"Bash","input":{{"command":"pwd"}}}}]}}}}"#
            ).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","timestamp":"2026-05-15T21:17:01.000Z","toolUseResult":{{"stdout":"/foo"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_2","content":"/foo","is_error":false}}]}}}}"#
            ).unwrap();
            f.sync_all().unwrap();
        }

        let _manager2 = make_persisting_adapter(jsonl, &db_path, "h1", "r1");
        let db = crate::db::Database::open(&db_path).unwrap();
        let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        assert_eq!(
            actions.len(),
            2,
            "expected backfill to pick up only the new row"
        );
        // Newest first.
        let p0: serde_json::Value = serde_json::from_str(&actions[0].payload).unwrap();
        let p1: serde_json::Value = serde_json::from_str(&actions[1].payload).unwrap();
        assert_eq!(p0["tool"], "Bash");
        assert_eq!(p1["tool"], "Read");
    }

    /// Each row that arrives on the live tail (after attach) ALSO
    /// gets persisted. This is the steady-state path: Claude writes
    /// a new row, notify fires, tick reads + extracts + persists.
    #[test]
    fn live_tail_persists_rows_appended_after_attach() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let db_path = dir.path().join("test.db");

        // File doesn't exist yet — adapter will attach via parent-dir
        // watcher and start at byte 0 on create.
        let _manager = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");

        // Create + append a tool_use/result pair.
        {
            let mut f = std::fs::File::create(&jsonl).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"echo hi"}}}}]}}}}"#
            ).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"stdout":"hi"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"hi","is_error":false}}]}}}}"#
            ).unwrap();
            f.sync_all().unwrap();
        }

        // FSEvents can take up to ~1-2 s on macOS to deliver. Poll
        // the DB until the row lands or the deadline trips.
        let db = crate::db::Database::open(&db_path).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
            if !actions.is_empty() {
                assert_eq!(actions[0].kind, crate::db::action_kind::TOOL_CALL);
                break;
            }
            assert!(
                std::time::Instant::now() <= deadline,
                "live tail never persisted the action"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}
