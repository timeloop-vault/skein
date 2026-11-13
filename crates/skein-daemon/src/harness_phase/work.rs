//! The end-of-turn deferral's tick rules (#277, generalised by #446 + #441).
//! Port of `app/src/deferral.ts`. An `impl PhaseMachine` block over the two
//! registries the machine owns.
//!
//! "Outstanding work" is subagents PLUS background tasks. Either one holding
//! a main session's end of turn keeps the harness `running` instead of a
//! premature `waiting`.
//!
//! The summary strings ("N delegated agents finished", "N background tasks
//! still running") are not here: in TS they live in `harnessActivityLabels.ts`
//! (`delegationSummary` / `stillRunningSummary`), and are not ported by this
//! step.

use std::collections::BTreeMap;

use super::background_tasks::deadline_of;
use super::constants::{BACKGROUND_TASK_CEILING_MS, DELEGATION_CEILING_MS, DELEGATION_SETTLE_MS};
use super::machine::{PhaseMachine, TickWork};
use super::types::{ActivityPhase, TransitionSource};

/// #448: the two counts behind [`PhaseMachine::outstanding_work`], per
/// harness, for the agent API's listing verbs. Same
/// `working_count`s, so attach-seeded and overdue entries are excluded
/// exactly as they are from the deferral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkCounts {
    pub subagents: usize,
    pub background_tasks: usize,
}

impl PhaseMachine {
    /// Working subagents plus working background tasks for one harness.
    #[must_use]
    pub fn outstanding_work(&self, id: &str) -> usize {
        self.subagents.working_count(id) + self.background_tasks.working_count(id)
    }

    /// `workSnapshot`: [`WorkCounts`] for each of `ids`.
    #[must_use]
    pub fn work_snapshot<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> BTreeMap<String, WorkCounts> {
        ids.into_iter()
            .map(|id| {
                (
                    id.to_owned(),
                    WorkCounts {
                        subagents: self.subagents.working_count(id),
                        background_tasks: self.background_tasks.working_count(id),
                    },
                )
            })
            .collect()
    }

    /// Ceiling for background tasks that have no deadline of their own: a
    /// Bash/PowerShell task or a persistent/no-timeout Monitor, measured from
    /// max(deferral armed, task started). A Monitor with a timeout is ended
    /// by `expire_due` at its deadline instead. Marked overdue, not removed:
    /// it is presumably still running.
    fn ceiling_overdue_tasks(&mut self, id: &str, armed_at: i64, now_ms: i64) -> usize {
        let ids: Vec<String> = self
            .background_tasks
            .live(id)
            .into_iter()
            .filter(|e| !e.from_attach && !e.overdue && deadline_of(e).is_none())
            .filter(|e| now_ms >= armed_at.max(e.started_at) + BACKGROUND_TASK_CEILING_MS)
            .map(|e| e.task_id)
            .collect();
        if ids.is_empty() {
            return 0;
        }
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let n = self.background_tasks.mark_overdue(id, &refs);
        if n > 0 {
            tracing::warn!(
                target: "skein::activity",
                "[skein] harness {id}: {n} background task(s) held the deferred end of turn for {} min; presumed still running, no longer counted",
                BACKGROUND_TASK_CEILING_MS / 60_000
            );
        }
        n
    }

