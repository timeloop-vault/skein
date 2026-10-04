//! History scans and the batched writes of extracted actions into `harness_actions`.

use super::ClaudeEvent;
use super::adapter::ActionPersistence;
use super::background_tasks::BackgroundState;
use super::parse::apply_initial_state_row;
use crate::db::Database;
use crate::harness_kind::HarnessKind;
use skein_harness::claude::local_command::LocalCommandTracker;

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
///
/// `background` (#445) sees every row too, with no sink: the tracker is
/// primed, nothing is emitted.
pub(super) fn scan_history(
    content: &str,
    mut actions: Option<&mut ActionPersistence>,
    background: &mut BackgroundState,
    local_command: &mut LocalCommandTracker,
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
        apply_initial_state_row(&value, &mut last, local_command);
        background.feed(&value, None, None);
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
pub(super) fn persist_extracted(
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
            Some(HarnessKind::Claude.as_str()),
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
                            Some(HarnessKind::Claude.as_str()),
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
pub(super) fn persist_extracted_batch(
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
    if let Err(e) = ap.db.record_harness_actions(
        &ap.harness_id,
        &ap.room_id,
        Some(HarnessKind::Claude.as_str()),
        &rows,
    ) {
        // max_ts didn't advance, so the next attach retries this same batch.
        tracing::warn!(harness_id = %ap.harness_id, rows = rows.len(), error = %e,
            "claude_events: batch backfill insert failed");
    }
}

/// Query the largest `timestamp_ms` already persisted for this
/// harness. Returns 0 when the harness has no rows yet (first
/// attach). Read errors fall back to 0 — re-inserting rows is
/// recoverable, missing the backfill entirely is not.
pub(super) fn max_persisted_ts_ms(db: &Database, harness_id: &str) -> i64 {
    db.recent_harness_actions_by_harness(harness_id, -1, 1)
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .map_or(0, |r| r.timestamp_ms)
}
