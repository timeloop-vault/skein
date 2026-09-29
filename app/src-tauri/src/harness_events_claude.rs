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

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

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

/// Minimum spacing between heartbeat log lines for one attached adapter
/// (#362). Piggybacks on whatever tick already ran — no timer thread of
/// its own, so a transcript with nothing nearby to trigger a tick
/// simply gets no heartbeat until the next one does. That's fine: the
/// heartbeat's job is to prove "the Rust side is still ticking", and a
/// rising `last_pos` alongside `events_sent` is what distinguishes that
/// from "Rust stalled" — a frontend-silent-but-Rust-healthy report
/// would show both climbing while nothing arrives on the other side.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// How many consecutive ticks may see a UTF-8 decode failure at the
/// *same* `last_pos` before it's worth a warn (#362). One or two is the
/// ordinary case — a write straddling a debounce tick, picked up whole
/// on the very next one — logged at debug and cleared automatically the
/// moment a read at that position succeeds. A run that survives this
/// many ticks (1s+ of wall time at the 50ms debounce) means the file is
/// stuck mid multi-byte character for good, not just a momentary race.
const UTF8_STALL_WARN_THRESHOLD: u32 = 20;

/// How often the background supervisor re-evaluates every attached
/// adapter's watch set (#362) — see `ClaudeEventsManager::new`'s
/// spawned thread and `supervise_once`/`supervise_map`. Independent of
/// `DEBOUNCE_MS`: that's notify's own coalescing window once a watch is
/// live, this is how long a directory that vanished and came back (the
/// #362 failure mode — Windows delivers nothing when a watched
/// directory is deleted, so nothing tells the tail to look again) can
/// stay silently unwatched before the next pass notices.
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(2);

/// Pure decision for whether a heartbeat should fire this tick, given
/// when the last one fired (`None` = never yet) and the current time.
/// Factored out so the rate limit is testable without sleeping 60s
/// (#362) — a test just constructs `now` and `now - INTERVAL - epsilon`.
fn should_heartbeat(last: Option<Instant>, now: Instant) -> bool {
    match last {
        None => true,
        Some(last) => now.saturating_duration_since(last) >= HEARTBEAT_INTERVAL,
    }
}

/// Pure decision for whether this tick should log the UTF-8 stall warn,
/// given the running consecutive-failure count (already incremented for
/// this tick) and whether the warn already fired for the current run.
/// Fires exactly once per stall run, on the tick where the count first
/// reaches [`UTF8_STALL_WARN_THRESHOLD`] — factored out for the same
/// clock-free testability reason as `should_heartbeat`.
fn should_warn_utf8_stall(count: u32, already_warned: bool) -> bool {
    count >= UTF8_STALL_WARN_THRESHOLD && !already_warned
}

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
    /// A subagent transcript began (or resumed): a new
    /// `agent-<id>.jsonl` appeared under the session's `subagents`
    /// dir, or one that had already finished got more rows after its
    /// terminal row (a follow-up delegation to the same id, which
    /// flips it live again). `agent_type`/`description` come from the
    /// `agent-<id>.meta.json` sidecar when it exists and parses;
    /// `None` otherwise. `initial` is `true` only for the batch seeded
    /// from disk at attach time (`attach_at`'s `initial_subagent_starts`)
    /// — a transcript that already existed, with no terminal
    /// `stop_reason`, belongs to a Claude process that died with the
    /// PTY it ran in (PTYs die with Skein; `claude_events_attach` runs
    /// once per spawn), so it can never still be running. `false` is
    /// everything a live watcher tick discovers afterwards, which can
    /// be. #277 needs the distinction to avoid deferring a harness's
    /// "done" notification for the ceiling's whole duration after
    /// every restart on account of subagents that are already dead.
    SubagentStart {
        agent_id: String,
        agent_type: Option<String>,
        description: Option<String>,
        initial: bool,
    },
    /// A tool call inside a subagent's own turn just returned a
    /// result. This exists so a permission dialog a *subagent* opened
    /// can be cleared (epic #298 — today the badge stays stuck until
    /// the whole subagent finishes). It must never be read as a
    /// parent-harness phase change: it says nothing about the main
    /// session's own state.
    SubagentToolResult { agent_id: String },
    /// A subagent's transcript ended — its last row is an assistant
    /// row with a terminal `stop_reason` (see
    /// `skein_harness::claude::subagent_row_is_terminal`). A subagent
    /// has no user to await, so the end of its turn IS its exit.
    SubagentEnd {
        agent_id: String,
        agent_type: Option<String>,
        description: Option<String>,
    },
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

/// `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`. The encoding
/// and layout live in `skein-harness` (#209); chapter 5 pre-allocates
/// the session uuid, so a fresh spawn can only compute the path — the
/// file does not exist anywhere until the first prompt.
///
/// A transcript that already exists wins over the computed path. That
/// is resume, and the scan is what `resume.rs` already trusts: when the
/// two disagree the encoder has drifted from Claude's (#259 was exactly
/// that), and tailing the computed path would watch a directory Claude
/// never writes to. `None` when the home dir can't be resolved (exotic
/// environments) so the caller can no-op cleanly.
fn session_jsonl_path(cwd: &str, session_id: &str) -> Option<PathBuf> {
    let home = crate::home_dir()?;
    let computed = skein_harness::claude::session_jsonl_path(&home, cwd, session_id);
    if computed.is_file() {
        return Some(computed);
    }
    match skein_harness::claude::find_session_jsonl(&home, session_id) {
        Some(found) => {
            tracing::warn!(
                computed = %computed.display(),
                found = %found.display(),
                "claude_events: transcript is not where the cwd encodes to; tailing where it is"
            );
            Some(found)
        }
        None => Some(computed),
    }
}

/// Identifies a directory *instance*, not just its path (#362). A
/// delete followed by a recreate at the same path — the failure this
/// issue is about — must read as a change: on Windows,
/// `ReadDirectoryChangesW` on a directory that gets deleted just stops
/// delivering, silently and with no error, so nothing short of noticing
/// the identity changed will ever re-arm the watch. `dev`+`ino` is the
/// authoritative identity on Unix; Windows exposes no cheap equivalent
/// through `std`, so birth time is the next best signal — good enough
/// given a delete+recreate cycle is seconds apart, not
/// sub-timestamp-resolution apart. (Also covers the symmetric inotify
/// case: a deleted watched directory emits `IN_IGNORED` and the watch
/// is simply gone — `rearm` treats "no longer in `armed`'s desired set"
/// and "identity changed" the same way, so nothing extra was needed for
/// Linux/macOS.)
///
/// Two known ways this identity can fail to *distinguish* a
/// delete+recreate, both accepted because dead-tail detection
/// (`Adapter::check_dead_tail`) is the backstop for either:
///
/// - `of` can't always read one at all — `meta.created()` errors on
///   some filesystems/platforms (review finding on #410: the original
///   version silently dropped the directory from the watch set
///   whenever this failed, which is worse than a merely-imprecise
///   identity — a directory that's never watched can never re-arm even
///   on an ordinary identity change). `None` here means exactly that:
///   "exists, but this platform/filesystem won't say when it was
///   born" — still watched, just unable to distinguish instance A from
///   a same-path instance B. Two `None`s compare equal, which is
///   required for `rearm`'s `desired == armed` fast path to stay quiet
///   on a genuinely unchanged directory whose identity simply can't be
///   read.
/// - On Linux, a rapid delete+recreate can hand back the exact same
///   `(dev, ino)` pair (inode numbers get reused) — `rearm` would then
///   see no change at all, same as the `None` case above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirId(
    #[cfg(unix)] Option<(u64, u64)>,
    #[cfg(not(unix))] Option<std::time::SystemTime>,
);

impl DirId {
    /// Never fails for a path that exists — `desired_watches` already
    /// checked that with `is_dir()`. A metadata read that fails anyway
    /// (a race, or the identity simply isn't available on this
    /// platform/filesystem) degrades to the "unknown" identity rather
    /// than silently excluding the directory from the watch set — see
    /// the type's own doc comment for what that costs and why it's
    /// accepted.
    fn of(path: &Path) -> Self {
        let Ok(meta) = fs::metadata(path) else {
            return Self(None);
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self(Some((meta.dev(), meta.ino())))
        }
        #[cfg(not(unix))]
        {
            Self(meta.created().ok())
        }
    }
}

