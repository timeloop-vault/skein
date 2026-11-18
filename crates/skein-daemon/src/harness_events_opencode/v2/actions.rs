//! Live `harness_actions` rows from the v2 event stream (#678). The frames
//! the phase translator reads are also fed to a [`V2ActionTracker`]; rows
//! are recorded and broadcast through v1's `record_and_emit`, so a live
//! `patch` row captures its review baseline and emits `harness-action`.

use std::sync::Arc;

use skein_harness::opencode::v2::{V2Action, V2ActionKind, parse_action_event};

use super::super::frames::record_and_emit;
use crate::db::Database;
use crate::db::action_kind;
use crate::event_sink::SharedEventSink;
use crate::harness_actions_opencode::ExtractedAction;
use crate::harness_actions_opencode::v2::V2ActionTracker;

/// Who the rows belong to, and where they go. One per adapter, kept across
/// reconnects like the translator's session knowledge.
pub(in crate::harness_events_opencode) struct ActionRecorder {
    pub(in crate::harness_events_opencode) db: Arc<Database>,
    pub(in crate::harness_events_opencode) events: Option<SharedEventSink>,
    pub(in crate::harness_events_opencode) harness_id: String,
    pub(in crate::harness_events_opencode) room_id: String,
    pub(in crate::harness_events_opencode) cwd: String,
    tracker: V2ActionTracker,
}

impl ActionRecorder {
    pub(in crate::harness_events_opencode) fn new(
        db: Arc<Database>,
        events: Option<SharedEventSink>,
        harness_id: String,
        room_id: String,
        cwd: String,
    ) -> Self {
        Self {
            db,
            events,
            harness_id,
            room_id,
            cwd,
            tracker: V2ActionTracker::default(),
        }
    }

    /// Record the rows of one frame payload. `root_of` answers what is known
    /// of a session (`Some(true)` root, `Some(false)` child, `None` unknown).
    pub(super) fn record_frame(&mut self, payload: &str, root_of: impl Fn(&str) -> Option<bool>) {
        let Some(action) = parse_action_event(payload) else {
            return;
        };
        let root = root_of(&action.session_id);
        for row in rows_for(&mut self.tracker, &action, root) {
            record_and_emit(
                &self.db,
                self.events.as_deref(),
                &self.harness_id,
                &self.room_id,
                &self.cwd,
                &row,
            );
        }
    }
}

/// The rows to record for one frame. Tool, reasoning and cost rows of any
/// session are kept; a user prompt of a KNOWN child session is dropped (it
/// is the delegation text, not something the user typed). An unknown session
/// is shown, as in v1 (#525): a missing prompt makes the feed look dead.
pub(super) fn rows_for(
    tracker: &mut V2ActionTracker,
    action: &V2Action,
    root: Option<bool>,
) -> Vec<ExtractedAction> {
    let rows = tracker.observe(action);
    if matches!(action.kind, V2ActionKind::InboxEnqueued { .. }) && root == Some(false) {
        return rows
            .into_iter()
            .filter(|r| r.kind != action_kind::USER_PROMPT)
            .collect();
    }
    rows
}