    /// One armed deferral's tick (TS `tickDeferred`).
    fn tick_deferred(&mut self, id: &str, armed_at: i64, now_ms: i64) {
        let Some(a) = self.store.get(id) else {
            return;
        };
        // #86: permission is the harder stop. Leave the harness alone;
        // re-evaluate on a later tick once the dialog clears back to `running`.
        if a.phase == ActivityPhase::Permission {
            return;
        }
        let activity_at = a.delegation_activity_at;

        let mut released = self.ceiling_overdue_tasks(id, armed_at, now_ms);
        // Rule 4: subagent ceiling. See `DELEGATION_CEILING_MS` for the
        // measurement behind the threshold.
        if self.subagents.working_count(id) > 0 && now_ms - activity_at >= DELEGATION_CEILING_MS {
            let dropped = self.subagents.presume_gone(id);
            released += dropped;
            tracing::warn!(
                target: "skein::activity",
                "[skein] harness {id}: presumed {dropped} subagent(s) gone after {} min with no subagent event; flushing the deferred end of turn",
                DELEGATION_CEILING_MS / 60_000
            );
        }

        let outstanding = self.outstanding_work(id);
        let Some(a) = self.store.get_mut(id) else {
            return;
        };
        if outstanding > 0 {
            // Still non-empty (perhaps again, after having been seen empty):
            // clear the mark so a fresh settle window starts cleanly.
            a.delegation_emptied_at = None;
            return;
        }
        if released > 0 {
            // Nothing actually finished, so no settle window: flush now, with
            // the source that tells notifications not to claim completion.
            a.delegation_deferred_at = None;
            a.delegation_emptied_at = None;
            // Nothing was claimed finished, so a later ordinary waiting
            // notification must not report these as "delegated agents
            // finished". `waitingNote` ignores `delegatedCount` for this
            // source.
            a.delegated_count = 0;
            self.set_phase(
                id,
                ActivityPhase::Waiting,
                TransitionSource::DelegationCeiling,
                now_ms,
            );
            return;
        }
        // Rule 3: settle timer. See `DELEGATION_SETTLE_MS` for the
        // measurement behind the threshold.
        match a.delegation_emptied_at {
            None => a.delegation_emptied_at = Some(now_ms),
            Some(emptied_at) if now_ms - emptied_at >= DELEGATION_SETTLE_MS => {
                a.delegation_deferred_at = None;
                a.delegation_emptied_at = None;
                self.set_phase(
                    id,
                    ActivityPhase::Waiting,
                    TransitionSource::DelegationSettled,
                    now_ms,
                );
            }
            Some(_) => {}
        }
    }

    /// #441: a `running` authoritative harness with outstanding work, no
    /// deferral armed, and no signal of any kind for the ceiling has lost its
    /// end (a dropped `SubagentStop`, a task notification that never landed).
    /// Presume the work gone and flip to `waiting`; nothing is claimed
    /// finished. Returns true when it fired.
    ///
    /// A foreground tool writes no transcript row until it returns, so a long
    /// one reads as silence here. That is safe only because Claude Code caps
    /// a foreground Bash/PowerShell call at 10 min, under the 15 min
    /// `DELEGATION_CEILING_MS`; a foreground `Agent` call bumps
    /// `delegation_activity_at` through its subagent's own rows. Revisit if
    /// that cap is ever raised.
    fn tick_stale_work(&mut self, id: &str, now_ms: i64) -> bool {
        let Some(a) = self.store.get(id) else {
            return false;
        };
        if a.phase != ActivityPhase::Running || !a.authoritative || self.outstanding_work(id) == 0 {
            return false;
        }
        let newest_task = self
            .background_tasks
            .live(id)
            .iter()
            .filter(|e| !e.from_attach && !e.overdue)
            .map(|e| e.started_at)
            .fold(0, i64::max);
        let last_work_at = a
            .last_turn_signal
            .map_or(a.spawned_at, |s| s.at)
            .max(a.delegation_activity_at)
            .max(newest_task);
        if now_ms - last_work_at < DELEGATION_CEILING_MS {
            return false;
        }
        let subs = self.subagents.presume_gone(id);
        let tasks = self.background_tasks.presume_gone(id);
        tracing::warn!(
            target: "skein::activity",
            "[skein] harness {id}: no signal for {} min with work outstanding; presumed {subs} subagent(s) and {tasks} background task(s) gone (nothing claimed finished)",
            DELEGATION_CEILING_MS / 60_000
        );
        if let Some(a) = self.store.get_mut(id) {
            a.delegated_count = 0;
        }
        self.set_phase(
            id,
            ActivityPhase::Waiting,
            TransitionSource::WorkWatchdog,
            now_ms,
        );
        true
    }

    /// Called by the tick for EVERY store entry at the top of the loop body.
    /// [`TickWork::Handled`] means the tick must skip this harness.
    ///
    /// A deferred harness is evaluated before the tick's `authoritative` skip
    /// because it IS authoritative by definition: only the Claude
    /// translator's `awaiting_prompt_from_adapter` arms one, and only while an
    /// L2c adapter is attached.
    pub(super) fn tick_work(&mut self, id: &str, now_ms: i64) -> TickWork {
        // A Monitor ends at its own deadline; the Rust sweep only runs on
        // transcript ticks, so expire here too.
        self.background_tasks.expire_due(id, now_ms);
        let armed_at = self.store.get(id).and_then(|a| a.delegation_deferred_at);
        if let Some(armed_at) = armed_at {
            self.tick_deferred(id, armed_at, now_ms);
            return TickWork::Handled;
        }
        if self.tick_stale_work(id, now_ms) {
            TickWork::Handled
        } else {
            TickWork::NotHandled
        }
    }
}