/// Walks `start` and its ancestors, returning the first that exists as
/// a directory right now. Used when the transcript's own parent isn't
/// there — the desired watch set still needs *some* existing point in
/// the filesystem to sit on, so a later `rearm` pass can notice the
/// real parent appear (as a direct child of whatever this returns, or
/// closer, once Claude creates more of the path).
fn nearest_existing_dir(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(p) = cur {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// The watch set an adapter *should* have right now, computed fresh
/// from disk every time it's called — never carried forward (#362: that
/// nothing ever did this after attach was the whole bug). `parent` is
/// the main transcript's directory; `subagents_dir` is the sibling
/// dir, when a path for it could be derived at all (see
/// `TailState::subagents_dir`'s doc comment) — included only when it
/// currently exists, since an armed watch on a directory that isn't
/// there is meaningless.
///
/// Never creates anything: Claude owns both directories, and
/// pre-creating them was part of #362 itself — a watch on a directory
/// Skein invented could point at nothing Claude ever writes to, if the
/// encoder and Claude's own layout ever drift (#259 was exactly that
/// for the main file).
fn desired_watches(parent: &Path, subagents_dir: Option<&Path>) -> Vec<(PathBuf, DirId)> {
    let mut out = Vec::new();
    let main_dir = if parent.is_dir() {
        Some(parent.to_path_buf())
    } else {
        parent.parent().and_then(nearest_existing_dir)
    };
    if let Some(dir) = main_dir {
        let id = DirId::of(&dir);
        out.push((dir, id));
    }
    if let Some(dir) = subagents_dir
        && dir.is_dir()
    {
        out.push((dir.to_path_buf(), DirId::of(dir)));
    }
    out
}

/// Per-harness adapter handle. Holds the debouncer — dropping it stops
/// the watcher, which in turn drops the closure that holds the other
/// clone of `state`, so all per-harness state goes with it — plus what
/// `rearm` (#362) needs to recompute and reconcile the watch set from
/// outside the debounce thread's closure: the same `state`/`on_event`
/// the closure already holds clones of, and `armed`, the watch set as
/// of the last successful reconciliation.
struct Adapter {
    debouncer: Debouncer<RecommendedWatcher>,
    state: Arc<Mutex<TailState>>,
    on_event: Arc<dyn Fn(ClaudeEvent) + Send + Sync>,
    armed: Vec<(PathBuf, DirId)>,
    /// One-shot guard, per path, for the "could not (re)arm watch" warn
    /// (#410) — without it, a directory that stays permanently
    /// unwatchable (permissions, say) warns on every single supervisor
    /// pass forever. Same shape as `TailState`'s existing one-shot
    /// guards: a path's entry is inserted on the transition into
    /// failing (that's the one `warn!`) and removed the moment a watch
    /// on it succeeds again, with later failures at debug while the
    /// entry is already present.
    failing_watches: HashSet<PathBuf>,
    /// Ingredients for a fresh `ActionPersistence` on re-attach (#410).
    /// `None` mirrors `ActionPersistence`'s own optionality in
    /// phase-only tests (`attach_at` called with `actions: None`).
    reattach_recipe: Option<ReattachRecipe>,
    /// Dead-tail tracking (#410): `Some((last_pos, since))` once a
    /// supervisor pass has observed the transcript grow past
    /// `last_pos` without the tail's own `last_pos` moving — `since` is
    /// when *this* stall began. Cleared the moment the file stops
    /// growing relative to `last_pos` (the tail caught up) or vanishes
    /// (a missing file isn't a stalled tail, it's just not there).
    stall: Option<(u64, Instant)>,
    /// Assigned from `Registry::next_generation` at the moment this
    /// `Adapter` is actually installed into `Registry::adapters` — the
    /// compare-and-swap a re-attach's insert needs (#410 review fix):
    /// building a replacement adapter (`build_adapter`) does real I/O
    /// and file watching entirely OUTSIDE the registry lock, so a
    /// `detach` or a fresh caller-driven `attach()` can land on this
    /// harness id in the gap between "a reattach was decided" and "the
    /// replacement is ready to install". A reattach's insert only
    /// proceeds if the harness id's CURRENT generation still matches
    /// the one it captured when it started building — otherwise it
    /// would either resurrect a closed harness (a leaked watcher still
    /// writing `harness_actions` for a room that's gone) or clobber a
    /// newer adapter that legitimately replaced it. `attach()` itself
    /// is never subject to this check — a caller-driven attach always
    /// wins, same as before this field existed.
    generation: u64,
}

/// Ingredients for a from-scratch `ActionPersistence`, kept on
/// `Adapter` so a re-attach (#410) — automatic (dead-tail) or manual
/// (`ClaudeEventsManager::reattach`) — can rebuild one exactly the way
/// a first attach would, rather than resurrecting the old one. That
/// matters: the old `ActionExtractor`'s pending-tool-use buffer may be
/// stuck mid-turn on a dead tail, and `attach_at`'s own `scan_history`
/// already re-derives everything from disk fresh, same as a first
/// attach or a Skein restart.
#[derive(Clone)]
struct ReattachRecipe {
    db: Arc<Database>,
    harness_id: String,
    room_id: String,
    cwd: String,
    app: Option<tauri::AppHandle>,
}

impl ReattachRecipe {
    fn fresh_persistence(&self) -> ActionPersistence {
        ActionPersistence {
            extractor: ActionExtractor::new(),
            db: Arc::clone(&self.db),
            harness_id: self.harness_id.clone(),
            room_id: self.room_id.clone(),
            cwd: self.cwd.clone(),
            app: self.app.clone(),
        }
    }
}

impl Adapter {
    /// Recompute the desired watch set and reconcile `armed` against
    /// it: unwatch anything gone or replaced, watch anything new or
    /// replaced. Returns whether anything actually changed — the
    /// caller (`supervise_map`) only owes a catch-up `tick` when it
    /// did. A watch that fails to (re)arm is logged and left out of
    /// `armed`, so the next pass — recomputing the same desired entry —
    /// retries it for free; nothing here ever gives up permanently.
    fn rearm(&mut self, harness_id: &str) -> bool {
        let (parent, subagents_dir) = {
            let s = self.state.lock();
            (
                s.path.parent().map(Path::to_path_buf),
                s.subagents_dir.clone(),
            )
        };
        let Some(parent) = parent else {
            return false;
        };
        let desired = desired_watches(&parent, subagents_dir.as_deref());
        if desired == self.armed {
            return false;
        }

        let mut changes: Vec<(PathBuf, &'static str)> = Vec::new();
        for (path, id) in &self.armed {
            if desired.iter().any(|(p, i)| p == path && i == id) {
                continue;
            }
            // Ignore the error — the handle may already be dead, which
            // is exactly the #362 failure mode (the directory it
            // pointed at is gone).
            let _ = self.debouncer.watcher().unwatch(path);
            let reason = if desired.iter().any(|(p, _)| p == path) {
                "replaced"
            } else {
                "vanished"
            };
            changes.push((path.clone(), reason));
        }

        let mut new_armed = Vec::with_capacity(desired.len());
        for (path, id) in &desired {
            if self.armed.iter().any(|(p, i)| p == path && i == id) {
                new_armed.push((path.clone(), *id));
                continue;
            }
            match self
                .debouncer
                .watcher()
                .watch(path, RecursiveMode::NonRecursive)
            {
                Ok(()) => {
                    let reason = if self.armed.iter().any(|(p, _)| p == path) {
                        "replaced"
                    } else {
                        "appeared"
                    };
                    changes.push((path.clone(), reason));
                    new_armed.push((path.clone(), *id));
                    if self.failing_watches.remove(path) {
                        tracing::debug!(
                            harness_id = %harness_id,
                            path = %path.display(),
                            "claude_events: watch armed after previously failing"
                        );
                    }
                }
                Err(e) => {
                    if self.failing_watches.insert(path.clone()) {
                        tracing::warn!(
                            harness_id = %harness_id,
                            path = %path.display(),
                            error = %e,
                            "claude_events: could not (re)arm watch; will retry next supervise pass"
                        );
                    } else {
                        tracing::debug!(
                            harness_id = %harness_id,
                            path = %path.display(),
                            error = %e,
                            "claude_events: watch still failing to arm"
                        );
                    }
                }
            }
        }

        self.armed = new_armed;
        if changes.is_empty() {
            return false;
        }
        tracing::info!(
            harness_id = %harness_id,
            ?changes,
            "claude_events: re-armed watch(es)"
        );
        true
    }

    /// Dead-tail detection (#410): a watch can be silently gone in ways
    /// `rearm` can never notice — nothing about the directory's own
    /// identity changed, `notify` just stopped delivering (an
    /// overflowed queue, a platform quirk, whatever). The only
    /// observable symptom is the transcript growing while this
    /// adapter's own `last_pos` doesn't. Called once per supervisor
    /// pass, *after* that pass's re-arm and catch-up tick — so
    /// `last_pos` reflects anything a merely-misarmed watch already
    /// caught up on its own, and this only ever fires for a tail that
    /// re-arming couldn't fix.
    ///
    /// Skips detection entirely while `tick` is mid a UTF-8 stall
    /// (`utf8_stall_count > 0`, see `TailState`'s own doc comment on
    /// it): the tail IS ticking there, it just can't decode a
    /// straddled multi-byte character yet, and a re-attach cannot fix
    /// that any better than the next ordinary tick can — `attach_at`'s
    /// own `read_to_string` would hit the exact same byte.
    ///
    /// Returns the diagnostics for a `tracing::warn!` line the instant
    /// this pass CONFIRMS a dead tail (stalled at the same `last_pos`
    /// for at least `DEAD_TAIL_AFTER`) — not on the pass that merely
    /// starts tracking a new stall.
    fn check_dead_tail(&mut self, now: Instant) -> Option<DeadTail> {
        let (path, last_pos, utf8_stalled) = {
            let s = self.state.lock();
            (s.path.clone(), s.last_pos, s.utf8_stall_count > 0)
        };
        if utf8_stalled {
            return None;
        }
        // Missing file is healthy, not stalled — nothing to tail yet
        // (or any more), and re-attaching wouldn't change that.
        let Ok(meta) = fs::metadata(&path) else {
            self.stall = None;
            return None;
        };
        let file_len = meta.len();
        if file_len <= last_pos {
            self.stall = None;
            return None;
        }
        match self.stall {
            Some((stalled_last_pos, since)) if stalled_last_pos == last_pos => {
                let stalled_for = now.saturating_duration_since(since);
                if stalled_for >= DEAD_TAIL_AFTER {
                    Some(DeadTail {
                        path,
                        last_pos,
                        file_len,
                        stalled_for_ms: stalled_for.as_millis(),
                    })
                } else {
                    None
                }
            }
            _ => {
                self.stall = Some((last_pos, now));
                None
            }
        }
    }
}

/// How long a transcript may grow past this adapter's own `last_pos`
/// with zero progress before a supervisor pass calls it dead and
/// re-attaches (#410). Comfortably above `DEBOUNCE_MS` and
/// `SUPERVISE_INTERVAL` both — this is "the watch is gone", not "the
/// debouncer hasn't flushed yet".
const DEAD_TAIL_AFTER: Duration = Duration::from_secs(10);

/// Minimum spacing between AUTOMATIC re-attaches for one harness
/// (#410) — a persistently unwatchable directory (permissions, say)
/// must not be re-attached every single supervisor pass forever. Only
/// applies to the dead-tail path; a manual `reattach` call always runs.
const REATTACH_BACKOFF: Duration = Duration::from_secs(60);

/// What a confirmed dead tail needs to log and to act on — returned by
/// `Adapter::check_dead_tail`.
struct DeadTail {
    path: PathBuf,
    last_pos: u64,
    file_len: u64,
    stalled_for_ms: u128,
}

/// Per-subagent tail state, one per `agent-<id>.jsonl` under the
/// session's `subagents` dir. Mirrors `TailState`'s read/carry-partial
/// shape but scoped to a single subagent transcript.
struct SubagentTail {
    path: PathBuf,
    /// Byte offset already consumed — same role as `TailState::last_pos`.
    last_pos: u64,
    /// Trailing partial line carried across ticks — same role as
    /// `TailState::partial`.
    partial: String,
    /// Whether the last row read from this transcript is terminal
    /// (see `skein_harness::claude::subagent_row_is_terminal`). A
    /// subagent that gets more rows after finishing (a follow-up
    /// delegation to the same id) flips this back to `false` and
    /// re-emits `SubagentStart`.
    finished: bool,
    /// From the `agent-<id>.meta.json` sidecar; `None` when it's
    /// absent or doesn't parse. Carried here so `SubagentEnd` can
    /// report the same type/description `SubagentStart` did, without
    /// re-reading the sidecar every tick.
    agent_type: Option<String>,
    description: Option<String>,
    /// Epoch ms parsed from the *first* row read off this transcript
    /// after this `SubagentTail` was created, used to compute
    /// `SubagentEnd`'s `duration_ms`. Set once (see
    /// `started_ms_resolved`); stays `None` when that first row
    /// carried no `timestamp` field. A subagent seeded from disk at
    /// attach time (see `initial_subagents` in `attach_at`) is jumped
    /// straight to EOF and its earlier rows are never read — no extra
    /// file read is added just to backfill a start time — so it is
    /// seeded with this already resolved to `None` and stays that way
    /// for its whole life: an absent duration is honest, a guessed
    /// one is not.
    started_ms: Option<i64>,
    /// Whether `started_ms` has already been set from the first row.
    /// Needed because `started_ms` staying `None` is itself a valid
    /// resolved outcome (row had no timestamp) that must not be
    /// overwritten by a later row.
    started_ms_resolved: bool,
    /// Carry-forward "most recent timestamp seen on this transcript" —
    /// same role as `ActionExtractor::last_ts_ms` in
    /// `harness_actions_claude.rs`. Used as the `SubagentEnd` action's
    /// own timestamp when the terminal row itself carries no
    /// `timestamp`, rather than inventing `now`.
    last_ts_ms: i64,
    /// Whether the last attempt to open/seek this transcript failed and
    /// was already warned about (#362). A subagent file that becomes
    /// permanently unreadable would otherwise warn on every single
    /// tick forever — the entry is never removed and the metadata
    /// fast-path (`meta.len() == tail.last_pos`) can't short-circuit a
    /// broken file either. Set on the transition into failing, cleared
    /// (with a debug! line) the moment an open succeeds again.
    open_failure_logged: bool,
}

/// Mutable state shared with the watcher callback. The callback runs
/// on the debouncer thread — every field it touches lives in here.
// Four independent flags, each tracking a distinct one-shot condition
// (attach/parse state vs. two separate #362 log-dedup guards) rather
// than variants of one state machine — `struct_excessive_bools`'s
// alternative would just rename the same booleans.
#[allow(clippy::struct_excessive_bools)]
struct TailState {
    /// Stamped on every diagnostic log line this state's tick/detach
    /// path emits, so one harness's tail can be grepped out of a log
    /// that interleaves many rooms and harnesses (#362). Independent
    /// of `actions`, which is `None` in phase-only tests.
    harness_id: String,
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
    /// Whether this state has EVER been attached to the transcript
    /// (#425). `attached` flips back to false when the file vanishes,
    /// so it cannot tell a first appearance (every byte is new, read
    /// from 0 as live) from a reappearance (the same transcript,
    /// already consumed up to `last_pos`, which must not be replayed).
    ever_attached: bool,
    /// The last up to `FINGERPRINT_LEN` bytes ending at `last_pos`
    /// (#425). When the transcript reappears after a vanish, the same
    /// bytes at the same offset are what says "same file, keep going";
    /// anything else is a different or truncated file and is resynced
    /// as backfill instead of replayed as live.
    fingerprint: Vec<u8>,
    /// Tracks whether the previous emitted event was inside an
    /// assistant turn — used to coalesce streamed `assistant` rows
    /// into one `AssistantTurn` event per turn boundary.
    in_assistant_turn: bool,
    /// Action persistence sink. `None` for path-injected tests that
    /// only care about phase events. Populated in production by
    /// `attach_at_with_actions`. Lives in `TailState` so the watcher
    /// callback can both extract and persist on each tick. Issue #80.
    actions: Option<ActionPersistence>,
    /// The session's `subagents` dir — the computed path, whenever
    /// `skein_harness::claude::subagents_dir` can derive one, whether or
    /// not it exists yet (#362: this used to be created eagerly at
    /// attach time; Claude owns it, and a session that hasn't delegated
    /// yet has no reason to have one on disk before it does). `None`
    /// disables subagent tailing entirely for this attach — telemetry
    /// here is strictly additive and must never be a reason `attach`
    /// itself fails. The watch on this path (once it exists) is armed
    /// and re-armed by `Adapter::rearm`, not by anything in here.
    subagents_dir: Option<PathBuf>,
    /// Per-subagent tail state, keyed by agent id.
    subagents: HashMap<String, SubagentTail>,
    /// One-shot guard for the "could not read subagents dir this tick"
    /// warn (#362) — without it, a subagents dir that becomes
    /// permanently unreadable (deleted, permissions) warns on every
    /// tick forever. Set on the transition into failing, cleared once
    /// a read succeeds again.
    subagents_dir_read_failure_logged: bool,
    /// Set the first time a tick reads any bytes from the MAIN
    /// transcript since this `TailState` was created (#362) — i.e.
    /// once per attach, including a resumed session where the file
    /// already existed (it then fires on the first *appended* bytes
    /// after attach, since `attach_at` seeks straight to EOF). Backs
    /// the one `tracing::info!` line in `tick` that distinguishes
    /// "attached but nothing has ever come through the tail" from
    /// "tailing fine" at the default info filter.
    first_read_logged: bool,
    /// Cumulative count of `ClaudeEvent`s handed to `on_event` since
    /// this `TailState` was created — main and subagent events combined
    /// (#362). One of the heartbeat's two liveness numbers, alongside
    /// `last_pos`: both climbing means the tail is genuinely healthy
    /// even if the frontend never shows it; both frozen while the file
    /// grows is what a real stall looks like.
    events_sent: u64,
    /// Cumulative count of `on_event` dispatch batches that panicked,
    /// caught via `catch_unwind` around the whole per-tick dispatch loop
    /// (#362) so a bad callback can't unwind through `tick` unnoticed.
    /// Expected to stay 0 in production — the real callback in `lib.rs`
    /// only `.is_err()`s a dead channel, it never panics — so this
    /// exists for the heartbeat's own honesty and for tests that use a
    /// channel that can panic on send.
    send_errors: u64,
    /// Wall-clock time the last heartbeat line was logged; `None`
    /// before the first one. Rate-limited to at most one per
    /// `HEARTBEAT_INTERVAL` — see `should_heartbeat`.
    last_heartbeat: Option<Instant>,
    /// `last_pos` value the current run of consecutive UTF-8 mid-line
    /// failures started at (#362). `None` when the main tail isn't
    /// currently stalled at all.
    utf8_stall_at: Option<u64>,
    /// How many consecutive ticks have failed to decode at
    /// `utf8_stall_at`. Reset to 0 the moment a read succeeds (which
    /// can only happen once `last_pos` is genuinely able to advance
    /// past the incomplete character) — see `UTF8_STALL_WARN_THRESHOLD`.
    utf8_stall_count: u32,
    /// Whether the stall warn already fired for the *current* run
    /// (`utf8_stall_at`) — logged once per stall, not once per tick
    /// once past the threshold. Rearmed by the same reset as
    /// `utf8_stall_count`.
    utf8_stall_warned: bool,
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
    /// The room worktree, so a live patch row can capture a review
    /// baseline for the file it names (#211). Empty for tests that
    /// only care about phase events.
    cwd: String,
    /// Frontend emitter for live rows. `None` in tests. Backfill never
    /// emits (the frontend loads history via its initial query); only
    /// the live tail broadcasts. Issue #80 D1.
    app: Option<tauri::AppHandle>,
}

/// Diagnostics returned from a successful `attach`/`attach_at`, for the
/// caller (`claude_events_attach` in lib.rs) to log without either side
/// having to reach into the other's internals (#362).
pub struct AttachInfo {
    /// The transcript path actually tailed — the computed path unless
    /// `session_jsonl_path` fell back to a scan (see its doc comment).
    pub path: PathBuf,
    /// Whether the transcript already existed on disk at attach time
    /// (resume) as opposed to a fresh spawn that hasn't written
    /// anything yet.
    pub already_existed: bool,
    /// Whether subagent-transcript tailing is enabled for this attach —
    /// i.e. whether a `subagents` dir *path* could be derived from the
    /// transcript path at all (#362: this no longer means the directory
    /// existed, or that a watch on it was armed, at attach time — a
    /// session that hasn't delegated yet has no such directory on disk,
    /// and `Adapter::rearm` picks up the watch once it appears). `false`
    /// only when `skein_harness::claude::subagents_dir` couldn't derive
    /// a path at all, which in practice never happens.
    pub subagents_armed: bool,
}

/// Everything a supervisor pass and the manual `reattach` verb need
/// under one lock (#410): the live adapters, plus the automatic
/// re-attach backoff timestamps. The backoff map is deliberately
/// separate from `Adapter` itself — a re-attach *replaces* the
/// `Adapter` wholesale (same as any other attach), so backoff state
/// living on it would be lost at exactly the moment it needs to
/// survive.
#[derive(Default)]
struct Registry {
    adapters: HashMap<String, Adapter>,
    last_auto_reattach: HashMap<String, Instant>,
    /// Source of `Adapter::generation` (#410 review fix) — bumped every
    /// time an adapter is actually installed (a fresh `attach` or a
    /// successful reattach), never reused. Starts at 0, so the first
    /// real generation assigned is 1; `Adapter::generation` therefore
    /// never needs an `Option` to mean "not yet installed" — by the
    /// time anything can observe an `Adapter` at all, it already has
    /// one.
    next_generation: u64,
}

/// Manager — registry of live Claude adapters keyed by harness id.
/// Mirrors the shape of `PtyManager` / `WatcherManager`. `inner` is
/// `Arc`-wrapped (#362) so the background supervisor thread spawned by
/// `new` can hold a `Weak` to it — it must never be the reason this
/// manager (and everything it tails) outlives the app.
pub struct ClaudeEventsManager {
    inner: Arc<Mutex<Registry>>,
    /// Shared with the per-attach persistence sink so each adapter
    /// can write `harness_actions` rows directly from its tick
    /// thread. Issue #80.
    db: Arc<Database>,
    /// Cloned into each attach's `ActionPersistence` so the live tail
    /// can broadcast new rows. `None` in tests. Issue #80 D1.
    app: Option<tauri::AppHandle>,
}

impl ClaudeEventsManager {
    pub fn new(db: Arc<Database>, app: tauri::AppHandle) -> Self {
        let inner = Arc::new(Mutex::new(Registry::default()));
        spawn_supervisor_thread(Arc::downgrade(&inner));
        Self {
            inner,
            db,
            app: Some(app),
        }
    }

    /// Test constructor — no `AppHandle`, so the live tail persists
    /// without broadcasting (nothing to assert on the emit in a unit
    /// test, and building a real `AppHandle` needs a running app). No
    /// background thread either (#362) — tests drive re-arming
    /// deterministically via `supervise_once` instead of racing a 2 s
    /// timer.
    #[cfg(test)]
    fn new_for_test(db: Arc<Database>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Registry::default())),
            db,
            app: None,
        }
    }

    /// Test-only entry point for the same pass the background thread
    /// spawned by `new` runs every `SUPERVISE_INTERVAL` (#362):
    /// `new_for_test` starts no thread, so tests drive re-arming
    /// deterministically by calling this instead of racing a 2 s timer.
    /// Returns how many adapters were actually re-armed, for tests to
    /// assert against (a healthy, unchanged adapter re-arms 0).
    #[cfg(test)]
    pub(crate) fn supervise_once(&self) -> usize {
        supervise_map(&self.inner)
    }

    /// Test-only: run dead-tail detection alone, without immediately
    /// performing whatever it decides (#410 review fix) — the seam a
    /// test uses to land a `detach` or a fresh `attach()` in the gap
    /// between "a reattach was decided" and "the replacement is
    /// installed", the exact race `reattach_at_impl`'s generation CAS
    /// exists to survive, deterministically rather than via a real
    /// thread race. Pair with `perform_reattach_jobs_for_test`.
    #[cfg(test)]
    fn collect_dead_tail_jobs_for_test(&self) -> Vec<ReattachJob> {
        collect_dead_tail_jobs(&self.inner)
    }

    /// Test-only counterpart to `collect_dead_tail_jobs_for_test`: runs
    /// exactly what `supervise_map` would have run immediately after
    /// collecting, for each job, in order.
    #[cfg(test)]
    fn perform_reattach_jobs_for_test(&self, jobs: Vec<ReattachJob>) {
        for job in jobs {
            perform_reattach(&self.inner, job);
        }
    }

    /// Test-only: simulate a watch dying silently (#410) — the failure
    /// mode dead-tail detection exists for, where nothing about the
    /// watched directories' *identity* changes (so `rearm` has nothing
    /// to notice) but `notify` simply stops delivering. Unwatches every
    /// path this adapter currently thinks is armed WITHOUT touching
    /// `adapter.armed` itself, so `rearm`'s `desired == armed` fast path
    /// still sees no change and won't repair this on its own — only
    /// dead-tail detection can. Panics if `harness_id` isn't attached.
    #[cfg(test)]
    fn simulate_dead_watch_for_test(&self, harness_id: &str) {
        let mut reg = self.inner.lock();
        let adapter = reg
            .adapters
            .get_mut(harness_id)
            .expect("simulate_dead_watch_for_test: harness not attached");
        for (path, _) in &adapter.armed {
            let _ = adapter.debouncer.watcher().unwatch(path);
        }
    }

    /// Test-only: push a harness's already-recorded `Adapter::stall`
    /// further into the past than `DEAD_TAIL_AFTER`, so a test can
    /// confirm dead-tail detection without a real 10 s sleep — see
    /// `Adapter::check_dead_tail`'s doc comment for the state machine
    /// this pretends has already run its course. Panics if
    /// `harness_id` isn't attached or hasn't recorded a stall yet (call
    /// `supervise_once` once first to start one).
    #[cfg(test)]
    fn backdate_stall_for_test(&self, harness_id: &str) {
        let mut reg = self.inner.lock();
        let adapter = reg
            .adapters
            .get_mut(harness_id)
            .expect("backdate_stall_for_test: harness not attached");
        let (last_pos, _) = adapter
            .stall
            .expect("backdate_stall_for_test: no stall recorded yet");
        let since = Instant::now()
            .checked_sub(DEAD_TAIL_AFTER + Duration::from_secs(1))
            .expect("backdate_stall_for_test: process clock underflow");
        adapter.stall = Some((last_pos, since));
    }

    /// Start tailing the JSONL for `harness_id`. `on_event` fires on
    /// every parsed event from the debouncer's flush thread. Replaces
    /// any prior adapter for the same harness id (caller-driven
    /// reattach during respawn — fine to be idempotent).
    ///
    /// `room_id` is stamped on every `harness_actions` row this
    /// adapter persists (issue #80). The Live Context cards query
    /// per-room.
    ///
    /// Kept as an owned `harness_id: String` at this public boundary —
    /// unlike the internal `attach_at`/`attach_at_impl`/`build_adapter`
    /// chain below it, which all narrowed to `&str` for #410 — since
    /// this is where a Tauri command's already-owned, freshly
    /// deserialized `String` naturally lands; `#[allow]` below rather
    /// than threading a borrow back out to `lib.rs` for no benefit.
    #[allow(clippy::needless_pass_by_value)]
    pub fn attach<F>(
        &self,
        harness_id: String,
        room_id: String,
        session_id: &str,
        cwd: &str,
        on_event: F,
    ) -> Result<AttachInfo, ClaudeEventsError>
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
            cwd: cwd.to_string(),
            app: self.app.clone(),
        });
        self.attach_at(&harness_id, path, on_event, persistence)
    }

    /// Path-injected variant — used by tests to point the adapter at
    /// a tempdir without touching `HOME`. The production path goes
    /// through `attach()` above, which resolves the JSONL path from
    /// `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
    ///
    /// `actions` is the persistence sink; pass `None` from phase-only
    /// tests to skip the `harness_actions` table entirely.
    ///
    /// Thin wrapper over `attach_at_impl` (#410): the actual body only
    /// ever touches `self.inner`, never `self.db`/`self.app`, so it's a
    /// free function taking `&Arc<Mutex<Registry>>` directly — that's
    /// what lets `supervise_map`'s dead-tail re-attach and the manual
    /// `reattach` verb call the exact same attach logic without needing
    /// a whole `&ClaudeEventsManager` (the background supervisor thread
    /// only ever holds a `Weak` to `inner`, not to the manager itself).
    fn attach_at<F>(
        &self,
        harness_id: &str,
        path: PathBuf,
        on_event: F,
        actions: Option<ActionPersistence>,
    ) -> Result<AttachInfo, ClaudeEventsError>
    where
        F: Fn(ClaudeEvent) + Send + Sync + 'static,
    {
        attach_at_impl(&self.inner, harness_id, path, on_event, actions)
    }
}

