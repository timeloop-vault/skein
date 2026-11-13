//! Per-harness registry of live subagents. Port of `app/src/subagents.ts`.
//!
//! Pure tracking of Claude's live subagents (#276, epic #298). A subagent
//! runs in its own transcript, invisible to the main JSONL tail, so this
//! registry is the only record of which subagents are currently running
//! under a harness. #277's phase and notification policy reads it.
//!
//! Why the disk stays the source of truth, not this map: the adapter
//! rediscovers every still-running subagent from its own transcript the
//! moment it (re)attaches, on a fresh `subagent_start` and again after a
//! Skein restart. [`Subagents::record`] is idempotent for exactly that
//! reason: a restart replays `subagent_start` for everything still live,
//! and that replay must not look like a second subagent starting, nor
//! reset how long it has been running. That rediscovery also guarantees
//! this map can never leak a permanently stuck entry.
//!
//! The TS version notifies subscribers on change; here every mutating
//! method returns whether it changed something, and the owner decides
//! what to do with that. Time is never read: callers pass `now_ms`.

use std::collections::HashMap;

use serde::Serialize;

/// One live subagent, as last reported by `subagent_start`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentEntry {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    /// Epoch ms this subagent was first seen. Preserved across a
    /// redundant `subagent_start` for the same `agent_id`.
    pub started_at: i64,
    /// Has this id only ever been seen via an attach-time replay
    /// (`initial == true`), never a live start? `true` means the
    /// transcript was already on disk when the adapter attached; the PTY
    /// that spawned it is gone, so it is typically days old and nothing
    /// is coming. #277: 2.8% of real subagent transcripts are such
    /// orphans, and counting one as working would suppress a harness's
    /// notifications for the whole delegation-ceiling window after every
    /// restart. Sticky false: once a live start is recorded for an id, a
    /// later replay must not revert it.
    pub from_attach: bool,
}

/// What a `subagent_start` carries about the subagent's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentInput {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub description: Option<String>,
}

/// Live subagents per harness id. Entries are kept in insertion order so
/// that [`Subagents::live`]'s sort by `started_at` is stable on ties.
#[derive(Debug, Default)]
pub struct Subagents {
    live: HashMap<String, Vec<SubagentEntry>>,
}

impl Subagents {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A subagent transcript appeared, or one was found still running at
    /// attach time. Idempotent: a second start for an `agent_id` already
    /// tracked keeps the original `started_at` and does not double-count.
    ///
    /// `initial` mirrors `ClaudeEvent.initial` (#277): `true` for the
    /// attach-time batch found already on disk, `false` for a start
    /// observed live. `from_attach` is derived so a later live re-emit
    /// for an id first seen at attach promotes it: once false, always
    /// false. Always changes (the type and description are overwritten),
    /// so returns `true`.
    pub fn record(
        &mut self,
        harness_id: &str,
        entry: SubagentInput,
        initial: bool,
        now_ms: i64,
    ) -> bool {
        let list = self.live.entry(harness_id.to_owned()).or_default();
        if let Some(existing) = list.iter_mut().find(|e| e.agent_id == entry.agent_id) {
            existing.agent_type = entry.agent_type;
            existing.description = entry.description;
            existing.from_attach = existing.from_attach && initial;
        } else {
            list.push(SubagentEntry {
                agent_id: entry.agent_id,
                agent_type: entry.agent_type,
                description: entry.description,
                started_at: now_ms,
                from_attach: initial,
            });
        }
        true
    }

    /// The sidecar supplied a label after the start (#497). Updates the
    /// labels of an already-tracked entry only: a no-op (false) for an
    /// unknown id, and `started_at`/`from_attach` stay as they were,
    /// since a label says nothing about the work itself.
    pub fn relabel(
        &mut self,
        harness_id: &str,
        agent_id: &str,
        agent_type: Option<String>,
        description: Option<String>,
    ) -> bool {
        let Some(existing) = self
            .live
            .get_mut(harness_id)
            .and_then(|l| l.iter_mut().find(|e| e.agent_id == agent_id))
        else {
            return false;
        };
        existing.agent_type = agent_type;
        existing.description = description;
        true
    }

    /// The subagent reached a terminal stop reason. A no-op (false) for
    /// an unknown id (already finished, or never tracked).
    pub fn finish(&mut self, harness_id: &str, agent_id: &str) -> bool {
        let Some(list) = self.live.get_mut(harness_id) else {
            return false;
        };
        let Some(pos) = list.iter().position(|e| e.agent_id == agent_id) else {
            return false;
        };
        list.remove(pos);
        if list.is_empty() {
            self.live.remove(harness_id);
        }
        true
    }

    /// Drop every subagent tracked for a harness. Called from the same
    /// teardown that forgets the harness's phase, or the map grows for
    /// the life of the process. False when nothing was tracked.
    pub fn forget(&mut self, harness_id: &str) -> bool {
        self.live.remove(harness_id).is_some()
    }

    /// The harness's current live subagents, oldest first.
    #[must_use]
    pub fn live(&self, harness_id: &str) -> Vec<SubagentEntry> {
        let mut out = self.live.get(harness_id).cloned().unwrap_or_default();
        out.sort_by_key(|e| e.started_at);
        out
    }

    /// Count of this harness's subagents that count as working for #277's
    /// deferral: every tracked entry EXCEPT the attach-time-only orphans
    /// `from_attach` exists to exclude. [`Self::live`] stays an honest
    /// mirror of the transcripts (#276's contract); this is the one place
    /// the counting is opinionated.
    #[must_use]
    pub fn working_count(&self, harness_id: &str) -> usize {
        self.live
            .get(harness_id)
            .map_or(0, |l| l.iter().filter(|e| !e.from_attach).count())
    }

    /// #277's ceiling: presume every working subagent for this harness is
    /// gone (no event from any of them in `DELEGATION_CEILING_MS`, so a
    /// signal was almost certainly lost). Leaves attach-time-only entries
    /// alone; they were never counted as working, and the disk stays the
    /// eventual source of truth if one really is still running (its next
    /// event re-records it). Returns how many were dropped so the caller
    /// can name the count in its warning; non-zero means changed.
    pub fn presume_gone(&mut self, harness_id: &str) -> usize {
        let Some(list) = self.live.get_mut(harness_id) else {
            return 0;
        };
        let before = list.len();
        list.retain(|e| e.from_attach);
        let n = before - list.len();
        if list.is_empty() {
            self.live.remove(harness_id);
        }
        n
    }
}

#[cfg(test)]
#[path = "subagents_tests.rs"]
mod tests;
