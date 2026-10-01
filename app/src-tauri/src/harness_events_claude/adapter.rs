//! Per-harness tail state: the adapter, its dead-tail bookkeeping and the registry that holds them.

use super::ClaudeEvent;
use super::background_tasks::BackgroundState;
use super::paths::{DirId, desired_watches};
use crate::db::Database;
use crate::harness_actions_claude::ActionExtractor;
use notify_debouncer_mini::Debouncer;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use parking_lot::Mutex;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Per-harness adapter handle. Holds the debouncer — dropping it stops
/// the watcher, which in turn drops the closure that holds the other
/// clone of `state`, so all per-harness state goes with it — plus what
/// `rearm` (#362) needs to recompute and reconcile the watch set from
/// outside the debounce thread's closure: the same `state`/`on_event`
/// the closure already holds clones of, and `armed`, the watch set as
/// of the last successful reconciliation.
pub(super) struct Adapter {
    pub(super) debouncer: Debouncer<RecommendedWatcher>,
    pub(super) state: Arc<Mutex<TailState>>,
    pub(super) on_event: Arc<dyn Fn(ClaudeEvent) + Send + Sync>,
    pub(super) armed: Vec<(PathBuf, DirId)>,
    /// One-shot guard, per path, for the "could not (re)arm watch" warn
    /// (#410) — without it, a directory that stays permanently
    /// unwatchable (permissions, say) warns on every single supervisor
    /// pass forever. Same shape as `TailState`'s existing one-shot
    /// guards: a path's entry is inserted on the transition into
    /// failing (that's the one `warn!`) and removed the moment a watch
    /// on it succeeds again, with later failures at debug while the
    /// entry is already present.
    pub(super) failing_watches: HashSet<PathBuf>,
    /// Ingredients for a fresh `ActionPersistence` on re-attach (#410).
    /// `None` mirrors `ActionPersistence`'s own optionality in
    /// phase-only tests (`attach_at` called with `actions: None`).
    pub(super) reattach_recipe: Option<ReattachRecipe>,
    /// Dead-tail tracking (#410): `Some((last_pos, since))` once a
    /// supervisor pass has observed the transcript grow past
    /// `last_pos` without the tail's own `last_pos` moving — `since` is
    /// when *this* stall began. Cleared the moment the file stops
    /// growing relative to `last_pos` (the tail caught up) or vanishes
    /// (a missing file isn't a stalled tail, it's just not there).
    pub(super) stall: Option<(u64, Instant)>,
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
    pub(super) generation: u64,
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
pub(super) struct ReattachRecipe {
    pub(super) db: Arc<Database>,
    pub(super) harness_id: String,
    pub(super) room_id: String,
    pub(super) cwd: String,
    pub(super) app: Option<tauri::AppHandle>,
}

impl ReattachRecipe {
    pub(super) fn fresh_persistence(&self) -> ActionPersistence {
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
    pub(super) fn rearm(&mut self, harness_id: &str) -> bool {
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
    pub(super) fn check_dead_tail(&mut self, now: Instant) -> Option<DeadTail> {
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
pub(super) const DEAD_TAIL_AFTER: Duration = Duration::from_secs(10);

/// Minimum spacing between AUTOMATIC re-attaches for one harness
/// (#410) — a persistently unwatchable directory (permissions, say)
/// must not be re-attached every single supervisor pass forever. Only
/// applies to the dead-tail path; a manual `reattach` call always runs.
pub(super) const REATTACH_BACKOFF: Duration = Duration::from_secs(60);

/// What a confirmed dead tail needs to log and to act on — returned by
/// `Adapter::check_dead_tail`.
pub(super) struct DeadTail {
    pub(super) path: PathBuf,
    pub(super) last_pos: u64,
    pub(super) file_len: u64,
    pub(super) stalled_for_ms: u128,
}

/// Per-subagent tail state, one per `agent-<id>.jsonl` under the
/// session's `subagents` dir. Mirrors `TailState`'s read/carry-partial
/// shape but scoped to a single subagent transcript.
pub(super) struct SubagentTail {
    pub(super) path: PathBuf,
    /// Byte offset already consumed — same role as `TailState::last_pos`.
    pub(super) last_pos: u64,
    /// Trailing partial line carried across ticks — same role as
    /// `TailState::partial`.
    pub(super) partial: String,
    /// Exit/re-open state machine (`skein_harness::claude::SubagentLifecycle`,
    /// #440): a terminal `stop_reason` or a `SubagentHandback`
    /// `tool_result` ends the subagent; a later user prompt re-opens it
    /// and re-emits `SubagentStart`. Kept across ticks (and seeded from
    /// the whole file at attach) so a handback id seen earlier still
    /// matches its result.
    pub(super) lifecycle: skein_harness::claude::SubagentLifecycle,
    /// From the `agent-<id>.meta.json` sidecar; `None` when it's
    /// absent or doesn't parse. Carried here so `SubagentEnd` can
    /// report the same type/description `SubagentStart` did, without
    /// re-reading the sidecar every tick.
    pub(super) agent_type: Option<String>,
    pub(super) description: Option<String>,
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
    pub(super) started_ms: Option<i64>,
    /// Whether `started_ms` has already been set from the first row.
    /// Needed because `started_ms` staying `None` is itself a valid
    /// resolved outcome (row had no timestamp) that must not be
    /// overwritten by a later row.
    pub(super) started_ms_resolved: bool,
    /// Carry-forward "most recent timestamp seen on this transcript" —
    /// same role as `ActionExtractor::last_ts_ms` in
    /// `harness_actions_claude.rs`. Used as the `SubagentEnd` action's
    /// own timestamp when the terminal row itself carries no
    /// `timestamp`, rather than inventing `now`.
    pub(super) last_ts_ms: i64,
    /// Whether the last attempt to open/seek this transcript failed and
    /// was already warned about (#362). A subagent file that becomes
    /// permanently unreadable would otherwise warn on every single
    /// tick forever — the entry is never removed and the metadata
    /// fast-path (`meta.len() == tail.last_pos`) can't short-circuit a
    /// broken file either. Set on the transition into failing, cleared
    /// (with a debug! line) the moment an open succeeds again.
    pub(super) open_failure_logged: bool,
}

/// Mutable state shared with the watcher callback. The callback runs
/// on the debouncer thread — every field it touches lives in here.
// Four independent flags, each tracking a distinct one-shot condition
// (attach/parse state vs. two separate #362 log-dedup guards) rather
// than variants of one state machine — `struct_excessive_bools`'s
// alternative would just rename the same booleans.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct TailState {
    /// Stamped on every diagnostic log line this state's tick/detach
    /// path emits, so one harness's tail can be grepped out of a log
    /// that interleaves many rooms and harnesses (#362). Independent
    /// of `actions`, which is `None` in phase-only tests.
    pub(super) harness_id: String,
    pub(super) path: PathBuf,
    /// Byte offset into the file we've already consumed. Bumped on
    /// every read so the next tick only sees fresh bytes.
    pub(super) last_pos: u64,
    /// Trailing partial line carried across ticks — Claude flushes
    /// after each event but the OS may still split a write across
    /// what `notify` reports as separate events.
    pub(super) partial: String,
    /// Have we observed `attached` yet? If false, the file didn't
    /// exist when `attach` was called and we're waiting for create.
    pub(super) attached: bool,
    /// Whether this state has EVER been attached to the transcript
    /// (#425). `attached` flips back to false when the file vanishes,
    /// so it cannot tell a first appearance (every byte is new, read
    /// from 0 as live) from a reappearance (the same transcript,
    /// already consumed up to `last_pos`, which must not be replayed).
    pub(super) ever_attached: bool,
    /// The last up to `FINGERPRINT_LEN` bytes ending at `last_pos`
    /// (#425). When the transcript reappears after a vanish, the same
    /// bytes at the same offset are what says "same file, keep going";
    /// anything else is a different or truncated file and is resynced
    /// as backfill instead of replayed as live.
    pub(super) fingerprint: Vec<u8>,
    /// Tracks whether the previous emitted event was inside an
    /// assistant turn — used to coalesce streamed `assistant` rows
    /// into one `AssistantTurn` event per turn boundary.
    pub(super) in_assistant_turn: bool,
    /// Local slash command classifier (#463); fed every main-chain row.
    pub(super) local_command: LocalCommandTracker,
    /// Action persistence sink. `None` for path-injected tests that
    /// only care about phase events. Populated in production by
    /// `attach_at_with_actions`. Lives in `TailState` so the watcher
    /// callback can both extract and persist on each tick. Issue #80.
    pub(super) actions: Option<ActionPersistence>,
    /// The session's `subagents` dir — the computed path, whenever
    /// `skein_harness::claude::subagents_dir` can derive one, whether or
    /// not it exists yet (#362: this used to be created eagerly at
    /// attach time; Claude owns it, and a session that hasn't delegated
    /// yet has no reason to have one on disk before it does). `None`
    /// disables subagent tailing entirely for this attach — telemetry
    /// here is strictly additive and must never be a reason `attach`
    /// itself fails. The watch on this path (once it exists) is armed
    /// and re-armed by `Adapter::rearm`, not by anything in here.
    pub(super) subagents_dir: Option<PathBuf>,
    /// Per-subagent tail state, keyed by agent id.
    pub(super) subagents: HashMap<String, SubagentTail>,
    /// Background-task tracker fed every row of the main transcript and
    /// of every subagent transcript (#445), plus which subagent owns
    /// which task.
    pub(super) background: BackgroundState,
    /// One-shot guard for the "could not read subagents dir this tick"
    /// warn (#362) — without it, a subagents dir that becomes
    /// permanently unreadable (deleted, permissions) warns on every
    /// tick forever. Set on the transition into failing, cleared once
    /// a read succeeds again.
    pub(super) subagents_dir_read_failure_logged: bool,
    /// Set the first time a tick reads any bytes from the MAIN
    /// transcript since this `TailState` was created (#362) — i.e.
    /// once per attach, including a resumed session where the file
    /// already existed (it then fires on the first *appended* bytes
    /// after attach, since `attach_at` seeks straight to EOF). Backs
    /// the one `tracing::info!` line in `tick` that distinguishes
    /// "attached but nothing has ever come through the tail" from
    /// "tailing fine" at the default info filter.
    pub(super) first_read_logged: bool,
    /// Cumulative count of `ClaudeEvent`s handed to `on_event` since
    /// this `TailState` was created — main and subagent events combined
    /// (#362). One of the heartbeat's two liveness numbers, alongside
    /// `last_pos`: both climbing means the tail is genuinely healthy
    /// even if the frontend never shows it; both frozen while the file
    /// grows is what a real stall looks like.
    pub(super) events_sent: u64,
    /// Cumulative count of `on_event` dispatch batches that panicked,
    /// caught via `catch_unwind` around the whole per-tick dispatch loop
    /// (#362) so a bad callback can't unwind through `tick` unnoticed.
    /// Expected to stay 0 in production — the real callback in `lib.rs`
    /// only `.is_err()`s a dead channel, it never panics — so this
    /// exists for the heartbeat's own honesty and for tests that use a
    /// channel that can panic on send.
    pub(super) send_errors: u64,
    /// Wall-clock time the last heartbeat line was logged; `None`
    /// before the first one. Rate-limited to at most one per
    /// `HEARTBEAT_INTERVAL` — see `should_heartbeat`.
    pub(super) last_heartbeat: Option<Instant>,
    /// `last_pos` value the current run of consecutive UTF-8 mid-line
    /// failures started at (#362). `None` when the main tail isn't
    /// currently stalled at all.
    pub(super) utf8_stall_at: Option<u64>,
    /// How many consecutive ticks have failed to decode at
    /// `utf8_stall_at`. Reset to 0 the moment a read succeeds (which
    /// can only happen once `last_pos` is genuinely able to advance
    /// past the incomplete character) — see `UTF8_STALL_WARN_THRESHOLD`.
    pub(super) utf8_stall_count: u32,
    /// Whether the stall warn already fired for the *current* run
    /// (`utf8_stall_at`) — logged once per stall, not once per tick
    /// once past the threshold. Rearmed by the same reset as
    /// `utf8_stall_count`.
    pub(super) utf8_stall_warned: bool,
}

/// Action-extraction context bundled per attached harness. The
/// `extractor` holds the pending-tool-use buffer between rows; `db`
/// is the persistence sink; `harness_id`/`room_id` are stamped on
/// every row before insert.
pub(super) struct ActionPersistence {
    pub(super) extractor: ActionExtractor,
    pub(super) db: Arc<Database>,
    pub(super) harness_id: String,
    pub(super) room_id: String,
    /// The room worktree, so a live patch row can capture a review
    /// baseline for the file it names (#211). Empty for tests that
    /// only care about phase events.
    pub(super) cwd: String,
    /// Frontend emitter for live rows. `None` in tests. Backfill never
    /// emits (the frontend loads history via its initial query); only
    /// the live tail broadcasts. Issue #80 D1.
    pub(super) app: Option<tauri::AppHandle>,
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
pub(super) struct Registry {
    pub(super) adapters: HashMap<String, Adapter>,
    pub(super) last_auto_reattach: HashMap<String, Instant>,
    /// Source of `Adapter::generation` (#410 review fix) — bumped every
    /// time an adapter is actually installed (a fresh `attach` or a
    /// successful reattach), never reused. Starts at 0, so the first
    /// real generation assigned is 1; `Adapter::generation` therefore
    /// never needs an `Option` to mean "not yet installed" — by the
    /// time anything can observe an `Adapter` at all, it already has
    /// one.
    pub(super) next_generation: u64,
}