/// Builds a fully-armed `Adapter` from scratch — all the real work of
/// an attach (reading history, arming watches, running the first
/// catch-up tick) — WITHOUT ever touching `Registry` (#410 review fix).
/// That split matters: installing the result needs a
/// compare-and-swap-under-lock for a reattach (see `reattach_at_impl`)
/// but not for a normal caller-driven attach (see `attach_at_impl`),
/// and neither install step should hold the registry lock across this
/// function's disk I/O and `notify` calls. `Adapter::generation` on the
/// returned value is a placeholder (`0`) — every caller overwrites it
/// with a real generation at the moment it actually installs the
/// adapter, under the lock.
fn build_adapter<F>(
    harness_id: &str,
    path: PathBuf,
    on_event: F,
    actions: Option<ActionPersistence>,
) -> Result<(Adapter, AttachInfo), ClaudeEventsError>
where
    F: Fn(ClaudeEvent) + Send + Sync + 'static,
{
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
        // file read — `scan_history` walks it exactly once (#171e:
        // this used to be two full parses, `determine_initial_state`
        // then `backfill_actions`, each running `serde_json::from_str`
        // on every line) and the fresh actions it collects are
        // persisted with one batch insert instead of one per line.
        let mut actions = actions;
        // Captured before `actions` is moved into `TailState` below, so
        // a future re-attach (#410) can build a brand-new
        // `ActionPersistence` — including a fresh `ActionExtractor` —
        // rather than resurrecting one that may be stuck mid-turn.
        let reattach_recipe = actions.as_ref().map(|ap| ReattachRecipe {
            db: Arc::clone(&ap.db),
            harness_id: ap.harness_id.clone(),
            room_id: ap.room_id.clone(),
            cwd: ap.cwd.clone(),
            app: ap.app.clone(),
        });
        let (last_pos, attached, initial_event, fingerprint) = match fs::read_to_string(&path) {
            Ok(content) => {
                let (init, fresh) = scan_history(&content, actions.as_mut());
                if let Some(ap) = actions.as_ref() {
                    persist_extracted_batch(ap, fresh);
                }
                let len = u64::try_from(content.len()).unwrap_or(u64::MAX);
                (len, true, init, fingerprint_of(content.as_bytes()))
            }
            // The file EXISTS but can't be read as UTF-8 (a write cut
            // mid-character): claim it as already attached-before, with
            // a `last_pos` no file can reach, so the first tick takes
            // the reappear path and resyncs it as backfill instead of
            // reading it from 0 as live (#425).
            Err(_) if path.exists() => (u64::MAX, false, None, Vec::new()),
            Err(_) => (0, false, None, Vec::new()),
        };
        let ever_attached = attached || last_pos == u64::MAX;
        // Derive the parent before moving `path` into TailState.
        // Watching the parent (not the file directly) survives some
        // platforms (Linux/inotify) losing the watch when the file is
        // replaced atomically. It's also `Adapter::rearm`'s (#362)
        // starting point for the desired watch set — see
        // `desired_watches`, which falls back to the nearest existing
        // ancestor when this doesn't exist yet (a brand-new worktree,
        // before Claude has written anything here at all).
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ClaudeEventsError("session path has no parent".into()))?;

        // Subagent transcripts live in a sibling `<session-id>/subagents`
        // dir next to the main `.jsonl` (`skein_harness::claude::
        // subagents_dir`, #209). #362: this is NOT created here any
        // more — it's Claude's directory to create, the moment (if
        // ever) a session actually delegates, and pre-creating it ahead
        // of that was itself part of #362's bug: a watch armed on a
        // directory Skein invented, before anything could ever be
        // written into it. `subagents_dir_opt` is just the computed
        // path; whether it currently exists is checked fresh every time
        // `desired_watches` runs — at attach below, and on every later
        // supervisor pass (`Adapter::rearm`).
        let subagents_dir_opt = skein_harness::claude::subagents_dir(&path);

        // Seed initial subagent state from disk *before* arming the
        // watcher — same reasoning as the history probe above: the
        // disk is the truth, and a subagent could already be mid-flight
        // by the time we watch. `last_pos` starts at each file's
        // current length (everything already there counts as "seen");
        // `finished` reflects whichever row is last, so a subagent
        // that already finished before this attach doesn't get
        // replayed as freshly started. The `SubagentStart` events
        // themselves are held back to `initial_subagent_starts` and
        // emitted only after the watcher is armed, for the same
        // no-lost-window reason `initial_event` is below.
        //
        // Accepted gap: a subagent that started AND finished entirely
        // within the gap this attach is closing never gets a
        // `SubagentStart` *or* a `subagent_end` action row — it's
        // already `finished` by the time this runs, so it's seeded
        // silently, same as the identical gap on a plain Skein restart
        // (a PTY, and everything running in it, dies with the process).
        // #410 makes this reachable a second way: a dead-tail re-attach
        // runs this exact same seeding path, so a subagent that both
        // started and finished during the dead window is missed the
        // same way, even though the harness itself never restarted.
        let mut initial_subagents: HashMap<String, SubagentTail> = HashMap::new();
        let mut initial_subagent_starts: Vec<ClaudeEvent> = Vec::new();
        if let Some(dir) = &subagents_dir_opt
            && let Ok(entries) = fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                let sub_path = entry.path();
                let Some(agent_id) = skein_harness::claude::subagent_id_from_path(&sub_path) else {
                    continue;
                };
                let content = fs::read_to_string(&sub_path).unwrap_or_default();
                let last_pos = u64::try_from(content.len()).unwrap_or(u64::MAX);
                let finished = subagent_content_is_finished(&content);
                let meta = skein_harness::claude::read_subagent_meta(&sub_path);
                let (agent_type, description) =
                    meta.map_or((None, None), |m| (m.agent_type, m.description));
                if !finished {
                    // #362: one line per subagent id the moment it joins
                    // the tailed set, so a session with subagent activity
                    // shows up in the log even when the main transcript
                    // itself goes quiet around the same time.
                    tracing::info!(
                        harness_id = %harness_id,
                        agent_id = %agent_id,
                        initial = true,
                        agent_type = ?agent_type,
                        "claude_events: subagent transcript joined the tailed set"
                    );
                    initial_subagent_starts.push(ClaudeEvent::SubagentStart {
                        agent_id: agent_id.clone(),
                        agent_type: agent_type.clone(),
                        description: description.clone(),
                        initial: true,
                    });
                }
                initial_subagents.insert(
                    agent_id,
                    SubagentTail {
                        path: sub_path,
                        last_pos,
                        partial: String::new(),
                        finished,
                        agent_type,
                        description,
                        // Seeded at attach, jumped straight to EOF —
                        // see the field doc on `started_ms`.
                        started_ms: None,
                        started_ms_resolved: true,
                        last_ts_ms: 0,
                        open_failure_logged: false,
                    },
                );
            }
        }

        let attach_info_path = path.clone();
        let state = Arc::new(Mutex::new(TailState {
            harness_id: harness_id.to_string(),
            path,
            last_pos,
            partial: String::new(),
            attached,
            ever_attached,
            fingerprint,
            in_assistant_turn: false,
            actions,
            subagents_dir: subagents_dir_opt.clone(),
            subagents: initial_subagents,
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }));
        let cb_state = Arc::clone(&state);
        let on_event: Arc<dyn Fn(ClaudeEvent) + Send + Sync> = Arc::new(on_event);
        let cb_on_event = Arc::clone(&on_event);
        let cb_harness_id = harness_id.to_string();

        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| {
                // notify can deliver an Err when the queue overflows
                // (rare, but possible during heavy filesystem activity).
                // Treat that exactly like a normal tick — we'll read
                // up to the current EOF and catch up. Better
                // stale-but-honest than silently miss events. Still
                // worth a warn: a run of these means we may be missing
                // debounce coalescing, not just losing one event (#362).
                if let Err(errors) = &result {
                    tracing::warn!(
                        harness_id = %cb_harness_id,
                        ?errors,
                        "claude_events: debouncer reported error(s); ticking anyway"
                    );
                }
                // A panic inside `tick` would otherwise unwind straight
                // through the notify callback and silently kill the
                // watcher thread — no more events, ever, for this
                // harness, with nothing in the log to say why (#362).
                // `parking_lot::Mutex` doesn't poison, so the lock
                // stays usable for the next tick even if this one
                // panicked mid-hold.
                if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tick(&cb_state, cb_on_event.as_ref());
                })) {
                    tracing::error!(
                        harness_id = %cb_harness_id,
                        panic = %panic_message(&panic),
                        "claude_events: tick panicked; watcher stays alive"
                    );
                }
            },
        )
        .map_err(ClaudeEventsError::from_err)?;

        // Arm the initial watch set (#362): `desired_watches` picks the
        // parent if it exists or its nearest existing ancestor if it
        // doesn't yet, plus the subagents dir when that already exists.
        // Neither is created here — see the doc comments above on
        // `parent`'s derivation and `subagents_dir_opt`. A watch that
        // fails to arm is warned and simply left out of `armed`; unlike
        // the old unconditional `?` on the main-parent watch, that must
        // NOT fail `attach` any more — the periodic supervisor
        // (`Adapter::rearm`) will retry it on the next pass, the same
        // path a directory that vanishes mid-session takes.
        let initial_desired = desired_watches(&parent, subagents_dir_opt.as_deref());
        let mut armed: Vec<(PathBuf, DirId)> = Vec::with_capacity(initial_desired.len());
        for (dir, id) in &initial_desired {
            match debouncer.watcher().watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => armed.push((dir.clone(), *id)),
                Err(e) => {
                    tracing::warn!(
                        harness_id = %harness_id,
                        path = %dir.display(),
                        error = %e,
                        "claude_events: could not arm watch at attach; the periodic supervisor will retry"
                    );
                }
            }
        }

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
        // Same reasoning, for the subagents discovered above: emit
        // unconditionally — `initial_subagent_starts` is only ever
        // non-empty when `subagents_dir_opt` was `Some` *and* that dir
        // already existed and was readable (see the seeding loop
        // above), so there's nothing to gate here any more.
        for event in initial_subagent_starts {
            on_event(event);
        }

        // One immediate tick to catch anything written between the
        // read_to_string above and the watcher arming. In the steady
        // case this seeks to last_pos == file length and reads 0
        // bytes — cheap no-op.
        tick(&state, on_event.as_ref());

        let subagents_armed = subagents_dir_opt.is_some();
        let adapter = Adapter {
            debouncer,
            // The closure captured in the debouncer above holds its own
            // clone of each (`cb_state`/`cb_on_event`); these are a
            // third, used by `Adapter::rearm` and its catch-up tick
            // (#362), which run outside that closure entirely.
            state: Arc::clone(&state),
            on_event: Arc::clone(&on_event),
            armed,
            failing_watches: HashSet::new(),
            reattach_recipe,
            stall: None,
            // Overwritten by whichever install step (`attach_at_impl` or
            // `reattach_at_impl`) actually inserts this adapter — see
            // `Adapter::generation`'s doc comment.
            generation: 0,
        };
        drop(state);
        Ok((
            adapter,
            AttachInfo {
                path: attach_info_path,
                already_existed: attached,
                subagents_armed,
            },
        ))
    }
}

/// Install a freshly built adapter unconditionally (#410 review fix) —
/// the normal caller-driven path (`ClaudeEventsManager::attach`/
/// `attach_at`): a real `attach()` call always wins, replacing whatever
/// was there, exactly like before generations existed. Builds the
/// adapter (real I/O, `notify` calls) OUTSIDE the registry lock; only
/// the assign-generation-and-insert step is under it.
fn attach_at_impl<F>(
    inner: &Arc<Mutex<Registry>>,
    harness_id: &str,
    path: PathBuf,
    on_event: F,
    actions: Option<ActionPersistence>,
) -> Result<AttachInfo, ClaudeEventsError>
where
    F: Fn(ClaudeEvent) + Send + Sync + 'static,
{
    let (mut adapter, info) = build_adapter(harness_id, path, on_event, actions)?;
    let mut reg = inner.lock();
    reg.next_generation += 1;
    adapter.generation = reg.next_generation;
    let replaced = reg
        .adapters
        .insert(harness_id.to_string(), adapter)
        .is_some();
    drop(reg);
    if replaced {
        tracing::info!(
            harness_id,
            "claude_events: attach replaced an existing adapter for this harness"
        );
    }
    Ok(info)
}

/// Outcome of `reattach_at_impl`'s compare-and-swap install (#410
/// review fix).
enum ReattachInstall {
    /// The adapter this reattach built was installed. No payload —
    /// neither caller (`perform_reattach`, `ClaudeEventsManager::
    /// reattach`) needs anything from the `AttachInfo` a successful
    /// reattach produces, only the fact that it succeeded.
    Installed,
    /// The CAS failed: `harness_id`'s current generation no longer
    /// matched the one this reattach was decided against, because a
    /// `detach` or a fresh `attach`/another reattach landed first. The
    /// freshly built (unused) adapter — and the `notify` watches it
    /// armed — are simply dropped. `still_present` says which: `false`
    /// means detached (the harness is gone), `true` means replaced by
    /// something newer.
    Abandoned { still_present: bool },
}

/// Install a re-attached adapter under a compare-and-swap on
/// `Adapter::generation` (#410 review fix — the fix for a real race:
/// `supervise_map` collects dead-tail jobs, and `ClaudeEventsManager::
/// reattach` builds its replacement, entirely outside the registry
/// lock; either a `detach` or a fresh caller-driven `attach()` can land
/// on the same harness id before the replacement is ready). Only
/// installs when `harness_id`'s adapter STILL has `expected_generation`
/// — otherwise something else already resolved this harness id and the
/// reattach is simply abandoned, never resurrecting a closed harness or
/// clobbering a newer adapter. Builds the adapter OUTSIDE the lock,
/// same as `attach_at_impl`; only the compare-and-maybe-insert is under
/// it.
fn reattach_at_impl<F>(
    inner: &Arc<Mutex<Registry>>,
    harness_id: &str,
    expected_generation: u64,
    path: PathBuf,
    on_event: F,
    actions: Option<ActionPersistence>,
) -> Result<ReattachInstall, ClaudeEventsError>
where
    F: Fn(ClaudeEvent) + Send + Sync + 'static,
{
    let (mut adapter, _info) = build_adapter(harness_id, path, on_event, actions)?;
    let mut reg = inner.lock();
    let current_generation = reg.adapters.get(harness_id).map(|a| a.generation);
    if current_generation != Some(expected_generation) {
        let still_present = current_generation.is_some();
        drop(reg);
        return Ok(ReattachInstall::Abandoned { still_present });
    }
    reg.next_generation += 1;
    adapter.generation = reg.next_generation;
    reg.adapters.insert(harness_id.to_string(), adapter);
    drop(reg);
    Ok(ReattachInstall::Installed)
}

/// Outcome of a manual `ClaudeEventsManager::reattach` call (#410) —
/// mirrors, string for string, `ReattachOutcome` in the frontend's
/// `harnessEvents.ts` (`reattachClaudeTelemetry`'s return type). Plain
/// unit variants with no `#[serde(tag = ..)]`, so this serializes as a
/// bare JSON string (`"reattached"`, not `{"kind":"reattached"}`) —
/// exactly what `invoke<ReattachOutcome>` on the frontend expects. A
/// `Result::Err` here collapses to a plain `String` at the
/// `claude_events_reattach` Tauri command boundary, which the frontend
/// treats as a rejection — the fourth outcome, not a variant of this
/// enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReattachOutcome {
    /// The tail was dead and is live again — the phase settles on its
    /// own from the events that follow, same as a fresh attach.
    Reattached,
    /// Nothing to do; already logged on the Rust side.
    Healthy,
    /// No adapter at all for this harness id (attach never ran, or it
    /// failed). Only a restart re-attaches from scratch in that case —
    /// there's nothing here to re-arm or re-attach.
    NotAttached,
}

impl ClaudeEventsManager {
    /// Stop the adapter for `harness_id`. No-op if unknown. Returns
    /// whether an adapter was actually removed, so the caller (and the
    /// `#362` unit test below) can tell a real detach from a stale one
    /// firing against an id that already went away. Also drops any
    /// pending automatic-re-attach backoff for this id (#410) — a
    /// harness that's gone has nothing left to back off from, and
    /// reusing a harness id later (unlikely, but IDs are caller-chosen)
    /// must not inherit a stale cooldown.
    pub fn detach(&self, harness_id: &str) -> bool {
        let mut reg = self.inner.lock();
        let removed = reg.adapters.remove(harness_id).is_some();
        reg.last_auto_reattach.remove(harness_id);
        tracing::info!(harness_id, removed, "claude_events: detach");
        removed
    }

    /// Manual counterpart to the automatic dead-tail path in
    /// `supervise_map` (#410) — the `claude_events_reattach` Tauri
    /// command's entry point. Unlike the automatic path, this never
    /// backs off: the caller explicitly asked right now.
    ///
    /// First re-arms — in case the fix is as cheap as a directory
    /// identity change `rearm` already knows how to handle — and, same
    /// as `supervise_map`'s own phase 1, only runs a catch-up `tick`
    /// when `rearm` reports it actually changed something. That
    /// asymmetry matters: the failure this whole issue is about is a
    /// watch that died with NO identity change at all, so `rearm`
    /// returns `false` and `last_pos` is left exactly as stale as it
    /// really is — running an unconditional tick here would silently
    /// catch it up via a plain synchronous read (which doesn't depend
    /// on the watch at all) and this call would report `Healthy` for a
    /// harness whose watch is still just as dead going forward.
    ///
    /// Neither `reattach_at_impl` nor its CAS check ever runs while
    /// `inner` is locked here (it locks `inner` itself, twice: once to
    /// read the generation to compare against, once to install) —
    /// everything needed is cloned out first. That CAS (#410 review
    /// fix) is why this can return `Healthy` even for a tail this
    /// function itself found dead: if a fresh `attach()` (or another
    /// reattach) replaced the adapter while this one was busy building
    /// its own replacement, the fresh adapter wins and this one's
    /// result is simply discarded — silently correct rather than
    /// clobbering something newer.
    pub fn reattach(&self, harness_id: &str) -> Result<ReattachOutcome, ClaudeEventsError> {
        let (generation, rearmed, path, state, on_event, recipe) = {
            let mut reg = self.inner.lock();
            let Some(adapter) = reg.adapters.get_mut(harness_id) else {
                tracing::info!(
                    harness_id,
                    "claude_events: manual reattach requested; not attached"
                );
                return Ok(ReattachOutcome::NotAttached);
            };
            let rearmed = adapter.rearm(harness_id);
            let path = adapter.state.lock().path.clone();
            (
                adapter.generation,
                rearmed,
                path,
                Arc::clone(&adapter.state),
                Arc::clone(&adapter.on_event),
                adapter.reattach_recipe.clone(),
            )
        };
        // See the doc comment above: only tick when `rearm` says it
        // changed something, so a watch that died silently (no identity
        // change, `rearm` reports `false`) leaves `last_pos` exactly as
        // stale as it really is for the check below.
        if rearmed {
            tick(&state, on_event.as_ref());
        }

        let last_pos = state.lock().last_pos;
        // A missing file is healthy, not dead — mirrors
        // `Adapter::check_dead_tail`'s same rule.
        let file_len = fs::metadata(&path).ok().map(|m| m.len());
        if file_len.is_none_or(|len| len <= last_pos) {
            tracing::info!(
                harness_id,
                "claude_events: manual reattach requested; tail is healthy, nothing to do"
            );
            return Ok(ReattachOutcome::Healthy);
        }
        let file_len = file_len.unwrap_or(last_pos);

        tracing::warn!(
            harness_id,
            reason = "manual",
            path = %path.display(),
            last_pos,
            file_len,
            "claude_events: re-attaching a dead tail"
        );
        let cb = on_event;
        let forward = move |e: ClaudeEvent| (cb.as_ref())(e);
        let persistence = recipe.as_ref().map(ReattachRecipe::fresh_persistence);
        match reattach_at_impl(
            &self.inner,
            harness_id,
            generation,
            path,
            forward,
            persistence,
        )? {
            ReattachInstall::Installed => Ok(ReattachOutcome::Reattached),
            ReattachInstall::Abandoned {
                still_present: false,
            } => {
                tracing::info!(
                    harness_id,
                    "claude_events: manual reattach abandoned: harness detached meanwhile"
                );
                Ok(ReattachOutcome::NotAttached)
            }
            ReattachInstall::Abandoned {
                still_present: true,
            } => {
                tracing::info!(
                    harness_id,
                    "claude_events: manual reattach abandoned: a newer attach won the race"
                );
                Ok(ReattachOutcome::Healthy)
            }
        }
    }
}

/// One adapter's `(state, on_event)` pair, carried out of the registry
/// lock so `supervise_map` can run its catch-up `tick` after releasing
/// it. Named purely so the `Vec` below doesn't trip
/// `clippy::type_complexity`.
type CatchUpTick = (
    Arc<Mutex<TailState>>,
    Arc<dyn Fn(ClaudeEvent) + Send + Sync>,
);

/// Everything `perform_reattach` needs for one harness, carried out of
/// the registry lock the same way `CatchUpTick` is (#410) —
/// `reattach_at_impl` must never run while `inner` is locked, since it
/// locks `inner` itself (twice) to check-and-install the replacement
/// adapter.
struct ReattachJob {
    harness_id: String,
    /// The generation `Adapter::check_dead_tail` observed this harness
    /// at, captured at collection time (#410 review fix) — the CAS
    /// `reattach_at_impl` runs before installing the replacement. A
    /// `detach` or a fresh `attach()` landing between collection and
    /// install bumps or removes this, so the stale job is abandoned
    /// instead of resurrecting or clobbering something.
    generation: u64,
    path: PathBuf,
    on_event: Arc<dyn Fn(ClaudeEvent) + Send + Sync>,
    recipe: Option<ReattachRecipe>,
    /// Short, log-friendly cause: `"transcript grew while the tail read
    /// nothing"` for the automatic dead-tail path, `"manual"` for
    /// `ClaudeEventsManager::reattach`.
    reason: &'static str,
    last_pos: u64,
    file_len: u64,
    /// `None` for a manual reattach — there's no stall duration to
    /// report when the caller asked right now, unconditionally.
    stalled_for_ms: Option<u128>,
}

