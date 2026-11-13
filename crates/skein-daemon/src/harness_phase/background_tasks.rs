//! Per-harness registry of live background tasks. Port of
//! `app/src/backgroundTasks.ts`.
//!
//! Pure tracking of Claude's live background tasks (#446, epic #439): a
//! `Bash`/`PowerShell` `run_in_background` command or a `Monitor`. A task
//! is not a subagent, but it holds an end of turn the same way, so this
//! mirrors `subagents.rs` and feeds the same deferral.
//!
//! Why the disk stays the source of truth, not this map: the adapter
//! re-derives the live set from the transcripts on every attach and
//! replays `background_start` for whatever is still live.
//! [`BackgroundTasks::record`] is idempotent for exactly that reason: a
//! replay must read as "still going", never as a second start, and must
//! not reset `started_at`.
//!
//! A task seeded at attach (`initial == true`) may be an orphan from a
//! hard-killed session: a missing `.output` trailer does NOT mean it is
//! running. So it is listed but never counted as working (`from_attach`).
//!
//! `overdue` is the deferral ceiling's verdict: the task has held an end
//! of turn too long. It stays in the live list (it is presumably still
//! running, and the UI keeps showing it) but no longer counts as working.
//! Only `finish`/`forget`/`expire_due` remove an entry.
//!
//! The TS version notifies subscribers on change; here every mutating
//! method reports whether it changed something instead. Time is never
//! read: callers pass `now_ms`.

use std::collections::HashMap;

use serde::Serialize;
use skein_harness::claude::background::BackgroundKind;

/// One live background task, as last reported by `background_start`.
// Four independent flags, serialized as-is to clients (camelCase mirror
// of the TS entry); an enum would change the wire shape.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskEntry {
    pub task_id: String,
    pub kind: BackgroundKind,
    pub description: Option<String>,
    pub command: Option<String>,
    pub timeout_ms: Option<i64>,
    pub persistent: bool,
    /// The launching subagent, or `None` for the main session.
    pub agent_id: Option<String>,
    /// Epoch ms first seen. Preserved across a re-record of the same id.
    pub started_at: i64,
    /// Only ever seen via an attach-time replay. Sticky false: once a
    /// live start is recorded, a later replay must not demote it.
    pub from_attach: bool,
    /// Held an end of turn past the ceiling. Sticky until removed.
    pub overdue: bool,
    /// An overdue ceiling/watchdog notice already counted this task.
    /// Bookkeeping for [`BackgroundTasks::take_unreported_overdue`];
    /// never affects display.
    pub overdue_reported: bool,
}

/// What a `background_start` carries: an entry minus the bookkeeping
/// fields the registry owns.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundTaskInput {
    pub task_id: String,
    pub kind: BackgroundKind,
    pub description: Option<String>,
    pub command: Option<String>,
    pub timeout_ms: Option<i64>,
    pub persistent: bool,
    pub agent_id: Option<String>,
}

/// The moment a task ends by its own timeout: only a non-persistent
/// Monitor with a timeout has one. Everything else gets the ceiling.
#[must_use]
pub fn deadline_of(entry: &BackgroundTaskEntry) -> Option<i64> {
    if entry.kind != BackgroundKind::Monitor || entry.persistent {
        return None;
    }
    entry.timeout_ms.map(|t| entry.started_at.saturating_add(t))
}

/// Live background tasks per harness id, in insertion order so the sort
/// by `started_at` in [`BackgroundTasks::live`] is stable on ties.
#[derive(Debug, Default)]
pub struct BackgroundTasks {
    live: HashMap<String, Vec<BackgroundTaskEntry>>,
}

