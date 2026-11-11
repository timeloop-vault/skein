//! Noticing that a transcript reappeared or shrank under the tail, and recovering from it.

use super::ClaudeEvent;
use super::adapter::TailState;
use super::background_tasks::{BackgroundState, now_ms, reconcile_background};
use super::persist::{persist_extracted_batch, scan_history};
use super::subagents::subagent_lifecycle_from_content;
use super::tick::dispatch_events;
use crate::harness_actions_claude::ActionExtractor;
use parking_lot::Mutex;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

/// How many trailing consumed bytes `TailState::fingerprint` keeps (#425).
pub(super) const FINGERPRINT_LEN: usize = 256;

/// The last up to `FINGERPRINT_LEN` bytes of `bytes` (#425).
pub(super) fn fingerprint_of(bytes: &[u8]) -> Vec<u8> {
    bytes[bytes.len().saturating_sub(FINGERPRINT_LEN)..].to_vec()
}

/// Append freshly consumed bytes to a fingerprint, keeping only the
/// last `FINGERPRINT_LEN`. A read shorter than that extends the old
/// fingerprint rather than replacing it (#425).
pub(super) fn extend_fingerprint(fp: &mut Vec<u8>, new: &[u8]) {
    fp.extend_from_slice(new);
    if fp.len() > FINGERPRINT_LEN {
        fp.drain(..fp.len() - FINGERPRINT_LEN);
    }
}

/// What a transcript that vanished and came back looks like relative
/// to what this tail had consumed (#425).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Reappear {
    /// Same bytes at the consumed offset: keep going, the rest is new.
    Same,
    /// Shorter than what we consumed.
    Truncated,
    /// Long enough, but the consumed tail bytes differ (or can't be read).
    Different,
}

impl Reappear {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Truncated => "truncated",
            Self::Different => "different",
        }
    }
}

/// Classify a reappeared transcript by length and by the fingerprint
/// of the bytes ending at `last_pos` (#425).
pub(super) fn classify_reappear(path: &Path, last_pos: u64, fp: &[u8], file_len: u64) -> Reappear {
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
pub(super) enum Resync {
    Done(Vec<ClaudeEvent>),
    Unreadable,
}

/// Resync a truncated or different transcript exactly like `attach_at`
/// reads an existing file (#425): fresh `ActionExtractor`, then
/// `scan_history` and `persist_extracted_batch` (only rows newer than the max persisted
/// timestamp, no broadcast, no baseline capture), `last_pos` at the
/// content length. Returns the derived initial phase event, which the
/// caller emits after dropping the lock. `Unreadable` means the file
/// could not be read as UTF-8; state is untouched so the caller can retry.
pub(super) fn resync_as_backfill(s: &mut TailState) -> Resync {
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
    // Background tasks (#445): same no-replay rule. The tracker is
    // rebuilt from the new main content and the subagent files already
    // tailed, with every transition discarded; a task outstanding across
    // the resync is not re-announced.
    // What was announced, and who owns what, carries over, so the
    // reconcile below can close (or keep) exactly those, and close an
    // owned one as `subagent_ended` even when the new content no longer
    // shows its start.
    let mut background = BackgroundState {
        announced: std::mem::take(&mut s.background.announced),
        owner: std::mem::take(&mut s.background.owner),
        ..BackgroundState::default()
    };
    let mut local_command = LocalCommandTracker::default();
    let (init, fresh) = scan_history(
        &content,
        s.actions.as_mut(),
        &mut background,
        &mut local_command,
    );
    for (agent_id, tail) in &s.subagents {
        if let Ok(sub) = fs::read_to_string(&tail.path) {
            subagent_lifecycle_from_content(&sub, &mut background, agent_id);
        }
    }
    let background_events = reconcile_background(&mut background, &s.subagents, now_ms());
    s.background = background;
    if let Some(ap) = s.actions.as_ref() {
        persist_extracted_batch(ap, fresh);
    }
    s.last_pos = u64::try_from(content.len()).unwrap_or(u64::MAX);
    s.partial.clear();
    // `attach_at` starts a state with `false` after the same scan.
    s.in_assistant_turn = false;
    s.local_command = local_command;
    s.fingerprint = fingerprint_of(content.as_bytes());
    s.utf8_stall_at = None;
    s.utf8_stall_count = 0;
    s.utf8_stall_warned = false;
    Resync::Done(init.into_iter().chain(background_events).collect())
}

/// Run `resync_as_backfill`, drop the lock, and emit only the initial
/// event (if any) through `dispatch_events` — `on_event` is never called
/// under the lock (#425).
pub(super) fn finish_resync(
    mut s: parking_lot::MutexGuard<'_, TailState>,
    state: &Arc<Mutex<TailState>>,
    on_event: &(dyn Fn(ClaudeEvent) + Send + Sync),
) {
    let events = match resync_as_backfill(&mut s) {
        Resync::Done(events) => events,
        Resync::Unreadable => {
            // Retry on the next tick via the reappear path.
            s.attached = false;
            Vec::new()
        }
    };
    drop(s);
    dispatch_events(state, events, on_event);
}