/// Dead-tail detection: phase 2 of `supervise_map` (#410), split into
/// its own function so a test can run it and `perform_reattach` as two
/// separate steps with an arbitrary interleaving in between — exactly
/// the race `reattach_at_impl`'s generation CAS exists to survive,
/// exercised deterministically instead of needing a real thread race.
/// See `ClaudeEventsManager::collect_dead_tail_jobs_for_test` /
/// `perform_reattach_jobs_for_test`.
///
/// For every adapter *still* stalled — `rearm` (phase 1, run by the
/// caller first) couldn't have fixed it, since nothing about its
/// watched directories' identities changed — checks the
/// automatic-re-attach backoff (`Registry::last_auto_reattach`) and
/// collects a `ReattachJob` for anything both dead and off backoff.
/// Updates `last_auto_reattach` for each job collected, under the same
/// lock acquisition — a job is "spent" against the backoff the moment
/// it's decided, not when it's (maybe, much later) actually performed.
fn collect_dead_tail_jobs(inner: &Arc<Mutex<Registry>>) -> Vec<ReattachJob> {
    let mut to_reattach: Vec<ReattachJob> = Vec::new();
    let mut reg = inner.lock();
    let now = Instant::now();
    // Disjoint field borrows — `adapters` and `last_auto_reattach` need
    // to be mutated independently inside the same loop, which a single
    // `&mut reg.adapters` (borrowing all of `reg`) can't express.
    let Registry {
        adapters,
        last_auto_reattach,
        ..
    } = &mut *reg;
    for (harness_id, adapter) in adapters {
        let Some(dead) = adapter.check_dead_tail(now) else {
            continue;
        };
        if let Some(last) = last_auto_reattach.get(harness_id)
            && now.saturating_duration_since(*last) < REATTACH_BACKOFF
        {
            continue;
        }
        last_auto_reattach.insert(harness_id.clone(), now);
        to_reattach.push(ReattachJob {
            harness_id: harness_id.clone(),
            generation: adapter.generation,
            path: dead.path,
            on_event: Arc::clone(&adapter.on_event),
            recipe: adapter.reattach_recipe.clone(),
            reason: "transcript grew while the tail read nothing",
            last_pos: dead.last_pos,
            file_len: dead.file_len,
            stalled_for_ms: Some(dead.stalled_for_ms),
        });
    }
    to_reattach
}

/// The pass both `ClaudeEventsManager::supervise_once` and the
/// background thread spawned by `new` run (#362, extended by #410):
///
/// 1. Re-arm every adapter's watch set, then run one catch-up `tick`
///    for each adapter that actually changed — this can all by itself
///    resolve a merely-misarmed watch (a directory whose identity
///    changed), which is why dead-tail detection runs strictly after
///    it: `last_pos` needs to reflect anything that catch-up tick
///    already recovered on its own.
/// 2. Dead-tail detection (#410): `collect_dead_tail_jobs`, then
///    `perform_reattach` each job.
///
/// All three phases collect their cross-adapter work into a `Vec` and
/// run it after releasing `inner` — `tick` and `reattach_at_impl` both
/// do real I/O and must never run while every other `attach`/`detach`
/// call is blocked on the registry lock. Returns the number of
/// adapters re-armed (not reattached), purely for tests and the
/// supervisor's own bookkeeping.
fn supervise_map(inner: &Arc<Mutex<Registry>>) -> usize {
    let mut to_tick: Vec<CatchUpTick> = Vec::new();
    let mut rearmed = 0usize;
    {
        let mut reg = inner.lock();
        for (harness_id, adapter) in &mut reg.adapters {
            if adapter.rearm(harness_id) {
                rearmed += 1;
                to_tick.push((Arc::clone(&adapter.state), Arc::clone(&adapter.on_event)));
            }
        }
    }
    for (state, on_event) in to_tick {
        tick(&state, on_event.as_ref());
    }

    for job in collect_dead_tail_jobs(inner) {
        perform_reattach(inner, job);
    }

    rearmed
}

/// Re-attaches one dead tail (#410), unattended — the automatic path
/// from `supervise_map`. Warns once with the diagnostics `ReattachJob`
/// carries, then calls `reattach_at_impl`'s generation-CAS install
/// (#410 review fix): a failure there (a real attach error) is warned
/// and the existing (dead) adapter is left in place; an `Abandoned`
/// result (a `detach` or a fresh `attach()` raced this reattach) is
/// expected and merely noted at info — there's no caller here to hand
/// either outcome to, that's the manual
/// `ClaudeEventsManager::reattach`'s job.
fn perform_reattach(inner: &Arc<Mutex<Registry>>, job: ReattachJob) {
    tracing::warn!(
        harness_id = %job.harness_id,
        reason = job.reason,
        path = %job.path.display(),
        last_pos = job.last_pos,
        file_len = job.file_len,
        stalled_for_ms = job.stalled_for_ms,
        "claude_events: re-attaching a dead tail"
    );
    let cb = job.on_event;
    let forward = move |e: ClaudeEvent| (cb.as_ref())(e);
    let persistence = job.recipe.as_ref().map(ReattachRecipe::fresh_persistence);
    match reattach_at_impl(
        inner,
        &job.harness_id,
        job.generation,
        job.path,
        forward,
        persistence,
    ) {
        Ok(ReattachInstall::Installed) => {}
        Ok(ReattachInstall::Abandoned { .. }) => {
            tracing::info!(
                harness_id = %job.harness_id,
                "claude_events: re-attach abandoned: harness detached or re-attached meanwhile"
            );
        }
        Err(e) => {
            tracing::warn!(
                harness_id = %job.harness_id,
                error = %e,
                "claude_events: automatic re-attach failed; keeping the existing (dead) adapter"
            );
        }
    }
}

/// Background thread started by `ClaudeEventsManager::new` (production
/// only — `new_for_test` spawns none, see its doc comment). Loops
/// forever at `SUPERVISE_INTERVAL`, upgrading `inner` fresh each pass so
/// the thread exits cleanly the moment the manager itself is gone
/// rather than being the reason it can't be. A panic inside one pass
/// is caught and logged, mirroring the tick callback's own
/// `catch_unwind` (#362) — one bad pass must not silently end
/// supervision for every other attached harness for the rest of the
/// process's life.
fn spawn_supervisor_thread(inner: Weak<Mutex<Registry>>) {
    let spawned = thread::Builder::new()
        .name("claude-events-supervisor".to_string())
        .spawn(move || {
            loop {
                thread::sleep(SUPERVISE_INTERVAL);
                let Some(inner) = inner.upgrade() else {
                    tracing::info!("claude_events: supervisor exiting; manager is gone");
                    break;
                };
                if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    supervise_map(&inner);
                })) {
                    tracing::error!(
                        panic = %panic_message(&panic),
                        "claude_events: supervisor pass panicked; retrying next interval"
                    );
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!(
            error = %e,
            "claude_events: failed to spawn the watch supervisor thread; #362 re-arming is disabled for this run"
        );
    }
}

/// Format a `catch_unwind` panic payload for a log line. Panics carry
/// either a `&str` (the common `panic!("literal")` / `unwrap` case) or
/// a `String` (`panic!("{}", x)`); anything else is a payload type we
/// can't stringify without `Any::downcast` guessing, so name that
/// honestly rather than pretend.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// How many trailing consumed bytes `TailState::fingerprint` keeps (#425).
const FINGERPRINT_LEN: usize = 256;

/// The last up to `FINGERPRINT_LEN` bytes of `bytes` (#425).
fn fingerprint_of(bytes: &[u8]) -> Vec<u8> {
    bytes[bytes.len().saturating_sub(FINGERPRINT_LEN)..].to_vec()
}

/// Append freshly consumed bytes to a fingerprint, keeping only the
/// last `FINGERPRINT_LEN`. A read shorter than that extends the old
/// fingerprint rather than replacing it (#425).
fn extend_fingerprint(fp: &mut Vec<u8>, new: &[u8]) {
    fp.extend_from_slice(new);
    if fp.len() > FINGERPRINT_LEN {
        fp.drain(..fp.len() - FINGERPRINT_LEN);
    }
}

/// What a transcript that vanished and came back looks like relative
/// to what this tail had consumed (#425).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reappear {
    /// Same bytes at the consumed offset: keep going, the rest is new.
    Same,
    /// Shorter than what we consumed.
    Truncated,
    /// Long enough, but the consumed tail bytes differ (or can't be read).
    Different,
}

impl Reappear {
    fn name(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Truncated => "truncated",
            Self::Different => "different",
        }
    }
}

/// Classify a reappeared transcript by length and by the fingerprint
/// of the bytes ending at `last_pos` (#425).
fn classify_reappear(path: &Path, last_pos: u64, fp: &[u8], file_len: u64) -> Reappear {
    if file_len < last_pos {
        return Reappear::Truncated;
    }
    let Some(start) = last_pos.checked_sub(fp.len() as u64) else {
        return Reappear::Different;
    };
    let mut got = vec![0u8; fp.len()];
    let read = fs::File::open(path)
        .and_then(|mut f| {
            f.seek(SeekFrom::Start(start))?;
            f.read_exact(&mut got)
        })
        .is_ok();
    if read && got == fp {
        Reappear::Same
    } else {
        Reappear::Different
    }
}

/// Outcome of `resync_as_backfill` (#425).
enum Resync {
    Done(Option<ClaudeEvent>),
    Unreadable,
}

/// Resync a truncated or different transcript exactly like `attach_at`
/// reads an existing file (#425): fresh `ActionExtractor`, then
/// `scan_history` and `persist_extracted_batch` (only rows newer than the max persisted
/// timestamp, no broadcast, no baseline capture), `last_pos` at the
/// content length. Returns the derived initial phase event, which the
/// caller emits after dropping the lock. `Unreadable` means the file
/// could not be read as UTF-8; state is untouched so the caller can retry.
fn resync_as_backfill(s: &mut TailState) -> Resync {
    let content = match fs::read_to_string(&s.path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                harness_id = %s.harness_id,
                path = %s.path.display(),
                error = %e,
                "claude_events: could not read transcript to resync"
            );
            return Resync::Unreadable;
        }
    };
    if let Some(ap) = s.actions.as_mut() {
        // Same reasoning as `ReattachRecipe::fresh_persistence`: the old
        // extractor may be stuck mid-turn on rows that no longer exist.
        ap.extractor = ActionExtractor::new();
    }
    let (init, fresh) = scan_history(&content, s.actions.as_mut());
    if let Some(ap) = s.actions.as_ref() {
        persist_extracted_batch(ap, fresh);
    }
    s.last_pos = u64::try_from(content.len()).unwrap_or(u64::MAX);
    s.partial.clear();
    // `attach_at` starts a state with `false` after the same scan.
    s.in_assistant_turn = false;
    s.fingerprint = fingerprint_of(content.as_bytes());
    s.utf8_stall_at = None;
    s.utf8_stall_count = 0;
    s.utf8_stall_warned = false;
    Resync::Done(init)
}

/// Run `resync_as_backfill`, drop the lock, and emit only the initial
/// event (if any) through `dispatch_events` — `on_event` is never called
/// under the lock (#425).
fn finish_resync(
    mut s: parking_lot::MutexGuard<'_, TailState>,
    state: &Arc<Mutex<TailState>>,
    on_event: &(dyn Fn(ClaudeEvent) + Send + Sync),
) {
    let init = match resync_as_backfill(&mut s) {
        Resync::Done(init) => init,
        Resync::Unreadable => {
            // Retry on the next tick via the reappear path.
            s.attached = false;
            None
        }
    };
    drop(s);
    dispatch_events(state, init.into_iter().collect(), on_event);
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
            if s.ever_attached {
                // #425: the transcript we had already consumed
                // vanished and is back. Replaying it from 0 as live
                // would re-emit every historic row (phase events,
                // broadcasts, review baselines). Decide whether it is
                // the same file; if not, resync it as backfill.
                let file_len = fs::metadata(&s.path).map_or(0, |m| m.len());
                let case = classify_reappear(&s.path, s.last_pos, &s.fingerprint, file_len);
                tracing::info!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    case = case.name(),
                    last_pos = s.last_pos,
                    file_len,
                    "claude_events: transcript reappeared"
                );
                if case != Reappear::Same {
                    finish_resync(s, state, on_event);
                    return;
                }
            } else {
                s.ever_attached = true;
                // Start at 0 — file is fresh, all bytes are new.
                s.last_pos = 0;
                s.fingerprint.clear();
                tracing::info!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    "claude_events: transcript appeared"
                );
            }
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
    let mut file = match fs::File::open(&s.path) {
        Ok(f) => f,
        Err(e) => {
            // File vanished — Claude was uninstalled mid-session, or
            // the user wiped ~/.claude. Surface as SessionEnd once and
            // detach from this run (the adapter stays alive in case the
            // file comes back, but we don't keep re-emitting).
            if s.attached {
                s.attached = false;
                let harness_id = s.harness_id.clone();
                let path = s.path.clone();
                drop(s);
                tracing::warn!(
                    harness_id = %harness_id,
                    path = %path.display(),
                    error = %e,
                    "claude_events: transcript vanished; emitting SessionEnd"
                );
                on_event(ClaudeEvent::SessionEnd);
            } else if e.kind() != std::io::ErrorKind::NotFound {
                // NotFound while `!attached` is the expected "still
                // waiting for Claude to write the first row" case —
                // anything else (permissions, IO error) is worth a
                // line since it means this harness may never attach.
                tracing::warn!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    error = %e,
                    "claude_events: unexpected error opening transcript"
                );
            }
            return;
        }
    };

    // Defensive: file size dropped below last_pos (rotation /
    // truncation). #425: resync as backfill rather than replaying from
    // 0 as live — the rows in it are history we already reported (or
    // that the DB already holds), not new events.
    if let Ok(meta) = file.metadata()
        && meta.len() < s.last_pos
    {
        tracing::info!(
            harness_id = %s.harness_id,
            path = %s.path.display(),
            case = Reappear::Truncated.name(),
            last_pos = s.last_pos,
            file_len = meta.len(),
            "claude_events: transcript shrank"
        );
        drop(file);
        finish_resync(s, state, on_event);
        return;
    }

    if let Err(e) = file.seek(SeekFrom::Start(s.last_pos)) {
        tracing::warn!(
            harness_id = %s.harness_id,
            path = %s.path.display(),
            pos = s.last_pos,
            error = %e,
            "claude_events: seek failed"
        );
        return;
    }
    let mut buf = String::new();
    let bytes = match file.read_to_string(&mut buf) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::InvalidData {
                // UTF-8 decode failed somewhere mid-file. JSONL is
                // ASCII for the keys + UTF-8 content; the only way
                // this fires is if we landed in the middle of a
                // multi-byte char. Bump last_pos by what we did read
                // (zero in this case) and wait for the next tick to
                // pick up a full line. Expected transient state, not
                // an error — logged at debug.
                tracing::debug!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    "claude_events: utf8 mid-line; retrying next tick"
                );
                // #362: a one- or two-tick stall here is the ordinary
                // case above. Count consecutive failures at the SAME
                // last_pos — a run that survives past the threshold
                // means the file is stuck mid multi-byte character for
                // good, not just a write straddling a debounce tick.
                if s.utf8_stall_at == Some(s.last_pos) {
                    s.utf8_stall_count = s.utf8_stall_count.saturating_add(1);
                } else {
                    s.utf8_stall_at = Some(s.last_pos);
                    s.utf8_stall_count = 1;
                    s.utf8_stall_warned = false;
                }
                if should_warn_utf8_stall(s.utf8_stall_count, s.utf8_stall_warned) {
                    s.utf8_stall_warned = true;
                    let len = file.metadata().ok().map(|m| m.len());
                    tracing::warn!(
                        harness_id = %s.harness_id,
                        path = %s.path.display(),
                        pos = s.last_pos,
                        file_len = ?len,
                        ticks = s.utf8_stall_count,
                        "claude_events: transcript stuck mid multi-byte character across many ticks"
                    );
                }
            } else {
                tracing::warn!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    error = %e,
                    "claude_events: read failed"
                );
            }
            return;
        }
    };
    // Reaching here means the read decoded cleanly — any UTF-8 stall
    // run in progress is over. Rearm so a *future* stall gets its own
    // fresh count and its own warn (#362).
    s.utf8_stall_at = None;
    s.utf8_stall_count = 0;
    s.utf8_stall_warned = false;
    let file_len = file.metadata().ok().map(|m| m.len());
    let advance: u64 = u64::try_from(bytes).unwrap_or(u64::MAX);
    s.last_pos = s.last_pos.saturating_add(advance);
    extend_fingerprint(&mut s.fingerprint, buf.as_bytes());

    // First bytes ever read off the MAIN transcript since this attach
    // (#362) — once per attach, logged at info so "attached but the
    // watcher never fires / nothing read" is distinguishable from
    // "tailing fine" without cranking RUST_LOG. Fires on the first
    // *appended* bytes for a resumed session too, since `attach_at`
    // seeks straight to EOF before this tick ever runs.
    if bytes > 0 && !s.first_read_logged {
        s.first_read_logged = true;
        tracing::info!(
            harness_id = %s.harness_id,
            bytes,
            "claude_events: first transcript bytes read since attach"
        );
    }

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
    let mut lines_parsed: usize = 0;
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
        lines_parsed += 1;
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
            persist_extracted(ap, extracted, true);
        }
    }
    s.in_assistant_turn = in_assistant_turn;

    // Subagent transcripts, after the main file — same lock, same
    // events vec, so consumers see main-session events first and
    // subagent events second within one tick.
    let main_events = events.len();
    tick_subagents(&mut s, &mut events);
    let subagent_events = events.len() - main_events;
    // Only the subagents still being tailed live count as "watched" —
    // a finished one is a cheap stat check, not a file whose new rows
    // we're expecting (#362).
    let live_subagents = s.subagents.values().filter(|t| !t.finished).count();
    let watched_files = 1 + live_subagents;
    tracing::debug!(
        harness_id = %s.harness_id,
        bytes,
        lines_parsed,
        events = main_events,
        subagent_events,
        watched_files,
        "claude_events: tick"
    );

    // Heartbeat (#362): at most once per HEARTBEAT_INTERVAL per
    // attached adapter, and only on a tick that actually runs — no
    // timer thread. A rising `last_pos` alongside `events_sent` proves
    // the Rust side is still reading and emitting even when the
    // frontend goes quiet; both frozen while `file_len` keeps growing
    // is what an actual stall looks like.
    let now = Instant::now();
    if should_heartbeat(s.last_heartbeat, now) {
        s.last_heartbeat = Some(now);
        tracing::info!(
            harness_id = %s.harness_id,
            session = %s.path.display(),
            last_pos = s.last_pos,
            file_len = ?file_len,
            events_sent = s.events_sent,
            send_errors = s.send_errors,
            live_subagents,
            "claude_events: heartbeat"
        );
    }
    drop(s);

    dispatch_events(state, events, on_event);
}

/// Hand `events` to `on_event` with tick's panic containment and
/// `events_sent`/`send_errors` bookkeeping. Must be called WITHOUT the
/// state lock held. Shared by `tick` and the #425 resync path so a
/// resync's initial event is counted like any other.
fn dispatch_events(
    state: &Arc<Mutex<TailState>>,
    events: Vec<ClaudeEvent>,
    on_event: &(dyn Fn(ClaudeEvent) + Send + Sync),
) {
    // A panic inside `on_event` would otherwise unwind straight through
    // `tick` — caught here (rather than only by the debouncer callback's
    // own `catch_unwind`) so it's counted toward `send_errors` and
    // logged with the harness id that triggered it (#362). The
    // production callback in `lib.rs` never panics — it only
    // `.is_err()`s a dead channel — so this is expected to stay silent
    // there; it exists for this counter's own honesty and for tests
    // that use a channel that can panic on send.
    //
    // `dispatched` counts only events actually handed to `on_event`
    // before any panic, matching `events_sent`'s own doc — a panic
    // partway through the loop must not credit events that were never
    // delivered.
    let mut dispatched: u64 = 0;
    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for event in events {
            on_event(event);
            dispatched += 1;
        }
    }));
    let mut s = state.lock();
    s.events_sent = s.events_sent.saturating_add(dispatched);
    if let Err(panic) = panic_result {
        s.send_errors = s.send_errors.saturating_add(1);
        tracing::error!(
            harness_id = %s.harness_id,
            panic = %panic_message(&panic),
            total_send_errors = s.send_errors,
            "claude_events: on_event panicked while dispatching this tick's events"
        );
    }
}