impl BackgroundTasks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A task started, or was found still running at attach time.
    /// Idempotent: keeps the original `started_at`, `overdue` and
    /// `overdue_reported`, and `from_attach` is sticky false (a live start
    /// promotes, a replay never demotes). Always changes, returns `true`.
    pub fn record(
        &mut self,
        harness_id: &str,
        entry: BackgroundTaskInput,
        initial: bool,
        now_ms: i64,
    ) -> bool {
        let list = self.live.entry(harness_id.to_owned()).or_default();
        if let Some(e) = list.iter_mut().find(|e| e.task_id == entry.task_id) {
            e.kind = entry.kind;
            e.description = entry.description;
            e.command = entry.command;
            e.timeout_ms = entry.timeout_ms;
            e.persistent = entry.persistent;
            e.agent_id = entry.agent_id;
            e.from_attach = e.from_attach && initial;
        } else {
            list.push(BackgroundTaskEntry {
                task_id: entry.task_id,
                kind: entry.kind,
                description: entry.description,
                command: entry.command,
                timeout_ms: entry.timeout_ms,
                persistent: entry.persistent,
                agent_id: entry.agent_id,
                started_at: now_ms,
                from_attach: initial,
                overdue: false,
                overdue_reported: false,
            });
        }
        true
    }

    /// The task ended. A no-op (false) for an unknown id.
    pub fn finish(&mut self, harness_id: &str, task_id: &str) -> bool {
        let Some(list) = self.live.get_mut(harness_id) else {
            return false;
        };
        let Some(pos) = list.iter().position(|e| e.task_id == task_id) else {
            return false;
        };
        list.remove(pos);
        self.drop_if_empty(harness_id);
        true
    }

    /// Drop everything tracked for a harness (teardown). False when
    /// nothing was tracked.
    pub fn forget(&mut self, harness_id: &str) -> bool {
        self.live.remove(harness_id).is_some()
    }

    /// The harness's live tasks, oldest first.
    #[must_use]
    pub fn live(&self, harness_id: &str) -> Vec<BackgroundTaskEntry> {
        let mut out = self.live.get(harness_id).cloned().unwrap_or_default();
        out.sort_by_key(|e| e.started_at);
        out
    }

    /// Tasks that count as working for the deferral: not attach-only
    /// orphans, not overdue.
    #[must_use]
    pub fn working_count(&self, harness_id: &str) -> usize {
        self.live.get(harness_id).map_or(0, |l| {
            l.iter().filter(|e| !e.from_attach && !e.overdue).count()
        })
    }

    /// Tasks the ceiling has marked overdue (attach-only ones excluded).
    #[must_use]
    pub fn overdue_count(&self, harness_id: &str) -> usize {
        self.live.get(harness_id).map_or(0, |l| {
            l.iter().filter(|e| !e.from_attach && e.overdue).count()
        })
    }

    /// Overdue tasks (attach-only excluded) no earlier ceiling notice has
    /// announced; marks them reported. Silent: the flag is
    /// display-irrelevant, so this never counts as a change.
    pub fn take_unreported_overdue(&mut self, harness_id: &str) -> usize {
        let Some(list) = self.live.get_mut(harness_id) else {
            return 0;
        };
        let mut n = 0;
        for e in list.iter_mut() {
            if e.from_attach || !e.overdue || e.overdue_reported {
                continue;
            }
            e.overdue_reported = true;
            n += 1;
        }
        n
    }

    /// Remove every entry whose own deadline has passed (a Monitor ends
    /// at its timeout; the Rust sweep only runs on transcript ticks).
    /// Returns `(removed, changed)`: `removed` counts those that were not
    /// attach-only (the TS return value), `changed` is true if anything
    /// at all was removed, attach-only included.
    pub fn expire_due(&mut self, harness_id: &str, now_ms: i64) -> (usize, bool) {
        let Some(list) = self.live.get_mut(harness_id) else {
            return (0, false);
        };
        let mut removed = 0;
        let mut changed = false;
        list.retain(|e| {
            let due = deadline_of(e).is_some_and(|d| d <= now_ms);
            if due {
                changed = true;
                if !e.from_attach {
                    removed += 1;
                }
            }
            !due
        });
        if changed {
            self.drop_if_empty(harness_id);
        }
        (removed, changed)
    }

    /// Mark these ids overdue (skipping unknown, attach-only and already
    /// overdue). Returns how many changed; non-zero means changed.
    pub fn mark_overdue(&mut self, harness_id: &str, task_ids: &[&str]) -> usize {
        let Some(list) = self.live.get_mut(harness_id) else {
            return 0;
        };
        let mut n = 0;
        for id in task_ids {
            let Some(e) = list.iter_mut().find(|e| e.task_id == *id) else {
                continue;
            };
            if e.from_attach || e.overdue {
                continue;
            }
            e.overdue = true;
            n += 1;
        }
        n
    }

    /// Stale-work watchdog: mark every working task overdue. Not a
    /// delete, since the task may genuinely still run. Returns the count.
    pub fn presume_gone(&mut self, harness_id: &str) -> usize {
        let Some(list) = self.live.get_mut(harness_id) else {
            return 0;
        };
        let mut n = 0;
        for e in list.iter_mut().filter(|e| !e.from_attach && !e.overdue) {
            e.overdue = true;
            n += 1;
        }
        n
    }

    fn drop_if_empty(&mut self, harness_id: &str) {
        if self.live.get(harness_id).is_some_and(Vec::is_empty) {
            self.live.remove(harness_id);
        }
    }
}

#[cfg(test)]
#[path = "tests/background_tasks.rs"]
mod tests;
