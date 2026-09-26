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
use std::sync::Arc;
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

/// Per-harness adapter handle. Only role is to keep the debouncer
/// alive — dropping it stops the watcher (which in turn drops the
/// closure that holds the shared `TailState` Arc, so all per-harness
/// state goes with it).
struct Adapter {
    _debouncer: Debouncer<RecommendedWatcher>,
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
    /// Tracks whether the previous emitted event was inside an
    /// assistant turn — used to coalesce streamed `assistant` rows
    /// into one `AssistantTurn` event per turn boundary.
    in_assistant_turn: bool,
    /// Action persistence sink. `None` for path-injected tests that
    /// only care about phase events. Populated in production by
    /// `attach_at_with_actions`. Lives in `TailState` so the watcher
    /// callback can both extract and persist on each tick. Issue #80.
    actions: Option<ActionPersistence>,
    /// The session's `subagents` dir, when it could be created and
    /// watched at attach time. `None` disables subagent tailing
    /// entirely for this attach — telemetry here is strictly
    /// additive and must never be a reason `attach` itself fails.
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
    /// Whether the subagents-dir watch armed successfully. `false`
    /// means subagent telemetry is disabled for this attach — see the
    /// `tracing::warn!` sites in `attach_at` for why.
    pub subagents_armed: bool,
}

/// Manager — registry of live Claude adapters keyed by harness id.
/// Mirrors the shape of `PtyManager` / `WatcherManager`.
pub struct ClaudeEventsManager {
    inner: Mutex<HashMap<String, Adapter>>,
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
        Self {
            inner: Mutex::new(HashMap::new()),
            db,
            app: Some(app),
        }
    }

    /// Test constructor — no `AppHandle`, so the live tail persists
    /// without broadcasting (nothing to assert on the emit in a unit
    /// test, and building a real `AppHandle` needs a running app).
    #[cfg(test)]
    fn new_for_test(db: Arc<Database>) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            db,
            app: None,
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
    ) -> Result<AttachInfo, ClaudeEventsError>
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
        // file read — `scan_history` walks it exactly once (#171e:
        // this used to be two full parses, `determine_initial_state`
        // then `backfill_actions`, each running `serde_json::from_str`
        // on every line) and the fresh actions it collects are
        // persisted with one batch insert instead of one per line.
        let mut actions = actions;
        let (last_pos, attached, initial_event) = match fs::read_to_string(&path) {
            Ok(content) => {
                let (init, fresh) = scan_history(&content, actions.as_mut());
                if let Some(ap) = actions.as_ref() {
                    persist_extracted_batch(ap, fresh);
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

        // Subagent transcripts live in a sibling `<session-id>/subagents`
        // dir next to the main `.jsonl` (`skein_harness::claude::
        // subagents_dir`, #209). Create it eagerly, mirroring the
        // `parent` precedent just above — a session that hasn't
        // delegated yet still gets a watchable directory the moment it
        // does. This is strictly additive telemetry: any failure here
        // degrades to no subagent tracking for this attach rather than
        // failing `attach` itself — a working harness must never be
        // held hostage by it.
        let mut subagents_dir_opt = skein_harness::claude::subagents_dir(&path);
        if let Some(dir) = &subagents_dir_opt
            && let Err(e) = fs::create_dir_all(dir)
        {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "claude_events: could not create subagents dir; subagent telemetry disabled for this attach"
            );
            subagents_dir_opt = None;
        }

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
            harness_id: harness_id.clone(),
            path,
            last_pos,
            partial: String::new(),
            attached,
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
        let on_event = Arc::new(on_event);
        let cb_on_event = Arc::clone(&on_event);
        let cb_harness_id = harness_id.clone();
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
        debouncer
            .watcher()
            .watch(parent_ref, RecursiveMode::NonRecursive)
            .map_err(ClaudeEventsError::from_err)?;

        // Arm the subagents-dir watch on the same debouncer — notify
        // supports several watched paths on one watcher, and the same
        // `tick` closure above fires for either. Same additive-only
        // rule as directory creation above: a failure here disables
        // subagent tailing for this attach (clearing the state's
        // `subagents_dir`) rather than failing `attach`.
        if let Some(dir) = &subagents_dir_opt
            && let Err(e) = debouncer.watcher().watch(dir, RecursiveMode::NonRecursive)
        {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "claude_events: could not watch subagents dir; subagent telemetry disabled for this attach"
            );
            state.lock().subagents_dir = None;
            subagents_dir_opt = None;
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
        // Same reasoning, for the subagents discovered above: only
        // emit if the watch actually armed (otherwise `subagents_dir`
        // was cleared and disk state is untracked from here on).
        if subagents_dir_opt.is_some() {
            for event in initial_subagent_starts {
                on_event(event);
            }
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
        let subagents_armed = subagents_dir_opt.is_some();
        let log_harness_id = harness_id.clone();
        let replaced = self
            .inner
            .lock()
            .insert(
                harness_id,
                Adapter {
                    _debouncer: debouncer,
                },
            )
            .is_some();
        if replaced {
            tracing::info!(
                harness_id = %log_harness_id,
                "claude_events: attach replaced an existing adapter for this harness"
            );
        }
        Ok(AttachInfo {
            path: attach_info_path,
            already_existed: attached,
            subagents_armed,
        })
    }

    /// Stop the adapter for `harness_id`. No-op if unknown. Returns
    /// whether an adapter was actually removed, so the caller (and the
    /// `#362` unit test below) can tell a real detach from a stale one
    /// firing against an id that already went away.
    pub fn detach(&self, harness_id: &str) -> bool {
        let removed = self.inner.lock().remove(harness_id).is_some();
        tracing::info!(harness_id, removed, "claude_events: detach");
        removed
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
            tracing::info!(
                harness_id = %s.harness_id,
                path = %s.path.display(),
                "claude_events: transcript appeared"
            );
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
    // truncation). Reset and read from 0 — better to replay than to
    // silently miss events.
    if let Ok(meta) = file.metadata()
        && meta.len() < s.last_pos
    {
        s.last_pos = 0;
        s.partial.clear();
        s.in_assistant_turn = false;
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
            tail.last_pos = 0;
            tail.partial.clear();
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
                            harness_id,
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
        // shared 5 s deadline.
        let mut satisfied = vec![false; N];
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && satisfied.iter().any(|s| !s) {
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

    #[test]
    fn subagent_start_emitted_with_meta_from_sidecar() {
        let dir = TempDir::new().unwrap();
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        // Drain the main-transcript bootstrap event before we care
        // about subagent ones.
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
            .attach_at("h1".into(), path, move |e| tx.send(e).unwrap(), None)
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
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
        let (_mgr, path, rx) = make_adapter_with_existing_main(&dir);
        drain(&rx);

        let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();

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
