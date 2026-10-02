//! `ClaudeEventsManager`, the public handle the Tauri commands drive: attach, detach, reattach.
#[cfg(test)]
use super::adapter::DEAD_TAIL_AFTER;
#[cfg(test)]
use super::supervise::ReattachJob;
#[cfg(test)]
use super::supervise::collect_dead_tail_jobs;
#[cfg(test)]
use super::supervise::perform_reattach;
#[cfg(test)]
use super::supervise::supervise_map;
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use super::adapter::{ActionPersistence, AttachInfo, ReattachRecipe, Registry};
use super::attach::{ReattachInstall, attach_at_impl, reattach_at_impl};
use super::paths::session_jsonl_path;
use super::supervise::spawn_supervisor_thread;
use super::tick::tick;
use super::{ClaudeEvent, ClaudeEventsError};
use crate::db::Database;
use crate::harness_actions_claude::ActionExtractor;
use parking_lot::Mutex;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

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
    pub(super) fn new_for_test(db: Arc<Database>) -> Self {
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
    pub(super) fn collect_dead_tail_jobs_for_test(&self) -> Vec<ReattachJob> {
        collect_dead_tail_jobs(&self.inner)
    }

    /// Test-only counterpart to `collect_dead_tail_jobs_for_test`: runs
    /// exactly what `supervise_map` would have run immediately after
    /// collecting, for each job, in order.
    #[cfg(test)]
    pub(super) fn perform_reattach_jobs_for_test(&self, jobs: Vec<ReattachJob>) {
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
    pub(super) fn simulate_dead_watch_for_test(&self, harness_id: &str) {
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
    pub(super) fn backdate_stall_for_test(&self, harness_id: &str) {
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
        fresh_process: bool,
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
        self.attach_at_with(&harness_id, path, on_event, persistence, fresh_process)
    }

    /// Path-injected variant — used by tests to point the adapter at
    /// a tempdir without touching `HOME`. The production path goes
    /// through `attach()` above, which resolves the JSONL path from
    /// `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
    ///
    /// `actions` is the persistence sink; pass `None` from phase-only
    /// tests to skip the `harness_actions` table entirely.
    ///
    /// Test-only shorthand for `attach_at_with(..., false)`: a non-fresh
    /// attach, i.e. one that keeps the transcript's own verdict (#336).
    #[cfg(test)]
    pub(super) fn attach_at<F>(
        &self,
        harness_id: &str,
        path: PathBuf,
        on_event: F,
        actions: Option<ActionPersistence>,
    ) -> Result<AttachInfo, ClaudeEventsError>
    where
        F: Fn(ClaudeEvent) + Send + Sync + 'static,
    {
        attach_at_impl(&self.inner, harness_id, path, on_event, actions, false)
    }

    /// `attach_at` plus `fresh_process` (#336): true only for the attach
    /// right after `pty_spawn`. See `settle_initial_event_for_fresh_process`.
    pub(super) fn attach_at_with<F>(
        &self,
        harness_id: &str,
        path: PathBuf,
        on_event: F,
        actions: Option<ActionPersistence>,
        fresh_process: bool,
    ) -> Result<AttachInfo, ClaudeEventsError>
    where
        F: Fn(ClaudeEvent) + Send + Sync + 'static,
    {
        attach_at_impl(
            &self.inner,
            harness_id,
            path,
            on_event,
            actions,
            fresh_process,
        )
    }
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
    /// `tail_tests::detach_reports_whether_an_adapter_was_removed` test) can tell a real detach from a stale one
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
