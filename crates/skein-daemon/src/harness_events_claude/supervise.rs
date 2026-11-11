//! Dead-tail detection and automatic reattach, run from a background supervisor thread.

use super::ClaudeEvent;
use super::adapter::{REATTACH_BACKOFF, ReattachRecipe, Registry, TailState};
use super::attach::{ReattachInstall, reattach_at_impl};
use super::tick::tick;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

/// How often the background supervisor re-evaluates every attached
/// adapter's watch set (#362) — see `ClaudeEventsManager::new`'s
/// spawned thread and `supervise_once`/`supervise_map`. Independent of
/// `DEBOUNCE_MS`: that's notify's own coalescing window once a watch is
/// live, this is how long a directory that vanished and came back (the
/// #362 failure mode — Windows delivers nothing when a watched
/// directory is deleted, so nothing tells the tail to look again) can
/// stay silently unwatched before the next pass notices.
pub(super) const SUPERVISE_INTERVAL: Duration = Duration::from_secs(2);

/// One adapter's `(state, on_event)` pair, carried out of the registry
/// lock so `supervise_map` can run its catch-up `tick` after releasing
/// it. Named purely so the `Vec` below doesn't trip
/// `clippy::type_complexity`.
pub(super) type CatchUpTick = (
    Arc<Mutex<TailState>>,
    Arc<dyn Fn(ClaudeEvent) + Send + Sync>,
);

/// Everything `perform_reattach` needs for one harness, carried out of
/// the registry lock the same way `CatchUpTick` is (#410) —
/// `reattach_at_impl` must never run while `inner` is locked, since it
/// locks `inner` itself (twice) to check-and-install the replacement
/// adapter.
pub(super) struct ReattachJob {
    pub(super) harness_id: String,
    /// The generation `Adapter::check_dead_tail` observed this harness
    /// at, captured at collection time (#410 review fix) — the CAS
    /// `reattach_at_impl` runs before installing the replacement. A
    /// `detach` or a fresh `attach()` landing between collection and
    /// install bumps or removes this, so the stale job is abandoned
    /// instead of resurrecting or clobbering something.
    pub(super) generation: u64,
    pub(super) path: PathBuf,
    pub(super) on_event: Arc<dyn Fn(ClaudeEvent) + Send + Sync>,
    pub(super) recipe: Option<ReattachRecipe>,
    /// Short, log-friendly cause: `"transcript grew while the tail read
    /// nothing"` for the automatic dead-tail path, `"manual"` for
    /// `ClaudeEventsManager::reattach`.
    pub(super) reason: &'static str,
    pub(super) last_pos: u64,
    pub(super) file_len: u64,
    /// `None` for a manual reattach — there's no stall duration to
    /// report when the caller asked right now, unconditionally.
    pub(super) stalled_for_ms: Option<u128>,
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
pub(super) fn collect_dead_tail_jobs(inner: &Arc<Mutex<Registry>>) -> Vec<ReattachJob> {
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
pub(super) fn supervise_map(inner: &Arc<Mutex<Registry>>) -> usize {
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
pub(super) fn perform_reattach(inner: &Arc<Mutex<Registry>>, job: ReattachJob) {
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
pub(super) fn spawn_supervisor_thread(inner: Weak<Mutex<Registry>>) {
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
pub(super) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