/// Subagent-transcript half of `tick`: walks every `agent-*.jsonl`
/// under `state.subagents_dir`, tailing each exactly like the main
/// file (byte offset + carried partial line), and appends any
/// `SubagentStart`/`SubagentToolResult`/`SubagentEnd` events to
/// `events`. Called under the same lock `tick` already holds — never
/// locks independently.
///
/// A `None` `subagents_dir` (creation or watch failed at attach, or
/// this session never delegated and the dir was never armed) is a
/// silent no-op: subagent telemetry is strictly additive and must
/// never affect the main tail.
///
/// Also persists (and, live, broadcasts) one `subagent_end`
/// `harness_actions` row per `SubagentEnd` — mirroring how `tick`'s
/// main-file loop feeds `s.actions` — so the Live Context feed gets a
/// "subagent finished" row (epic #298). This only ever runs from a
/// live tick, never from attach-time disk seeding: a subagent already
/// finished on disk when `attach_at` seeds `initial_subagents` never
/// enters this function's terminal-row branch at all, because its
/// `finished` flag is already `true` and its rows are never re-read.
fn tick_subagents(s: &mut TailState, events: &mut Vec<ClaudeEvent>) {
    let Some(dir) = s.subagents_dir.clone() else {
        return;
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => {
            if s.subagents_dir_read_failure_logged {
                s.subagents_dir_read_failure_logged = false;
                tracing::debug!(
                    harness_id = %s.harness_id,
                    dir = %dir.display(),
                    "claude_events: subagents dir readable again"
                );
            }
            e
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // #362: normal now that `attach_at` no longer pre-creates
            // this directory — most sessions never delegate at all, so
            // this is the common case, not a problem to log. Leaves
            // `subagents_dir_read_failure_logged` untouched either way:
            // a prior *real* failure (permissions, say) still gets its
            // "readable again" debug line once a later read actually
            // succeeds, and this NotFound tick doesn't count as that.
            return;
        }
        Err(e) => {
            if !s.subagents_dir_read_failure_logged {
                s.subagents_dir_read_failure_logged = true;
                tracing::warn!(
                    harness_id = %s.harness_id,
                    dir = %dir.display(),
                    error = %e,
                    "claude_events: could not read subagents dir this tick"
                );
            }
            return;
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut new_count: usize = 0;
    let mut subagent_end_actions: Vec<crate::harness_actions_claude::ExtractedAction> = Vec::new();
    for entry in entries.flatten() {
        let sub_path = entry.path();
        let Some(agent_id) = skein_harness::claude::subagent_id_from_path(&sub_path) else {
            continue;
        };
        seen.insert(agent_id.clone());

        if !s.subagents.contains_key(&agent_id) {
            // A transcript we haven't seen before — a fresh
            // delegation since attach (or since the last tick).
            new_count += 1;
            let meta = skein_harness::claude::read_subagent_meta(&sub_path);
            let (agent_type, description) =
                meta.map_or((None, None), |m| (m.agent_type, m.description));
            // #362: one line per subagent id the moment it joins the
            // tailed set — the discovery this issue's symptom traced
            // back to (a session that had just started delegating).
            tracing::info!(
                harness_id = %s.harness_id,
                agent_id = %agent_id,
                initial = false,
                agent_type = ?agent_type,
                "claude_events: subagent transcript joined the tailed set"
            );
            events.push(ClaudeEvent::SubagentStart {
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                description: description.clone(),
                initial: false,
            });
            s.subagents.insert(
                agent_id.clone(),
                SubagentTail {
                    path: sub_path.clone(),
                    last_pos: 0,
                    partial: String::new(),
                    finished: false,
                    agent_type,
                    description,
                    started_ms: None,
                    started_ms_resolved: false,
                    last_ts_ms: 0,
                    open_failure_logged: false,
                },
            );
        }

        let Some(tail) = s.subagents.get_mut(&agent_id) else {
            continue;
        };

        // `tick` fires on ANY watched-path change — including ordinary
        // main-transcript writes that have nothing to do with
        // subagents — and finished entries are never removed from
        // `s.subagents` (the re-open transition below depends on them
        // surviving). Left unchecked, a long session with many
        // delegations pays an open+seek+read for every quiet subagent
        // file on every single tick, all under the tail lock. Stat
        // first and skip straight to the next entry when the file's
        // length hasn't moved since we last read it — a `stat` is far
        // cheaper than an `open`+`read`, and it helps a live-but-quiet
        // tail just as much as a finished one. A stat failure is not
        // proof there's nothing new, so it falls through to the normal
        // open path below rather than skipping — a skip must never
        // cost us data.
        if let Ok(meta) = fs::metadata(&tail.path)
            && meta.len() == tail.last_pos
        {
            continue;
        }

        let mut file = match fs::File::open(&tail.path) {
            Ok(f) => f,
            Err(e) => {
                if !tail.open_failure_logged {
                    tail.open_failure_logged = true;
                    tracing::warn!(
                        harness_id = %s.harness_id,
                        path = %tail.path.display(),
                        error = %e,
                        "claude_events: could not open subagent transcript"
                    );
                }
                continue;
            }
        };
        if tail.open_failure_logged {
            tail.open_failure_logged = false;
            tracing::debug!(
                harness_id = %s.harness_id,
                path = %tail.path.display(),
                "claude_events: subagent transcript readable again"
            );
        }
        if let Ok(meta) = file.metadata()
            && meta.len() < tail.last_pos
        {
            // #425: re-seed the way `attach_at` seeds a subagent — jump
            // to EOF, no events, no rows. Replaying from 0 would
            // re-emit rows already seen as live activity.
            // An unreadable file (a write cut mid UTF-8 char) is left
            // for the next tick: never fall back to 0, which would
            // replay this subagent live.
            let Ok(content) = fs::read_to_string(&tail.path) else {
                continue;
            };
            tail.last_pos = u64::try_from(content.len()).unwrap_or(u64::MAX);
            tail.partial.clear();
            tail.finished = subagent_content_is_finished(&content);
            tracing::info!(
                harness_id = %s.harness_id,
                path = %tail.path.display(),
                last_pos = tail.last_pos,
                file_len = meta.len(),
                "claude_events: subagent transcript shrank; re-seeded without replay"
            );
            continue;
        }
        if let Err(e) = file.seek(SeekFrom::Start(tail.last_pos)) {
            if !tail.open_failure_logged {
                tail.open_failure_logged = true;
                tracing::warn!(
                    harness_id = %s.harness_id,
                    path = %tail.path.display(),
                    error = %e,
                    "claude_events: subagent seek failed"
                );
            }
            continue;
        }
        let mut buf = String::new();
        let Ok(bytes) = file.read_to_string(&mut buf) else {
            // UTF-8 decode failed somewhere mid-file — same story as
            // the main tail's identical guard in `tick`: we landed
            // mid multi-byte char. Leave `last_pos` where it is and
            // wait for the next tick to pick up a full line; logged
            // at trace so it's diagnosable without being noisy.
            tracing::trace!(path = %tail.path.display(), "claude_events: subagent utf8 mid-line; retrying next tick");
            continue;
        };
        let advance: u64 = u64::try_from(bytes).unwrap_or(u64::MAX);
        tail.last_pos = tail.last_pos.saturating_add(advance);

        tail.partial.push_str(&buf);
        let drained = std::mem::take(&mut tail.partial);
        let mut lines = drained.split('\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_none() {
                tail.partial.push_str(line);
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };
            let row_ts = skein_harness::claude::timestamp_ms(&value);
            if !tail.started_ms_resolved {
                tail.started_ms = row_ts;
                tail.started_ms_resolved = true;
            }
            if let Some(ts) = row_ts {
                tail.last_ts_ms = ts;
            }
            if skein_harness::claude::subagent_row_is_terminal(&value) {
                if !tail.finished {
                    tail.finished = true;
                    events.push(ClaudeEvent::SubagentEnd {
                        agent_id: agent_id.clone(),
                        agent_type: tail.agent_type.clone(),
                        description: tail.description.clone(),
                    });
                    // Feed a `subagent_end` row into the activity feed
                    // (epic #298). The delegation itself already
                    // renders via the main transcript's `Agent`
                    // tool_call row (`AgentRow`, toolRows.tsx) — what's
                    // missing is the other end for a *background*
                    // subagent: its `AgentRow` lands at launch
                    // (`toolUseResult.status == "async_launched"`) and
                    // nothing ever marks completion. A second
                    // "delegated" row would just duplicate the
                    // existing one, so this is "finished" only.
                    let duration_ms = tail
                        .started_ms
                        .map(|start| tail.last_ts_ms.saturating_sub(start));
                    let mut payload = serde_json::json!({
                        "agent_id": agent_id,
                        "agent_type": tail.agent_type,
                        "description": tail.description,
                    });
                    if let Some(duration_ms) = duration_ms
                        && let Some(obj) = payload.as_object_mut()
                    {
                        obj.insert("duration_ms".into(), serde_json::json!(duration_ms));
                    }
                    subagent_end_actions.push(crate::harness_actions_claude::ExtractedAction {
                        kind: crate::db::action_kind::SUBAGENT_END,
                        timestamp_ms: tail.last_ts_ms,
                        payload: payload.to_string(),
                        source: None,
                    });
                }
            } else if tail.finished {
                // More rows arrived after a terminal one — a
                // follow-up delegation to the same id. Live again.
                // Reuses the cached `agent_type`/`description` from
                // this id's first appearance rather than re-reading
                // the `.meta.json` sidecar — on the assumption Claude
                // never changes an id's meta after the fact.
                tail.finished = false;
                events.push(ClaudeEvent::SubagentStart {
                    agent_id: agent_id.clone(),
                    agent_type: tail.agent_type.clone(),
                    description: tail.description.clone(),
                    initial: false,
                });
            }
            if is_subagent_tool_result_row(&value) {
                events.push(ClaudeEvent::SubagentToolResult {
                    agent_id: agent_id.clone(),
                });
            }
        }
    }

    // A transcript that vanished (pruned/rotated — not observed in
    // practice, but defensive symmetry with the main tail): drop it
    // from the map, no event. Nothing downstream needs to be told a
    // file disappeared; the last event it emitted already said
    // whether it was live or finished.
    s.subagents.retain(|id, _| seen.contains(id));

    tracing::debug!(
        harness_id = %s.harness_id,
        seen = seen.len(),
        new = new_count,
        "claude_events: tick_subagents"
    );

    // Persist (and, live, broadcast) any `subagent_end` rows collected
    // above — same sink and same `emit` semantics as the main tail's
    // `persist_extracted(ap, extracted, true)` call in `tick`. `None`
    // for the phase-only test constructor, in which case this is a
    // silent no-op (nothing to persist to).
    if let Some(ap) = s.actions.as_ref() {
        persist_extracted(ap, subagent_end_actions, true);
    }
}

/// Scans every complete line in `content` and returns whether the
/// *last* one is terminal (see
/// `skein_harness::claude::subagent_row_is_terminal`). Used to seed a
/// subagent's `finished` state from what's already on disk at attach
/// time, mirroring the per-row logic `tick_subagents` applies while
/// tailing live.
fn subagent_content_is_finished(content: &str) -> bool {
    let mut finished = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        finished = skein_harness::claude::subagent_row_is_terminal(&value);
    }
    finished
}

/// A `user` row whose `message.content` carries a `tool_result` block
/// — a tool call inside a subagent's own turn just returned. This is
/// what backs `ClaudeEvent::SubagentToolResult`; see its doc comment
/// for why it exists and what it must not be read as.
fn is_subagent_tool_result_row(value: &serde_json::Value) -> bool {
    if value.get("type").and_then(serde_json::Value::as_str) != Some("user") {
        return false;
    }
    value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result"))
        })
}

/// One-shot historical scan of the JSONL — runs once on attach
/// before the watcher arms, in a single walk over every line (#171e:
/// this used to be two separate walks, one for phase and one for
/// actions, each re-parsing every line). We still need to touch every
/// row in order for two independent reasons that happen to want the
/// same parsed line:
///
/// - the phase probe (`apply_initial_state_row`, same logic
///   `determine_initial_state` uses) needs the *last* relevant row,
///   not the first;
/// - the action extractor's `tool_use` buffer needs to join with its
///   result row even when they fall on different lines.
///
/// `actions` is `None` for phase-only callers/tests, in which case
/// the second element of the return is always empty. When present,
/// returns every extracted action whose timestamp is newer than the
/// largest one already persisted for this harness
/// (`max_persisted_ts_ms`) — that's how a re-attach after Skein
/// restart avoids duplicating rows. On first attach the max is 0, so
/// every row is fresh. Persistence itself is the caller's job
/// (`persist_extracted_batch`), so this function stays a pure scan.
fn scan_history(
    content: &str,
    mut actions: Option<&mut ActionPersistence>,
) -> (
    Option<ClaudeEvent>,
    Vec<crate::harness_actions_claude::ExtractedAction>,
) {
    let max_ts = actions
        .as_ref()
        .map_or(0, |ap| max_persisted_ts_ms(&ap.db, &ap.harness_id));
    let mut last: Option<ClaudeEvent> = None;
    let mut fresh = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        apply_initial_state_row(&value, &mut last);
        if let Some(ap) = actions.as_mut() {
            let extracted = ap.extractor.ingest(&value);
            fresh.extend(extracted.into_iter().filter(|a| a.timestamp_ms > max_ts));
        }
    }
    (last, fresh)
}

/// Insert every action in `extracted` into `harness_actions`. When
/// `emit` is set (live tail), broadcast each inserted row to the
/// frontend. Logs at warn on insert failure (sqlite locked, disk
/// full) but doesn't propagate — the tail loop continues; a swallowed
/// failure here previously left no trace at the default log level
/// (#362).
fn persist_extracted(
    ap: &ActionPersistence,
    extracted: Vec<crate::harness_actions_claude::ExtractedAction>,
    emit: bool,
) {
    for action in extracted {
        match ap.db.record_harness_action(
            &ap.harness_id,
            &ap.room_id,
            action.timestamp_ms,
            action.kind,
            &action.payload,
            action.source.as_deref(),
        ) {
            Ok(id) => {
                if emit {
                    // Live only. Capturing a baseline while replaying
                    // history would read a HEAD that has already moved
                    // past the edit — see review.rs's module docs.
                    if action.kind == crate::db::action_kind::PATCH {
                        crate::review::note_patch(
                            &ap.db,
                            &ap.room_id,
                            &ap.cwd,
                            &ap.harness_id,
                            &action.payload,
                        );
                    }
                    if let Some(app) = &ap.app {
                        crate::harness_action_event::emit(
                            app,
                            id,
                            &ap.harness_id,
                            &ap.room_id,
                            action.timestamp_ms,
                            action.kind,
                            &action.payload,
                            action.source.as_deref(),
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(harness_id = %ap.harness_id, kind = %action.kind, error = %e,
                    "claude_events: record_harness_action failed");
            }
        }
    }
}

/// Batch counterpart to `persist_extracted`, used by `scan_history`'s
/// backfill path (#171e): one transaction for the whole history read
/// instead of one `INSERT` per action. Backfill never emits — same
/// reasoning as `persist_extracted`'s `emit` flag — so there's no
/// per-row broadcast or review-baseline capture to replicate here.
/// A no-op for an empty `extracted` (also a no-op on the DB side, but
/// this skips the allocation).
///
/// Takes `&ActionPersistence`, not `&mut`: `extracted` already came out
/// of `scan_history`'s walk over the extractor, so this function only
/// writes rows — it never feeds the extractor itself.
fn persist_extracted_batch(
    ap: &ActionPersistence,
    extracted: Vec<crate::harness_actions_claude::ExtractedAction>,
) {
    if extracted.is_empty() {
        return;
    }
    let rows: Vec<crate::db::NewHarnessAction> = extracted
        .into_iter()
        .map(|a| crate::db::NewHarnessAction {
            timestamp_ms: a.timestamp_ms,
            kind: a.kind,
            payload: a.payload,
            source: a.source,
        })
        .collect();
    if let Err(e) = ap
        .db
        .record_harness_actions(&ap.harness_id, &ap.room_id, &rows)
    {
        // max_ts didn't advance, so the next attach retries this same batch.
        tracing::warn!(harness_id = %ap.harness_id, rows = rows.len(), error = %e,
            "claude_events: batch backfill insert failed");
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
///   `stop_reason` (`end_turn` / `stop_sequence` / `max_tokens`), or
///   the turn was last ended by an interrupt or a `turn_duration` row
///   (#260, see [`ends_turn_without_stop_reason`]).
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
///
/// Production code no longer calls this directly — `attach_at` uses
/// `scan_history`, which drives the same per-row logic
/// (`apply_initial_state_row`) from one walk shared with the action
/// extractor (#171e) instead of two. Kept as a `#[cfg(test)]` helper,
/// delegating to `scan_history` (with no `ActionPersistence`, so the
/// action side of its walk is a no-op) rather than re-implementing the
/// line walk, so the standalone "feed it a whole log, check the
/// derived phase" tests below stay simple to write.
#[cfg(test)]
fn determine_initial_state(content: &str) -> Option<ClaudeEvent> {
    scan_history(content, None).0
}

/// One row's worth of `determine_initial_state`'s logic, factored out
/// so `scan_history` can drive it from the same parsed `Value` its
/// action extractor already walked, instead of `determine_initial_state`
/// and the action scan each re-parsing every line (#171e). Mutates
/// `last` in place — same "last relevant row wins" rule the doc comment
/// on `determine_initial_state` describes.
fn apply_initial_state_row(value: &serde_json::Value, last: &mut Option<ClaudeEvent>) {
    if skein_harness::claude::is_sidechain(value) {
        return;
    }
    let Some(ty) = value.get("type").and_then(serde_json::Value::as_str) else {
        return;
    };
    // #260: same rule as the live parser, or an interrupted session
    // resumes into running on every restart.
    if ends_turn_without_stop_reason(ty, value) {
        *last = Some(ClaudeEvent::AwaitingPrompt);
        return;
    }
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
                *last = Some(ClaudeEvent::AwaitingPrompt);
            } else {
                *last = Some(ClaudeEvent::AssistantTurn);
            }
        }
        "user" => {
            *last = if value.get("toolUseResult").is_some() {
                Some(ClaudeEvent::ToolUseResult)
            } else {
                Some(ClaudeEvent::UserPrompt)
            };
        }
        // Metadata rows don't shift phase; skip.
        _ => {}
    }
}

/// Text Claude writes as a plain `user` row when the user stops a turn:
/// `[Request interrupted by user]` mid-stream, `… for tool use]` during
/// a tool call. Prefix-matched so both, and any later suffix, count.
const INTERRUPT_PREFIX: &str = "[Request interrupted by user";

/// Does this main-chain row end a turn that no terminal `stop_reason`
/// will end (#260)? Two shapes, verified against Claude Code 2.1.270:
///
/// - the interrupt row above. It is a `user` row with no
///   `toolUseResult`, so without this it reads as a fresh prompt and
///   the harness shows running until the next restart — and then again,
///   because the probe finds the same last row.
/// - `system` / `turn_duration`, written the moment any turn ends,
///   interrupted or not. A second signal, not a replacement for
///   `end_turn`: it is missing after some turns that did end cleanly.
fn ends_turn_without_stop_reason(ty: &str, value: &serde_json::Value) -> bool {
    match ty {
        "user" => {
            if value.get("toolUseResult").is_some() {
                return false;
            }
            let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
                return false;
            };
            // The first text block, or the whole content when it is a
            // bare string — an interrupt row carries nothing else.
            let text = content.as_str().or_else(|| {
                content
                    .as_array()?
                    .iter()
                    .find_map(|b| b.get("text").and_then(serde_json::Value::as_str))
            });
            text.is_some_and(|t| t.starts_with(INTERRUPT_PREFIX))
        }
        "system" => {
            value.get("subtype").and_then(serde_json::Value::as_str) == Some("turn_duration")
        }
        _ => false,
    }
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
    if skein_harness::claude::is_sidechain(value) {
        // Reset the turn flag if a sub-agent interrupts so the next
        // main-session assistant row starts a fresh turn.
        *in_assistant_turn = false;
        return None;
    }

    // #260: an interrupt or a turn_duration row ends the turn even
    // though no assistant row said so.
    if ends_turn_without_stop_reason(ty, value) {
        *in_assistant_turn = false;
        return Some(ClaudeEvent::AwaitingPrompt);
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
    use std::sync::Barrier;
    use std::sync::mpsc;
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
        ClaudeEventsManager::new_for_test(Arc::new(db))
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
                "harness-1",
                path.clone(),
                move |event| {
                    tx.send(event).unwrap();
                },
                None,
            )
            .unwrap();
        (manager, path, rx)
    }

    #[test]
    fn detach_reports_whether_an_adapter_was_removed() {
        let dir = TempDir::new().unwrap();
        let (mgr, _path, _rx) = make_adapter(&dir);

        assert!(
            !mgr.detach("no-such-harness"),
            "detach of an unknown id should report false"
        );
        assert!(
            mgr.detach("harness-1"),
            "detach of the attached id should report true"
        );
        assert!(
            !mgr.detach("harness-1"),
            "detaching the same id twice should report false the second time"
        );
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
                "h1",
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
                "h1",
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
            .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
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
            .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
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

    /// `parse_value` on one JSON row, starting outside an assistant turn.
    fn parse_one(row: &str) -> Option<ClaudeEvent> {
        let value: serde_json::Value = serde_json::from_str(row).unwrap();
        parse_value(&value, &mut false)
    }

    // #260 — the row shapes below are from a real Claude Code 2.1.270
    // transcript whose last turn was interrupted during a tool call.

    #[test]
    fn interrupt_rows_end_the_turn() {
        for row in [
            r#"{"type":"user","sessionId":"x","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#,
            r#"{"type":"user","sessionId":"x","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
            r#"{"type":"user","sessionId":"x","message":{"role":"user","content":"[Request interrupted by user]"}}"#,
        ] {
            assert!(
                matches!(parse_one(row), Some(ClaudeEvent::AwaitingPrompt)),
                "{row}"
            );
        }
    }

    #[test]
    fn interrupt_closes_an_open_assistant_turn() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"type":"user","sessionId":"x","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#,
        )
        .unwrap();
        let mut in_turn = true;
        assert!(matches!(
            parse_value(&value, &mut in_turn),
            Some(ClaudeEvent::AwaitingPrompt)
        ));
        assert!(!in_turn);
    }

    #[test]
    fn a_prompt_that_only_mentions_the_interrupt_text_is_still_a_prompt() {
        let row = r#"{"type":"user","sessionId":"x","message":{"content":[{"type":"text","text":"why did I see [Request interrupted by user]?"}]}}"#;
        assert!(matches!(parse_one(row), Some(ClaudeEvent::UserPrompt)));
        // A tool result never ends the turn, whatever its text says.
        let result = r#"{"type":"user","sessionId":"x","toolUseResult":"x","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        assert!(matches!(
            parse_one(result),
            Some(ClaudeEvent::ToolUseResult)
        ));
    }

    #[test]
    fn turn_duration_ends_the_turn_and_other_system_rows_do_not() {
        let duration = r#"{"type":"system","subtype":"turn_duration","durationMs":3158,"messageCount":44,"sessionId":"x"}"#;
        assert!(matches!(
            parse_one(duration),
            Some(ClaudeEvent::AwaitingPrompt)
        ));
        let other = r#"{"type":"system","subtype":"compact_boundary","sessionId":"x"}"#;
        assert!(parse_one(other).is_none());
    }

    #[test]
    fn determine_initial_state_resumes_an_interrupted_turn_as_waiting() {
        let log = concat!(
            r#"{"type":"user","sessionId":"x","message":{"content":"try again"}}"#,
            "\n",
            r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"ok"}]}}"#,
            "\n",
            r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","id":"t1","input":{}}]}}"#,
            "\n",
            r#"{"type":"user","sessionId":"x","toolUseResult":"Error","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
            "\n",
            r#"{"type":"user","sessionId":"x","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":3158,"sessionId":"x"}"#,
            "\n",
            r#"{"type":"cost-state","sessionId":"x","totalCostUSD":0.9}"#,
            "\n",
            r#"{"type":"last-prompt","lastPrompt":"try again","sessionId":"x"}"#,
            "\n"
        );
        let result = determine_initial_state(log);
        assert!(
            matches!(result, Some(ClaudeEvent::AwaitingPrompt)),
            "an interrupted session must not resume into running, got {result:?}"
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
        let manager = ClaudeEventsManager::new_for_test(Arc::clone(&db));
        let persistence = Some(ActionPersistence {
            extractor: ActionExtractor::new(),
            db,
            harness_id: harness_id.into(),
            room_id: room_id.into(),
            // Phase/action tests only; note_patch no-ops on an empty cwd.
            cwd: String::new(),
            app: None,
        });
        manager
            .attach_at(harness_id, jsonl, |_event| {}, persistence)
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

    // ── concurrent attach across many harnesses (#362) ──────────────
    //
    // Production shape: 5 Claude harnesses spawned within ~4 s, each
    // in a brand-new worktree, so each project dir under
    // `~/.claude/projects` did not exist yet. `claude_events_attach`
    // (lib.rs) runs every `attach` call on tokio's `spawn_blocking`
    // pool against ONE shared `ClaudeEventsManager` — i.e. concurrently,
    // on different OS threads. 4 of 5 adapters never read their
    // transcript: zero rows persisted, no events, ever. These tests
    // reproduce that shape directly against `attach_at` with plain
    // `std::thread`s released together by a `Barrier`, skipping tokio
    // entirely; one manager and one on-disk `Database` shared across
    // all harnesses, action persistence ON (production always runs
    // with it on, #80).

    /// One harness's fixture for a concurrent-attach run.
    struct ConcurrentHarness {
        harness_id: String,
        room_id: String,
        path: PathBuf,
        rx: mpsc::Receiver<ClaudeEvent>,
    }

    /// Attach `N` harnesses to `N` distinct transcript paths — each
    /// under its own never-before-seen project dir, mirroring one dir
    /// per brand-new worktree — releasing all `N` `attach_at` calls
    /// together via a `Barrier` so they race on parent-dir creation and
    /// `notify` watcher setup the way five simultaneous worktree spawns
    /// did in production. Then, from `N` more threads released the same
    /// way, mimics Claude actually writing each transcript: a user
    /// prompt row, a tool call + its result, and a terminal `end_turn`
    /// row, each write separated by a short sleep so the watcher has to
    /// fire more than once per harness — same as a real turn, and the
    /// path most likely to race a debounced tick against a fresh
    /// `attach_at` still setting up its `TailState`.
    ///
    /// `precreate_parent` / `precreate_file` select which of #362's
    /// real timings this run reproduces:
    ///   - neither: the project dir doesn't exist yet at attach time —
    ///     the production incident itself
    ///   - `precreate_parent` only: the dir exists (an earlier session
    ///     wrote there already) but this session's `.jsonl` doesn't
    ///   - both: the file already has a row in it before `attach_at`
    ///     runs — Claude won the race and wrote first
    ///
    /// Returns one `bool` per harness: whether it observed its
    /// `AwaitingPrompt` end-of-turn within a single 5 s budget shared
    /// across all `N` (not 5 s each serially, which would let a
    /// handful of stuck harnesses blow the test out to `N * 5` s).
    fn run_concurrent_attach_variant(precreate_parent: bool, precreate_file: bool) -> Vec<bool> {
        const N: usize = 6;
        let root = TempDir::new().unwrap();
        let db = Arc::new(crate::db::Database::open(&root.path().join("t.db")).unwrap());
        let manager = Arc::new(ClaudeEventsManager::new_for_test(Arc::clone(&db)));

        let mut senders = Vec::with_capacity(N);
        let mut harnesses = Vec::with_capacity(N);
        for i in 0..N {
            let harness_id = format!("h{i}");
            let room_id = format!("r{i}");
            // A distinct, never-before-seen project dir per harness.
            let parent = root.path().join("projects").join(format!("proj-{i}"));
            let path = parent.join(format!("{}.jsonl", uuid::Uuid::new_v4()));
            if precreate_parent || precreate_file {
                fs::create_dir_all(&parent).unwrap();
            }
            if precreate_file {
                let mut f = fs::File::create(&path).unwrap();
                writeln!(
                    f,
                    r#"{{"type":"user","sessionId":"s{i}","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
                )
                .unwrap();
                f.sync_all().unwrap();
            }
            let (tx, rx) = mpsc::channel();
            senders.push((harness_id.clone(), room_id.clone(), path.clone(), tx));
            harnesses.push(ConcurrentHarness {
                harness_id,
                room_id,
                path,
                rx,
            });
        }

        // Attach all N together, released by a barrier — the race is
        // inside `attach_at` (parent-dir creation, `notify` watcher
        // setup), not in anything downstream.
        let attach_barrier = Arc::new(Barrier::new(N));
        let attach_results: Vec<Result<AttachInfo, ClaudeEventsError>> = thread::scope(|scope| {
            let handles: Vec<_> = senders
                .into_iter()
                .map(|(harness_id, room_id, path, tx)| {
                    let manager = Arc::clone(&manager);
                    let db = Arc::clone(&db);
                    let barrier = Arc::clone(&attach_barrier);
                    scope.spawn(move || {
                        let persistence = Some(ActionPersistence {
                            extractor: ActionExtractor::new(),
                            db,
                            harness_id: harness_id.clone(),
                            room_id,
                            cwd: String::new(),
                            app: None,
                        });
                        barrier.wait();
                        manager.attach_at(
                            &harness_id,
                            path,
                            move |e| {
                                let _ = tx.send(e);
                            },
                            persistence,
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for (i, result) in attach_results.iter().enumerate() {
            assert!(
                result.is_ok(),
                "harness {i} failed to attach: {:?}",
                result.as_ref().err().map(ToString::to_string)
            );
        }

        // Now mimic Claude actually writing the transcript.
        let write_barrier = Arc::new(Barrier::new(N));
        thread::scope(|scope| {
            for (i, h) in harnesses.iter().enumerate() {
                let path = h.path.clone();
                let barrier = Arc::clone(&write_barrier);
                scope.spawn(move || {
                    barrier.wait();
                    if !precreate_file {
                        // #362: `attach_at` no longer pre-creates the
                        // project dir (that was Skein inventing a
                        // directory Claude might never write to) — so
                        // the write side of this fixture has to do what
                        // Claude itself does, create its own project
                        // dir lazily on first write, or this `File::
                        // create` fails outright when `precreate_parent`
                        // is also false.
                        fs::create_dir_all(path.parent().unwrap()).unwrap();
                        let mut f = fs::File::create(&path).unwrap();
                        writeln!(
                            f,
                            r#"{{"type":"user","sessionId":"s{i}","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
                        )
                        .unwrap();
                        f.sync_all().unwrap();
                    }
                    thread::sleep(Duration::from_millis(120));
                    {
                        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                        writeln!(
                            f,
                            r#"{{"type":"assistant","uuid":"a{i}","timestamp":"2026-05-15T21:16:{:02}.000Z","message":{{"content":[{{"type":"tool_use","id":"toolu_{i}","name":"Bash","input":{{"command":"echo hi"}}}}]}}}}"#,
                            20 + i
                        )
                        .unwrap();
                        f.sync_all().unwrap();
                    }
                    thread::sleep(Duration::from_millis(120));
                    {
                        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                        writeln!(
                            f,
                            r#"{{"type":"user","timestamp":"2026-05-15T21:16:{:02}.000Z","toolUseResult":{{"stdout":"hi"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_{i}","content":"hi","is_error":false}}]}}}}"#,
                            40 + i
                        )
                        .unwrap();
                        f.sync_all().unwrap();
                    }
                    thread::sleep(Duration::from_millis(120));
                    {
                        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                        writeln!(
                            f,
                            r#"{{"type":"assistant","sessionId":"s{i}","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
                        )
                        .unwrap();
                        f.sync_all().unwrap();
                    }
                });
            }
        });

        // Poll every harness's receiver in round-robin against ONE
        // shared 5 s deadline. Also drives `supervise_once` every
        // iteration (#362): with fresh parent dirs, the watch armed at
        // attach time is the nearest existing ancestor, not the real
        // project dir — production's background thread is what
        // notices the real dir appear and re-arms onto it (see
        // `Adapter::rearm`); `new_for_test` spawns no such thread, so
        // this loop stands in for it, at a much tighter interval than
        // `SUPERVISE_INTERVAL` so the fixture doesn't need anywhere
        // near this test's 5 s budget to converge.
        let mut satisfied = vec![false; N];
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && satisfied.iter().any(|s| !s) {
            manager.supervise_once();
            for (i, h) in harnesses.iter().enumerate() {
                if satisfied[i] {
                    continue;
                }
                while let Ok(event) = h.rx.try_recv() {
                    if matches!(event, ClaudeEvent::AwaitingPrompt) {
                        satisfied[i] = true;
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        // Action persistence (#80): every harness that reached
        // AwaitingPrompt must also have its tool-call row recorded.
        for (i, h) in harnesses.iter().enumerate() {
            if !satisfied[i] {
                continue;
            }
            let rows = db
                .recent_harness_actions_by_room(&h.room_id, -1, 100)
                .unwrap();
            assert!(
                rows.iter().any(|a| a.harness_id == h.harness_id),
                "harness {i} reached AwaitingPrompt but persisted no action row"
            );
        }

        satisfied
    }

    #[test]
    fn concurrent_attach_fresh_parent_dirs_all_succeed() {
        let satisfied = run_concurrent_attach_variant(false, false);
        assert!(
            satisfied.iter().all(|s| *s),
            "not every harness completed its turn (index -> ok): {:?}",
            satisfied.iter().enumerate().collect::<Vec<_>>()
        );
    }

    #[test]
    fn concurrent_attach_existing_parent_missing_file_all_succeed() {
        let satisfied = run_concurrent_attach_variant(true, false);
        assert!(
            satisfied.iter().all(|s| *s),
            "not every harness completed its turn (index -> ok): {:?}",
            satisfied.iter().enumerate().collect::<Vec<_>>()
        );
    }

    #[test]
    fn concurrent_attach_preexisting_file_all_succeed() {
        let satisfied = run_concurrent_attach_variant(true, true);
        assert!(
            satisfied.iter().all(|s| *s),
            "not every harness completed its turn (index -> ok): {:?}",
            satisfied.iter().enumerate().collect::<Vec<_>>()
        );
    }

    // ── subagent tailing (#276) ────────────────────────────────────

    /// Like `make_adapter`, but the main transcript already has one
    /// row on disk before `attach_at` runs. Subagent-tailing tests
    /// want `attached == true` from the start — otherwise `tick`
    /// returns before it ever reaches `tick_subagents` (see the early
    /// `if !s.attached` return), and a subagents-dir-only change
    /// would silently be missed until the main file also gets its
    /// first write. Production sessions always write the main file
    /// before delegating, so this mirrors reality, not just the test.
    fn make_adapter_with_existing_main(
        dir: &TempDir,
    ) -> (ClaudeEventsManager, PathBuf, mpsc::Receiver<ClaudeEvent>) {
        let path = dir.path().join("session.jsonl");
        {
            let mut f = fs::File::create(&path).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","sessionId":"x","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }
        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at(
                "harness-1",
                path.clone(),
                move |event| {
                    tx.send(event).unwrap();
                },
                None,
            )
            .unwrap();
        (manager, path, rx)
    }

    #[test]
    fn subagent_start_emitted_with_meta_from_sidecar() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        // Drain the main-transcript bootstrap event before we care
        // about subagent ones.
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: the subagents dir didn't exist at attach time, so
        // nothing is watching it yet — `supervise_once` stands in for
        // the background thread the production manager runs every
        // `SUPERVISE_INTERVAL`, arming the watch now that the dir is
        // here.
        mgr.supervise_once();
        fs::write(
            sub_dir.join("agent-a1.meta.json"),
            r#"{"agentType":"explore","description":"Map the tailer"}"#,
        )
        .unwrap();
        let mut f = fs::File::create(sub_dir.join("agent-a1.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClaudeEvent::SubagentStart { agent_id, agent_type, description, initial }
                    if agent_id == "a1"
                        && agent_type.as_deref() == Some("explore")
                        && description.as_deref() == Some("Map the tailer")
                        && !initial
            )),
            "expected a live (non-initial) SubagentStart with sidecar meta, got {events:?}"
        );
    }

    #[test]
    fn subagent_start_emitted_without_sidecar() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: arm the watch on the just-created dir — see the sibling
        // test above for why this is needed.
        mgr.supervise_once();
        // No .meta.json sidecar written for this one.
        let mut f = fs::File::create(sub_dir.join("agent-a2.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a2","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClaudeEvent::SubagentStart { agent_id, agent_type, description, initial }
                    if agent_id == "a2" && agent_type.is_none() && description.is_none() && !initial
            )),
            "expected a live (non-initial) SubagentStart with no metadata, got {events:?}"
        );
    }

    #[test]
    fn subagent_end_emitted_once_not_per_tick() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: arm the watch on the just-created dir.
        mgr.supervise_once();
        let mut f = fs::File::create(sub_dir.join("agent-a3.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a3","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        let end_count = events
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a3"))
            .count();
        assert_eq!(
            end_count, 1,
            "expected exactly one SubagentEnd, got {events:?}"
        );

        // Force another tick that has nothing new to say about the
        // subagent — append to the main transcript, which shares the
        // same debouncer/callback as the subagents dir.
        {
            let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                mf,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
            )
            .unwrap();
            mf.sync_all().unwrap();
        }

        let more = drain(&rx);
        assert!(
            !more.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a3")
            ),
            "SubagentEnd re-fired on a later tick, got {more:?}"
        );
    }

    /// `tick_subagents` stats a subagent file before opening it and
    /// skips the open/seek/read entirely when nothing has grown since
    /// `last_pos` — the skip must never wedge the tail. A no-growth
    /// tick produces no duplicate events, and a later tick where the
    /// file DOES grow is still read correctly.
    #[test]
    fn subagent_tail_skip_on_no_growth_does_not_wedge_later_reads() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: arm the watch on the just-created dir.
        mgr.supervise_once();
        let sub_path = sub_dir.join("agent-a6.jsonl");
        {
            let mut f = fs::File::create(&sub_path).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","isSidechain":true,"agentId":"a6","message":{{"stop_reason":"tool_use","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }

        let events = drain(&rx);
        assert!(
            events.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "a6")
            ),
            "expected SubagentStart for a6, got {events:?}"
        );

        // A tick with nothing new for the subagent — append only to
        // the main transcript, which shares the same debouncer/
        // callback and still drives `tick_subagents` once per tick.
        {
            let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                mf,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
            )
            .unwrap();
            mf.sync_all().unwrap();
        }
        let quiet = drain(&rx);
        assert!(
            !quiet.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "a6")
            ),
            "SubagentStart re-fired on a no-growth tick, got {quiet:?}"
        );

        // The subagent file DOES grow now — a stat-skip on the
        // previous tick must not have wedged `last_pos` such that this
        // new row goes unread.
        {
            let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","isSidechain":true,"agentId":"a6","message":{{"stop_reason":"end_turn","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }
        let after_growth = drain(&rx);
        let end_count = after_growth
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a6"))
            .count();
        assert_eq!(
            end_count, 1,
            "expected exactly one SubagentEnd after the file grew again, got {after_growth:?}"
        );
    }

    /// A live subagent reaching a terminal row writes exactly one
    /// `subagent_end` `harness_actions` row, with the expected payload
    /// fields including a `duration_ms` computed from the first row's
    /// timestamp on this transcript.
    #[test]
    fn subagent_end_persists_one_action_with_duration() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let db_path = dir.path().join("test.db");

        // `tick()` only reaches `tick_subagents` once the main
        // transcript exists and the initial attach probe has seen it
        // (see `!s.attached` in `tick`) — a session with no main file
        // yet never ticks at all, subagents included. Give it one row
        // so the watcher has something to attach to, mirroring
        // `main_transcript_events_unaffected_by_a_subagents_dir`.
        let _manager = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
        {
            let mut f = fs::File::create(&jsonl).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","timestamp":"2026-05-15T21:16:19.000Z","message":{{"stop_reason":"tool_use","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }

        let sub_dir = skein_harness::claude::subagents_dir(&jsonl).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(
            sub_dir.join("agent-a1.meta.json"),
            r#"{"agentType":"explore","description":"Map the tailer"}"#,
        )
        .unwrap();
        let mut f = fs::File::create(sub_dir.join("agent-a1.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","timestamp":"2026-05-15T21:16:20.000Z","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","timestamp":"2026-05-15T21:16:25.000Z","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        // Give the debouncer a moment to tick and persist.
        thread::sleep(Duration::from_secs(2));

        let db = crate::db::Database::open(&db_path).unwrap();
        let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        let ends: Vec<_> = actions
            .iter()
            .filter(|a| a.kind == crate::db::action_kind::SUBAGENT_END)
            .collect();
        assert_eq!(
            ends.len(),
            1,
            "expected exactly one subagent_end row, got {actions:?}"
        );
        let payload: serde_json::Value = serde_json::from_str(&ends[0].payload).unwrap();
        assert_eq!(payload["agent_id"], "a1");
        assert_eq!(payload["agent_type"], "explore");
        assert_eq!(payload["description"], "Map the tailer");
        assert_eq!(payload["duration_ms"], 5000);
    }

    /// A subagent already finished on disk at attach time must not get
    /// a synthetic `subagent_end` action — mirrors
    /// `attach_emits_start_only_for_the_unfinished_subagent`'s phase
    /// assertion, but for the persisted action row.
    #[test]
    fn attach_does_not_persist_subagent_end_for_already_finished_subagent() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let db_path = dir.path().join("test.db");
        {
            let mut f = fs::File::create(&jsonl).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }
        let sub_dir = skein_harness::claude::subagents_dir(&jsonl).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // Finished before attach — last row has a terminal stop_reason.
        fs::write(
            sub_dir.join("agent-fin.jsonl"),
            "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"fin\",\"timestamp\":\"2026-05-15T21:16:20.000Z\",\"message\":{\"stop_reason\":\"end_turn\",\"content\":[]}}\n",
        )
        .unwrap();

        let _manager = make_persisting_adapter(jsonl, &db_path, "h1", "r1");
        thread::sleep(Duration::from_secs(2));

        let db = crate::db::Database::open(&db_path).unwrap();
        let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        assert!(
            !actions
                .iter()
                .any(|a| a.kind == crate::db::action_kind::SUBAGENT_END),
            "attach must not synthesize a subagent_end action for an already-finished subagent, got {actions:?}"
        );
    }

    #[test]
    fn subagent_tool_result_emitted_for_tool_result_row() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: arm the watch on the just-created dir.
        mgr.supervise_once();
        let mut f = fs::File::create(sub_dir.join("agent-a4.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","isSidechain":true,"agentId":"a4","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"ok"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        let events = drain(&rx);
        assert!(
            events.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentToolResult { agent_id } if agent_id == "a4")
            ),
            "expected SubagentToolResult, got {events:?}"
        );
    }

    #[test]
    fn attach_emits_start_only_for_the_unfinished_subagent() {
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
        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // Finished before attach — last row has a terminal stop_reason.
        fs::write(
            sub_dir.join("agent-fin.jsonl"),
            "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"fin\",\"message\":{\"stop_reason\":\"end_turn\",\"content\":[]}}\n",
        )
        .unwrap();
        // Still running — last row has no terminal stop_reason.
        fs::write(
            sub_dir.join("agent-live.jsonl"),
            "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"live\",\"message\":{\"stop_reason\":\"tool_use\",\"content\":[]}}\n",
        )
        .unwrap();

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
            .unwrap();

        let events = drain_brief(&rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClaudeEvent::SubagentStart { agent_id, initial, .. }
                    if agent_id == "live" && *initial
            )),
            "expected an initial SubagentStart for the unfinished subagent, got {events:?}"
        );
        assert!(
            !events.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "fin")
            ),
            "the already-finished subagent must not get a synthetic SubagentStart, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::SubagentEnd { .. })),
            "attach should not synthesize a SubagentEnd for a subagent that was already finished, got {events:?}"
        );
    }

    #[test]
    fn subagent_line_split_across_two_ticks_is_reassembled() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: arm the watch on the just-created dir.
        mgr.supervise_once();
        let sub_path = sub_dir.join("agent-a5.jsonl");
        {
            let mut f = fs::File::create(&sub_path).unwrap();
            f.write_all(
                br#"{"type":"assistant","isSidechain":true,"agentId":"a5","message":{"stop_reason":"#,
            )
            .unwrap();
            f.sync_all().unwrap();
        }
        thread::sleep(Duration::from_millis(150));
        {
            let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
            f.write_all(b"\"end_turn\",\"content\":[]}}\n").unwrap();
            f.sync_all().unwrap();
        }

        let events = drain(&rx);
        let end_count = events
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a5"))
            .count();
        assert_eq!(
            end_count, 1,
            "expected the reassembled line to parse into exactly one SubagentEnd, got {events:?}"
        );
    }

    #[test]
    fn main_transcript_events_unaffected_by_a_subagents_dir() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter(&dir);

        // A session that has already delegated: subagents dir exists
        // with a live transcript in it before the main file gets its
        // first (and, here, only) row.
        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(
            sub_dir.join("agent-a9.jsonl"),
            "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"a9\",\"message\":{\"stop_reason\":\"tool_use\",\"content\":[]}}\n",
        )
        .unwrap();

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
            "main-transcript AwaitingPrompt must still fire with a subagents dir present, got {events:?}"
        );
    }

    /// #362: `TailState::first_read_logged` flips exactly once, on the
    /// first tick that actually reads bytes off the main transcript —
    /// not on an empty-file tick, and not again on a later tick once
    /// it's already true. Drives `tick` directly against a hand-built
    /// `TailState` rather than through `attach_at`'s watcher, since
    /// what's under test is the flag flip, not the debouncer plumbing;
    /// no tracing-subscriber harness needed because the assertion is
    /// on the state field the log line guards, not on emitted output.
    #[test]
    fn first_read_logged_flips_once_per_attach() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, "").unwrap();
        let state = Arc::new(Mutex::new(TailState {
            harness_id: "h".into(),
            path: path.clone(),
            last_pos: 0,
            partial: String::new(),
            attached: true,
            ever_attached: true,
            fingerprint: Vec::new(),
            in_assistant_turn: false,
            actions: None,
            subagents_dir: None,
            subagents: HashMap::new(),
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }));

        // Nothing written yet — a tick that reads zero bytes must not
        // flip the flag.
        tick(&state, &|_| {});
        assert!(!state.lock().first_read_logged);

        // Append a row — this tick's read is the first non-empty one.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, r#"{{"type":"user"}}"#).unwrap();
        f.sync_all().unwrap();

        tick(&state, &|_| {});
        assert!(state.lock().first_read_logged);

        // A later empty tick must leave it set, not toggle it back.
        tick(&state, &|_| {});
        assert!(state.lock().first_read_logged);
    }

    /// #362: `events_sent` must count only events actually handed to
    /// `on_event`, not everything a tick queued — a panic partway
    /// through the dispatch loop must not credit events that were
    /// never delivered. Two rows queue two events; the callback panics
    /// on the second call, so only the first should be counted, and
    /// the panic itself must still land in `send_errors`.
    #[test]
    fn events_sent_counts_only_events_dispatched_before_a_panic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,
                "\n",
            ),
        )
        .unwrap();
        let state = Arc::new(Mutex::new(TailState {
            harness_id: "h".into(),
            path: path.clone(),
            last_pos: 0,
            partial: String::new(),
            attached: true,
            ever_attached: true,
            fingerprint: Vec::new(),
            in_assistant_turn: false,
            actions: None,
            subagents_dir: None,
            subagents: HashMap::new(),
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }));

        let calls = std::sync::atomic::AtomicUsize::new(0);
        tick(&state, &|_event| {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(n != 1, "boom");
        });

        let s = state.lock();
        assert_eq!(
            s.events_sent, 1,
            "only the event dispatched before the panic should be counted, got {}",
            s.events_sent
        );
        assert_eq!(s.send_errors, 1);
    }

    // ── #362: heartbeat, sidecar discovery, utf8 stall ─────────────

    /// The headline #362 scenario: a subagent sidecar appears after
    /// attach, and the main transcript's own events must keep flowing
    /// exactly as before — a sidecar joining the tailed set must never
    /// starve `tick`'s main-file half.
    #[test]
    fn main_events_keep_flowing_after_sidecars_appear_post_attach() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: the dir didn't exist at attach, so a supervisor pass is
        // what notices it and arms the watch — production's background
        // thread does this on its own timer; `supervise_once` stands in
        // for it here.
        mgr.supervise_once();
        fs::write(
            sub_dir.join("agent-s1.meta.json"),
            r#"{"agentType":"explore","description":"Look around"}"#,
        )
        .unwrap();
        let mut sf = fs::File::create(sub_dir.join("agent-s1.jsonl")).unwrap();
        writeln!(
            sf,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"s1","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        sf.sync_all().unwrap();
        // Every handle is closed before its drain. macOS FSEvents reports
        // a content change when the writer *closes* the file, not per
        // write, so an append through a still-open handle is invisible
        // there until it drops — Linux and Windows report each write.
        // Claude Code itself closes after every append (no running
        // `claude` holds its transcript open), so closing is also what
        // mirrors production.
        drop(sf);

        let events = drain(&rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClaudeEvent::SubagentStart { agent_id, initial, .. }
                    if agent_id == "s1" && !initial
            )),
            "expected a live SubagentStart for s1, got {events:?}"
        );

        // The main transcript must keep producing its own events with
        // a subagent now mid-flight.
        let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            mf,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Read"}}]}}}}"#
        )
        .unwrap();
        mf.sync_all().unwrap();
        drop(mf);

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AssistantTurn)),
            "expected the main transcript's tool-use row to still fire AssistantTurn, got {events:?}"
        );

        let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            mf,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        mf.sync_all().unwrap();
        drop(mf);

        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected the main transcript's end_turn row to still fire AwaitingPrompt, got {events:?}"
        );

        let mut sf = fs::OpenOptions::new()
            .append(true)
            .open(sub_dir.join("agent-s1.jsonl"))
            .unwrap();
        writeln!(
            sf,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"s1","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        sf.sync_all().unwrap();
        drop(sf);

        let events = drain(&rx);
        assert!(
            events.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "s1")
            ),
            "expected SubagentEnd for s1, got {events:?}"
        );
    }

    /// Stress variant: five subagents churn (appear, run, finish) while
    /// the main transcript writes its own turns, all under one attach.
    /// Every main row must still produce its event and every subagent
    /// must still get exactly one Start and one End — bounded by one
    /// shared wall-clock deadline so a real regression (a stuck tail)
    /// fails the test instead of hanging it.
    #[test]
    fn main_and_subagent_events_survive_concurrent_sidecar_churn() {
        const MAIN_TURNS: usize = 5;
        const SUBAGENTS: usize = 5;

        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        // #362: arm the watch on the just-created dir before the churn
        // threads start writing into it.
        mgr.supervise_once();

        let barrier = Arc::new(Barrier::new(1 + SUBAGENTS));

        let main_path = path.clone();
        let main_barrier = Arc::clone(&barrier);
        let main_thread = thread::spawn(move || {
            main_barrier.wait();
            for i in 0..MAIN_TURNS {
                let mut f = fs::OpenOptions::new()
                    .append(true)
                    .open(&main_path)
                    .unwrap();
                writeln!(
                    f,
                    r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Tool{i}"}}]}}}}"#
                )
                .unwrap();
                f.sync_all().unwrap();
                thread::sleep(Duration::from_millis(15));
                let mut f = fs::OpenOptions::new()
                    .append(true)
                    .open(&main_path)
                    .unwrap();
                writeln!(
                    f,
                    r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
                )
                .unwrap();
                f.sync_all().unwrap();
                thread::sleep(Duration::from_millis(15));
            }
        });

        let mut sub_threads = Vec::new();
        for i in 0..SUBAGENTS {
            let sub_dir = sub_dir.clone();
            let sub_barrier = Arc::clone(&barrier);
            sub_threads.push(thread::spawn(move || {
                sub_barrier.wait();
                thread::sleep(Duration::from_millis(5 * i as u64));
                let sub_path = sub_dir.join(format!("agent-s{i}.jsonl"));
                fs::write(
                    &sub_path,
                    format!(
                        "{{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"s{i}\",\"message\":{{\"stop_reason\":\"end_turn\",\"content\":[]}}}}\n"
                    ),
                )
                .unwrap();
            }));
        }

        main_thread.join().unwrap();
        for h in sub_threads {
            h.join().unwrap();
        }

        let mut events = Vec::new();
        let mut assistant_turns = 0usize;
        let mut awaiting_prompts = 0usize;
        let mut started: HashSet<String> = HashSet::new();
        let mut ended: HashSet<String> = HashSet::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok(ev) = rx.recv_timeout(remaining.min(Duration::from_millis(200))) else {
                continue;
            };
            match &ev {
                ClaudeEvent::AssistantTurn => assistant_turns += 1,
                ClaudeEvent::AwaitingPrompt => awaiting_prompts += 1,
                ClaudeEvent::SubagentStart { agent_id, .. } => {
                    started.insert(agent_id.clone());
                }
                ClaudeEvent::SubagentEnd { agent_id, .. } => {
                    ended.insert(agent_id.clone());
                }
                _ => {}
            }
            events.push(ev);
            if assistant_turns >= MAIN_TURNS
                && awaiting_prompts >= MAIN_TURNS
                && started.len() >= SUBAGENTS
                && ended.len() >= SUBAGENTS
            {
                break;
            }
        }

        assert_eq!(
            assistant_turns, MAIN_TURNS,
            "lost a main AssistantTurn under sidecar churn, got {events:?}"
        );
        assert_eq!(
            awaiting_prompts, MAIN_TURNS,
            "lost a main AwaitingPrompt under sidecar churn, got {events:?}"
        );
        assert_eq!(
            started.len(),
            SUBAGENTS,
            "lost a SubagentStart under sidecar churn, got {events:?}"
        );
        assert_eq!(
            ended.len(),
            SUBAGENTS,
            "lost a SubagentEnd under sidecar churn, got {events:?}"
        );
    }

    // ── #362: no eager creation, and re-arming a deleted+recreated
    //    watched dir ─────────────────────────────────────────────────

    /// A subagents dir that simply doesn't exist yet — the common case
    /// now that `attach_at` no longer pre-creates it, since most
    /// sessions never delegate at all — must not trip the one-shot
    /// "could not read subagents dir" warn guard. That guard is for a
    /// REAL failure (permissions, say), not "this session hasn't
    /// delegated"; tripping it here would warn once per harness that
    /// never even uses subagents.
    #[test]
    fn missing_subagents_dir_does_not_trip_the_read_failure_guard() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, "").unwrap();
        // Deliberately never created.
        let sub_dir = dir.path().join("session").join("subagents");
        let state = Arc::new(Mutex::new(TailState {
            harness_id: "h".into(),
            path,
            last_pos: 0,
            partial: String::new(),
            attached: true,
            ever_attached: true,
            fingerprint: Vec::new(),
            in_assistant_turn: false,
            actions: None,
            subagents_dir: Some(sub_dir),
            subagents: HashMap::new(),
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }));

        tick(&state, &|_| {});
        assert!(
            !state.lock().subagents_dir_read_failure_logged,
            "a merely-nonexistent subagents dir must not trip the read-failure guard"
        );
    }

    /// Windows can briefly refuse to delete or recreate a directory a
    /// live `ReadDirectoryChangesW` watch still holds a handle open on
    /// — the delete lands in a "pending delete" state until the handle
    /// closes. Retries both directions of the #362 repro below rather
    /// than assume either side succeeds on the first try.
    fn retry_fs_op(mut op: impl FnMut() -> std::io::Result<()>, what: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match op() {
                Ok(()) => return,
                Err(e) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                    let _ = e;
                }
                Err(e) => panic!("{what} still failing after retrying for 2s: {e}"),
            }
        }
    }

    /// `attach_at` must not create either directory Claude owns — not
    /// the project dir the transcript lives in, and not the
    /// `subagents` dir next to it. Once Claude (simulated here)
    /// creates the project dir and writes the first row, a
    /// `supervise_once` pass must still find and tail it — nothing
    /// about not pre-creating the dir should cost a fresh spawn its
    /// first events.
    #[test]
    fn attach_does_not_create_claude_owned_dirs() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("projects").join("proj-x");
        let path = parent.join("session.jsonl");

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at(
                "h1",
                path.clone(),
                move |e| {
                    let _ = tx.send(e);
                },
                None,
            )
            .unwrap();

        assert!(
            !parent.exists(),
            "attach must not create the project dir Claude owns"
        );
        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        assert!(
            !sub_dir.exists(),
            "attach must not create the subagents dir Claude owns"
        );

        // Claude "arrives": creates its own project dir and writes the
        // first row.
        fs::create_dir_all(&parent).unwrap();
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        manager.supervise_once();
        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected AwaitingPrompt once the project dir and file appeared, got {events:?}"
        );
    }

    /// The #362 repro itself: a watched directory disappears out from
    /// under the live watch and comes back — on Windows,
    /// `ReadDirectoryChangesW` on a deleted directory just stops
    /// delivering, silently, so nothing short of a supervisor pass
    /// noticing the identity changed will ever tail it again. Exercises
    /// both the main transcript's parent and the subagents dir.
    #[test]
    fn tail_survives_watched_dir_deleted_and_recreated() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("proj");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("session.jsonl");

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at(
                "h1",
                path.clone(),
                move |e| {
                    let _ = tx.send(e);
                },
                None,
            )
            .unwrap();

        retry_fs_op(
            || fs::remove_dir_all(&parent),
            "remove_dir_all(main parent)",
        );
        retry_fs_op(
            || fs::create_dir_all(&parent),
            "create_dir_all(main parent)",
        );
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        manager.supervise_once();
        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "tail did not survive the main parent dir being deleted and recreated, got {events:?}"
        );

        // Same story, one level down: the subagents dir.
        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        manager.supervise_once();
        drain_brief(&rx); // nothing expected yet — just let the arm settle

        retry_fs_op(
            || fs::remove_dir_all(&sub_dir),
            "remove_dir_all(subagents dir)",
        );
        retry_fs_op(
            || fs::create_dir_all(&sub_dir),
            "create_dir_all(subagents dir)",
        );
        let mut sf = fs::File::create(sub_dir.join("agent-x.jsonl")).unwrap();
        writeln!(
            sf,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        sf.sync_all().unwrap();

        manager.supervise_once();
        let events = drain(&rx);
        assert!(
            events.iter().any(
                |e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "x")
            ),
            "subagent tail did not survive its dir being deleted and recreated, got {events:?}"
        );
    }

    // ── #425: a reappearing transcript is never replayed as live ────

    const T425_PROMPT: &str =
        r#"{"type":"user","timestamp":"2026-05-15T21:16:21.000Z","message":{"content":"go"}}"#;
    const T425_TOOL: &str = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#;
    const T425_RESULT: &str = r#"{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{"stdout":"x"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}]}}"#;
    const T425_END: &str = r#"{"type":"assistant","timestamp":"2026-05-15T21:16:24.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"done-A"}]}}"#;
    const T425_NEW: &str =
        r#"{"type":"user","timestamp":"2026-05-15T21:16:30.000Z","message":{"content":"more"}}"#;

    fn t425_jsonl(rows: &[&str]) -> String {
        let mut out = rows.join("\n");
        out.push('\n');
        out
    }

    /// A never-attached state on `path` (file may or may not exist yet),
    /// driven directly through `tick`.
    fn t425_state(path: &Path, actions: Option<ActionPersistence>) -> Arc<Mutex<TailState>> {
        Arc::new(Mutex::new(TailState {
            harness_id: "h1".into(),
            path: path.to_path_buf(),
            last_pos: 0,
            partial: String::new(),
            attached: false,
            ever_attached: false,
            fingerprint: Vec::new(),
            in_assistant_turn: false,
            actions,
            subagents_dir: None,
            subagents: HashMap::new(),
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }))
    }

    fn t425_tick(state: &Arc<Mutex<TailState>>) -> Vec<ClaudeEvent> {
        let out = std::sync::Mutex::new(Vec::new());
        tick(state, &|e| out.lock().unwrap().push(e));
        out.into_inner().unwrap()
    }

    fn t425_persistence(db_path: &Path) -> (Arc<crate::db::Database>, ActionPersistence) {
        let db = Arc::new(crate::db::Database::open(db_path).unwrap());
        let ap = ActionPersistence {
            extractor: ActionExtractor::new(),
            db: Arc::clone(&db),
            harness_id: "h1".into(),
            room_id: "r1".into(),
            cwd: String::new(),
            app: None,
        };
        (db, ap)
    }

    /// Attach live on the base transcript, then make it vanish
    /// (`SessionEnd`), leaving the state ready for a reappearance.
    fn t425_vanished(
        dir: &TempDir,
        with_db: bool,
    ) -> (
        Arc<Mutex<TailState>>,
        PathBuf,
        Option<Arc<crate::db::Database>>,
    ) {
        let path = dir.path().join("session.jsonl");
        let (db, ap) = if with_db {
            let (db, ap) = t425_persistence(&dir.path().join("t.db"));
            (Some(db), Some(ap))
        } else {
            (None, None)
        };
        let state = t425_state(&path, ap);
        fs::write(
            &path,
            t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END]),
        )
        .unwrap();
        let first = t425_tick(&state);
        assert_eq!(first.len(), 4, "first appearance reads live, got {first:?}");
        fs::remove_file(&path).unwrap();
        let ended = t425_tick(&state);
        assert!(matches!(ended.as_slice(), [ClaudeEvent::SessionEnd]));
        assert!(!state.lock().attached);
        (state, path, db)
    }

    #[test]
    fn reappear_same_reads_only_the_new_rows() {
        let dir = TempDir::new().unwrap();
        let (state, path, db) = t425_vanished(&dir, true);
        let db = db.unwrap();
        let rows_before = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        assert!(!rows_before.is_empty());

        fs::write(
            &path,
            t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END, T425_NEW]),
        )
        .unwrap();
        let events = t425_tick(&state);
        assert!(
            matches!(events.as_slice(), [ClaudeEvent::UserPrompt]),
            "only the appended row may be read live, got {events:?}"
        );
        let rows_after = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
        assert_eq!(
            rows_after.len(),
            rows_before.len(),
            "no duplicated harness_actions rows"
        );
        let len = fs::metadata(&path).unwrap().len();
        assert_eq!(state.lock().last_pos, len);
    }

    #[test]
    fn reappear_truncated_resyncs_as_backfill() {
        let dir = TempDir::new().unwrap();
        let (state, path, db) = t425_vanished(&dir, true);
        let db = db.unwrap();
        let rows_before = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();

        fs::write(&path, t425_jsonl(&[T425_PROMPT, T425_END])).unwrap();
        let events = t425_tick(&state);
        assert!(
            matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
            "expected only the derived initial phase, got {events:?}"
        );
        let len = fs::metadata(&path).unwrap().len();
        {
            let s = state.lock();
            assert_eq!(s.last_pos, len);
            assert!(s.partial.is_empty());
        }
        assert_eq!(
            db.recent_harness_actions_by_room("r1", -1, 100)
                .unwrap()
                .len(),
            rows_before.len()
        );
        // Tailing continues normally from the resynced position.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{T425_NEW}").unwrap();
        f.sync_all().unwrap();
        let events = t425_tick(&state);
        assert!(matches!(events.as_slice(), [ClaudeEvent::UserPrompt]));
    }

    #[test]
    fn reappear_different_content_resyncs_as_backfill() {
        let dir = TempDir::new().unwrap();
        let (state, path, _db) = t425_vanished(&dir, true);
        let old_len = state.lock().last_pos;

        // Longer than before, but not the same bytes.
        let other_end = T425_END.replace("done-A", "done-B");
        let content = t425_jsonl(&[
            T425_PROMPT,
            T425_TOOL,
            T425_RESULT,
            T425_NEW,
            T425_PROMPT,
            &other_end,
        ]);
        assert!(content.len() as u64 > old_len);
        fs::write(&path, &content).unwrap();
        let events = t425_tick(&state);
        assert!(
            matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
            "expected only the derived initial phase, got {events:?}"
        );
        assert_eq!(state.lock().last_pos, content.len() as u64);
    }

    /// A partial line carried across the vanish is completed by the
    /// bytes appended to the recreated (same) file: read live, once.
    #[test]
    fn reappear_same_completes_a_carried_partial_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let state = t425_state(&path, None);
        let (head, tail) = T425_END.split_at(40);
        let before = format!(
            "{T425_PROMPT}
{head}"
        );
        fs::write(&path, &before).unwrap();
        let first = t425_tick(&state);
        assert!(matches!(first.as_slice(), [ClaudeEvent::UserPrompt]));
        assert!(!state.lock().partial.is_empty());
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            t425_tick(&state).as_slice(),
            [ClaudeEvent::SessionEnd]
        ));

        fs::write(
            &path,
            format!(
                "{before}{tail}
"
            ),
        )
        .unwrap();
        let events = t425_tick(&state);
        assert!(
            matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
            "the completed row is new and read live once, got {events:?}"
        );
    }

    /// A file that exists at attach but isn't valid UTF-8 yet must not
    /// be replayed from 0 as live once it becomes readable.
    #[test]
    fn unreadable_at_attach_does_not_replay_once_readable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut bad = t425_jsonl(&[T425_PROMPT, T425_TOOL]).into_bytes();
        bad.extend_from_slice(&[0xff, 0xfe]);
        fs::write(&path, &bad).unwrap();

        let (tx, rx) = mpsc::channel();
        let manager = test_manager();
        manager
            .attach_at(
                "h1",
                path.clone(),
                move |e| {
                    let _ = tx.send(e);
                },
                None,
            )
            .unwrap();
        drain_brief(&rx);

        fs::write(
            &path,
            t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END]),
        )
        .unwrap();
        let events = drain(&rx);
        assert!(
            events
                .iter()
                .all(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "history must not replay as live events, got {events:?}"
        );
    }

    #[test]
    fn fingerprint_keeps_the_last_256_bytes_across_short_reads() {
        let mut fp = fingerprint_of(&[1u8; 300]);
        assert_eq!(fp.len(), FINGERPRINT_LEN);
        extend_fingerprint(&mut fp, &[2u8; 10]);
        assert_eq!(fp.len(), FINGERPRINT_LEN);
        assert_eq!(&fp[FINGERPRINT_LEN - 10..], &[2u8; 10]);
        assert_eq!(fp[0], 1);
        let mut small = Vec::new();
        extend_fingerprint(&mut small, b"ab");
        extend_fingerprint(&mut small, b"cd");
        assert_eq!(small, b"abcd");
    }

    /// While attached, a shrinking transcript is resynced, not replayed.
    #[test]
    fn attached_shrink_resyncs_without_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let state = t425_state(&path, None);
        fs::write(
            &path,
            t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END]),
        )
        .unwrap();
        assert_eq!(t425_tick(&state).len(), 4);
        fs::write(&path, t425_jsonl(&[T425_PROMPT, T425_END])).unwrap();
        let events = t425_tick(&state);
        assert!(matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]));
        assert_eq!(state.lock().last_pos, fs::metadata(&path).unwrap().len());
    }

    /// A subagent transcript that shrinks is re-seeded like at attach:
    /// nothing emitted, `last_pos` at the new EOF.
    #[test]
    fn subagent_shrink_reseeds_without_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, "").unwrap();
        let sub_dir = dir.path().join("subagents");
        fs::create_dir_all(&sub_dir).unwrap();
        let sub_path = sub_dir.join("agent-x.jsonl");
        let content = concat!(
            r#"{"type":"assistant","isSidechain":true,"agentId":"x","message":{"stop_reason":"end_turn","content":[]}}"#,
            "\n"
        );
        fs::write(&sub_path, content).unwrap();

        let state = t425_state(&path, None);
        {
            let mut s = state.lock();
            s.attached = true;
            s.ever_attached = true;
            s.subagents_dir = Some(sub_dir);
            s.subagents.insert(
                "x".into(),
                SubagentTail {
                    path: sub_path,
                    last_pos: 10_000,
                    partial: "junk".into(),
                    finished: false,
                    agent_type: None,
                    description: None,
                    started_ms: None,
                    started_ms_resolved: true,
                    last_ts_ms: 0,
                    open_failure_logged: false,
                },
            );
        }
        let events = t425_tick(&state);
        assert!(events.is_empty(), "no replay, got {events:?}");
        let s = state.lock();
        let t = s.subagents.get("x").unwrap();
        assert_eq!(t.last_pos, content.len() as u64);
        assert!(t.partial.is_empty());
        assert_eq!(t.finished, subagent_content_is_finished(content));
    }

    /// A pass over an adapter whose watched directories haven't
    /// changed at all since the last one must not touch the watcher —
    /// `rearm`'s fast-path equality check is what keeps a healthy
    /// harness's supervisor pass a no-op.
    #[test]
    fn supervise_once_does_not_rearm_a_healthy_unchanged_adapter() {
        let dir = TempDir::new().unwrap();
        let (mgr, _path, rx) = make_adapter(&dir);
        drain_brief(&rx);

        let rearmed = mgr.supervise_once();
        assert_eq!(
            rearmed, 0,
            "a healthy, unchanged adapter should not be re-armed"
        );
    }

    // ── #410: dead-tail detection and reattach ─────────────────────

    /// The headline #410 scenario: a watch dies in a way `rearm` can
    /// never notice (nothing about the directory's identity changed),
    /// the transcript keeps growing, and two supervisor passes across
    /// the `DEAD_TAIL_AFTER` window are what it takes to confirm and
    /// recover it — the first pass only starts tracking the stall, the
    /// second (once it's old enough) reattaches. Exactly one
    /// `AwaitingPrompt` proves the reattach happened exactly once, not
    /// zero or twice.
    #[test]
    fn dead_tail_is_reattached_and_settles_from_transcript() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        mgr.simulate_dead_watch_for_test("harness-1");

        // Grows the file while nothing is watching — the symptom.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        // Pass 1: notices the growth, starts tracking the stall — not
        // dead yet, and rearm has nothing to fix (the directory itself
        // never changed identity).
        let rearmed = mgr.supervise_once();
        assert_eq!(rearmed, 0, "the directory's identity never changed");
        assert!(
            drain_brief(&rx).is_empty(),
            "must not reattach on the very first observation of growth"
        );

        mgr.backdate_stall_for_test("harness-1");

        // Pass 2: same last_pos, now old enough — dead, reattach.
        mgr.supervise_once();

        let events = drain(&rx);
        let awaiting_count = events
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::AwaitingPrompt))
            .count();
        assert_eq!(
            awaiting_count, 1,
            "expected exactly one reattach's worth of AwaitingPrompt, got {events:?}"
        );
    }

    /// A tail that keeps up with the file it's watching must never be
    /// reattached, across several passes — the acceptance criterion is
    /// literally "no churn" for the common, healthy case.
    #[test]
    fn healthy_tail_is_never_reattached() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        for i in 0..3 {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Tool{i}"}}]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
            // Let the (still perfectly healthy) live watch catch up
            // normally before checking — `drain` waits for it.
            let events = drain(&rx);
            assert!(
                !events.is_empty(),
                "iteration {i}: the live watch should still be delivering normally"
            );
            let rearmed = mgr.supervise_once();
            assert_eq!(rearmed, 0, "iteration {i}: nothing about the watch changed");
        }

        assert!(
            drain_brief(&rx).is_empty(),
            "a healthy tail must never trigger a spurious reattach"
        );
    }

    /// At most one AUTOMATIC re-attach per `REATTACH_BACKOFF`: a second
    /// dead-tail cycle that starts (and gets confirmed) well within the
    /// backoff window of the first must not reattach again, even though
    /// `check_dead_tail` genuinely reports it as dead.
    #[test]
    fn auto_reattach_respects_backoff() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        mgr.simulate_dead_watch_for_test("harness-1");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
        mgr.supervise_once();
        mgr.backdate_stall_for_test("harness-1");
        mgr.supervise_once();
        let first = drain(&rx);
        assert!(
            !first.is_empty(),
            "expected the first automatic reattach to produce events"
        );

        // Break the freshly re-attached adapter's watch again right
        // away and grow the file again — a second dead cycle, well
        // within `REATTACH_BACKOFF` of the first.
        mgr.simulate_dead_watch_for_test("harness-1");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"X"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
        mgr.supervise_once();
        mgr.backdate_stall_for_test("harness-1");
        mgr.supervise_once();

        assert!(
            drain_brief(&rx).is_empty(),
            "a second automatic reattach within REATTACH_BACKOFF must not happen"
        );
    }

    /// `ClaudeEventsManager::reattach` — the manual, backoff-free path
    /// behind the `claude_events_reattach` Tauri command: `NotAttached`
    /// for an id with no adapter at all, `Healthy` when nothing needs
    /// fixing, `Reattached` when the tail actually was dead — and,
    /// unlike the automatic path, immediately, with no
    /// `DEAD_TAIL_AFTER` wait and no backoff.
    #[test]
    fn manual_reattach_outcomes() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        assert_eq!(
            mgr.reattach("no-such-harness").unwrap(),
            ReattachOutcome::NotAttached
        );

        assert_eq!(
            mgr.reattach("harness-1").unwrap(),
            ReattachOutcome::Healthy,
            "nothing changed since attach"
        );
        assert!(drain_brief(&rx).is_empty());

        mgr.simulate_dead_watch_for_test("harness-1");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();

        assert_eq!(
            mgr.reattach("harness-1").unwrap(),
            ReattachOutcome::Reattached,
            "manual reattach must recover a dead tail immediately, no backoff or wait"
        );
        let events = drain(&rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "expected the manual reattach to re-read the transcript, got {events:?}"
        );
    }

    /// Review fix for #410: `supervise_map` collects `ReattachJob`s
    /// under the registry lock, then installs each one — separately,
    /// after releasing it. A `detach` landing in that gap must not let
    /// the stale reattach resurrect the (now closed) harness. Runs the
    /// two phases as an explicit test-only seam rather than racing real
    /// threads — see `collect_dead_tail_jobs_for_test`'s doc comment.
    #[test]
    fn reattach_abandoned_when_detached_between_collect_and_perform() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        mgr.simulate_dead_watch_for_test("harness-1");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
        mgr.supervise_once();
        mgr.backdate_stall_for_test("harness-1");

        let jobs = mgr.collect_dead_tail_jobs_for_test();
        assert_eq!(jobs.len(), 1, "expected exactly one dead-tail job");

        // The race: the harness is closed between collection and
        // install.
        assert!(mgr.detach("harness-1"));

        mgr.perform_reattach_jobs_for_test(jobs);

        assert_eq!(
            mgr.reattach("harness-1").unwrap(),
            ReattachOutcome::NotAttached,
            "the abandoned reattach must not have resurrected a detached harness"
        );
        // The abandoned reattach's own construction (re-reading the
        // transcript's existing history) may have already emitted a
        // one-time synthetic event through the OLD channel before the
        // CAS ever ran — harmless, since nothing installed it. What
        // must NOT happen is a leaked watcher: drain whatever
        // construction-time backlog there was, then confirm nothing
        // MORE arrives after it, proving the abandoned adapter's
        // debouncer was actually dropped rather than left running.
        drain_brief(&rx);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"X"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
        assert!(
            drain_brief(&rx).is_empty(),
            "a detached harness must not keep receiving events from a leaked, abandoned watcher"
        );
    }

    /// Same race, the other direction: a fresh caller-driven `attach()`
    /// (a respawn — a new session on the same harness id) lands between
    /// collection and install. The fresh adapter must survive; the
    /// stale reattach's own (already-built) adapter is simply dropped.
    #[test]
    fn reattach_abandoned_when_fresh_attach_lands_between_collect_and_perform() {
        let dir = TempDir::new().unwrap();
        let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        mgr.simulate_dead_watch_for_test("harness-1");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
        mgr.supervise_once();
        mgr.backdate_stall_for_test("harness-1");

        let jobs = mgr.collect_dead_tail_jobs_for_test();
        assert_eq!(jobs.len(), 1, "expected exactly one dead-tail job");

        // The race: a fresh attach (respawn) lands on the same harness
        // id before the stale reattach installs.
        let (tx2, rx2) = mpsc::channel();
        let fresh_path = dir.path().join("fresh-session.jsonl");
        fs::write(&fresh_path, "").unwrap();
        mgr.attach_at(
            "harness-1",
            fresh_path.clone(),
            move |e| {
                let _ = tx2.send(e);
            },
            None,
        )
        .unwrap();

        mgr.perform_reattach_jobs_for_test(jobs);

        // Confirm the FRESH adapter is the one that survived: it must
        // still be tailing `fresh_path`, not the stale `path`.
        let mut f2 = fs::OpenOptions::new()
            .append(true)
            .open(&fresh_path)
            .unwrap();
        writeln!(
            f2,
            r#"{{"type":"assistant","sessionId":"y","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f2.sync_all().unwrap();
        let events = drain(&rx2);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
            "the fresh adapter must have survived the abandoned reattach, got {events:?}"
        );
    }

    #[test]
    fn should_heartbeat_rate_limits_to_once_per_interval() {
        let now = Instant::now();
        assert!(
            should_heartbeat(None, now),
            "the very first heartbeat should fire immediately"
        );
        assert!(
            !should_heartbeat(Some(now), now),
            "must not fire again at the same instant"
        );
        let almost_due = now + Duration::from_secs(59);
        assert!(
            !should_heartbeat(Some(now), almost_due),
            "must not fire before the interval elapses"
        );
        let due = now + HEARTBEAT_INTERVAL;
        assert!(
            should_heartbeat(Some(now), due),
            "must fire once the interval has elapsed"
        );
    }

    #[test]
    fn should_warn_utf8_stall_fires_once_at_threshold() {
        assert!(
            !should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD - 1, false),
            "must not fire before the threshold"
        );
        assert!(
            should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD, false),
            "must fire once the threshold is reached"
        );
        assert!(
            !should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD, true),
            "must not re-fire once already warned for this run"
        );
        assert!(
            should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD + 5, false),
            "must fire past the threshold too, as long as it hasn't warned yet"
        );
    }

    /// Review fix for #410: `DirId::of` must never fail for a path that
    /// exists — a directory whose identity genuinely can't be read (an
    /// unsupported filesystem, or a metadata race) degrades to an
    /// "unknown" identity rather than dropping out of the watch set
    /// entirely. Two "unknown" identities for the SAME still-existing
    /// path must compare equal — otherwise `rearm`'s `desired == armed`
    /// fast path would spuriously call every single such directory
    /// "changed" on every pass, forever.
    #[test]
    fn dir_id_of_an_unreadable_path_is_an_unknown_identity_equal_to_itself() {
        let dir = TempDir::new().unwrap();
        // A path with no metadata to read — the simplest cross-platform
        // stand-in for "identity unavailable" (the real-world case is a
        // filesystem where `created()` errors on an otherwise perfectly
        // normal, existing directory).
        let unreadable = dir.path().join("does-not-exist");
        let a = DirId::of(&unreadable);
        let b = DirId::of(&unreadable);
        assert_eq!(
            a, b,
            "two 'unknown' identities for the same path must compare equal"
        );
    }

    /// End-to-end (through `tick`, not just the pure decision fn): a
    /// main transcript stuck on the same invalid byte at the same
    /// `last_pos` climbs the stall counter one per tick, warns exactly
    /// once at the threshold, and a later successful read clears all
    /// three fields — the actual field-mutating logic `tick` uses.
    #[test]
    fn utf8_stall_counter_tracks_consecutive_failures_and_resets_on_recovery() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        // A lone continuation byte: not a prefix of anything, so it
        // never becomes valid UTF-8 no matter how many times it's
        // re-read — the stall persists until the file itself changes.
        fs::write(&path, [0x80]).unwrap();
        let state = Arc::new(Mutex::new(TailState {
            harness_id: "h".into(),
            path: path.clone(),
            last_pos: 0,
            partial: String::new(),
            attached: true,
            ever_attached: true,
            fingerprint: Vec::new(),
            in_assistant_turn: false,
            actions: None,
            subagents_dir: None,
            subagents: HashMap::new(),
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }));

        for expected in 1..UTF8_STALL_WARN_THRESHOLD {
            tick(&state, &|_| {});
            let s = state.lock();
            assert_eq!(
                s.utf8_stall_count, expected,
                "stall count should climb by exactly one per tick"
            );
            assert_eq!(s.utf8_stall_at, Some(0));
            assert!(!s.utf8_stall_warned, "must not warn before the threshold");
        }

        tick(&state, &|_| {});
        {
            let s = state.lock();
            assert_eq!(s.utf8_stall_count, UTF8_STALL_WARN_THRESHOLD);
            assert!(
                s.utf8_stall_warned,
                "must warn once the threshold is reached"
            );
        }

        // Recovery: the file changes underneath the stall — the next
        // tick decodes cleanly and must rearm every field.
        fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
        tick(&state, &|_| {});
        let s = state.lock();
        assert_eq!(
            s.utf8_stall_count, 0,
            "a successful read must clear the stall count"
        );
        assert_eq!(s.utf8_stall_at, None);
        assert!(
            !s.utf8_stall_warned,
            "a successful read must rearm the warn guard"
        );
    }
}
