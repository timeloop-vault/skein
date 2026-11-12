//! opencode half of the L2c translator (epic #50, L2c-2), ported from
//! `app/src/harnessEventsOpencode.ts` `translateOpencode`: each
//! [`OpencodeEvent`] from the embedded-server SSE adapter becomes a call on
//! the [`PhaseMachine`]. Pure: the caller owns the machine and the clock.
//!
//! Not ported (stays in the webview, none of it drives the phase):
//! session capture/follow on `SessionCreated` / `RootSessionPrompted`
//! (`sessionTracking.ts`, `harnessEventsOpencode.ts:204-255`),
//! `observedAgents.record` on `UserMessageAgent` (`:267-271`) and
//! `observedAgents.forget` on detach (`:130`), and the attach/stale-handler
//! guard `beginAttach`/`endAttach`/`isLive` (`:94,100,128`), folded into the
//! translator's `live` flag: [`OpencodeTranslator::apply`] drops events while
//! it is not attached.

use super::pending_prompts::{PendingPhase, PendingPrompts};
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_phase::{PhaseMachine, TransitionSource};

/// Per-harness translator state: the permission/question ids still
/// outstanding. Phase decisions depend on nothing else (no event is
/// filtered by session: a child session's prompt blocks the user too).
#[derive(Debug, Default)]
pub struct OpencodeTranslator {
    pending: PendingPrompts,
    /// True between [`Self::attached`] and [`Self::detached`]: TS drops events
    /// when the attach is closed or not live (`harnessEventsOpencode.ts:94-100`).
    live: bool,
}

impl OpencodeTranslator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether events are currently applied.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The adapter attached: mark the harness authoritative synchronously,
    /// before any event (`attachOpencodeEvents`, TS `:109`).
    pub fn attached(&mut self, m: &mut PhaseMachine, id: &str, _now_ms: i64) {
        self.live = true;
        m.attach_authoritative_source(id);
    }

    /// The adapter went away (unsubscribe, TS `:125-138`): drop authority and
    /// forget outstanding ids, since nothing will ever resolve them (#86).
    pub fn detached(&mut self, m: &mut PhaseMachine, id: &str, now_ms: i64) {
        self.live = false;
        m.detach_authoritative_source(id, now_ms);
        self.forget_pending(m, id);
    }

    /// Translate one event. Includes the `adapterDelivered` call the TS
    /// channel handler makes before translating (`:102-104`): `connected`
    /// re-arms authority itself, and `session_end` is the loss of it, so
    /// neither restores authority.
    pub fn apply(&mut self, m: &mut PhaseMachine, id: &str, event: &OpencodeEvent, now_ms: i64) {
        if !self.live {
            return;
        }
        let restores_authority =
            !matches!(event, OpencodeEvent::SessionEnd | OpencodeEvent::Connected);
        m.adapter_delivered(id, restores_authority, now_ms);
        self.translate(m, id, event, now_ms);
    }

    /// `forgetPending`: clear the ids and mirror "no question open" (#672).
    fn forget_pending(&mut self, m: &mut PhaseMachine, id: &str) {
        self.pending.forget();
        self.sync_question(m, id);
    }

    /// #672: mirror "a question prompt is open" into the activity record,
    /// which `canSendPrompt` reads. Call after every pending mutation.
    fn sync_question(&self, m: &mut PhaseMachine, id: &str) {
        m.set_question_open(id, self.pending.has_question());
    }

    /// Apply a [`PendingPhase`] derived from the pending sets. `permission`
    /// re-asserts (idempotent if already there); `running` needs
    /// `clears_permission` since draining every pending prompt is exactly the
    /// signal that the gate is gone (#86).
    fn apply_pending_phase(
        m: &mut PhaseMachine,
        id: &str,
        phase: PendingPhase,
        source: TransitionSource,
        now_ms: i64,
    ) {
        match phase {
            PendingPhase::Permission => {
                m.set_permission_from_adapter(id, source, None, None, None, now_ms);
            }
            PendingPhase::Waiting => m.set_waiting_from_adapter(id, source, now_ms),
            PendingPhase::Running => m.set_running_from_adapter(id, source, true, now_ms),
        }
    }

    fn translate(&mut self, m: &mut PhaseMachine, id: &str, event: &OpencodeEvent, now_ms: i64) {
        use TransitionSource as S;
        match event {
            OpencodeEvent::Connected => {
                // Re-arm authoritative on every successful (re)connect.
                // First connect: `attached` already armed it; a no-op.
                // Reconnect: SessionEnd previously detached authoritative
                // so L2a could take over during the outage; now that we're
                // back on the wire, adapter phases should win again.
                //
                // Phase change isn't done here; the SessionBaselineIdle the
                // adapter emits right after Connected handles the baseline
                // state. This arm must stay ungated: it re-fires on every
                // reconnect.
                //
                // #86: a reconnect replays nothing, so any permission/
                // question ids we were tracking are gone for good; the
                // SessionBaselineIdle that follows puts the harness in
                // `waiting`, not stuck in `permission` forever.
                self.forget_pending(m, id);
                m.attach_authoritative_source(id);
            }
            // Session capture/follow is the webview's (see module docs):
            // no phase effect.
            OpencodeEvent::SessionCreated { .. }
            | OpencodeEvent::RootSessionPrompted { .. }
            | OpencodeEvent::UserMessageAgent { .. } => {}
            OpencodeEvent::SessionBusy => {
                // #86: "still working": must not clear a pending
                // permission, so `clears_permission` is false.
                m.set_running_from_adapter(id, S::L2c2OpencodeBusy, false, now_ms);
            }
            OpencodeEvent::MessageDelta => {
                m.set_running_from_adapter(id, S::L2c2OpencodeMessageDelta, false, now_ms);
            }
            OpencodeEvent::ToolUseStart { .. } => {
                m.set_running_from_adapter(id, S::L2c2OpencodeToolUse, false, now_ms);
            }
            OpencodeEvent::PermissionAsked { request_id, .. } => {
                self.pending.permission_asked(request_id);
                self.sync_question(m, id);
                m.set_permission_from_adapter(
                    id,
                    S::L2c2OpencodePermission,
                    None,
                    None,
                    None,
                    now_ms,
                );
            }
            OpencodeEvent::PermissionReplied { request_id, .. } => {
                let next = self.pending.permission_replied(request_id);
                self.sync_question(m, id);
                Self::apply_pending_phase(m, id, next, S::L2c2OpencodePermissionReplied, now_ms);
            }
            OpencodeEvent::QuestionAsked { request_id, .. } => {
                // Outranked by a pending permission: `apply_pending_phase`
                // re-asserts `permission` rather than overwriting it with
                // `waiting` in that case.
                let next = self.pending.question_asked(request_id);
                self.sync_question(m, id);
                Self::apply_pending_phase(m, id, next, S::L2c2OpencodeQuestion, now_ms);
            }
            OpencodeEvent::QuestionResolved { request_id, .. } => {
                let next = self.pending.question_resolved(request_id);
                self.sync_question(m, id);
                Self::apply_pending_phase(m, id, next, S::L2c2OpencodeQuestionResolved, now_ms);
            }
            OpencodeEvent::SessionIdle => {
                // The signal we built this for: opencode finished its turn
                // and is awaiting user input. Unconditional (#86): whatever
                // was pending is moot once the turn itself ends.
                self.forget_pending(m, id);
                m.set_waiting_from_adapter(id, S::L2c2OpencodeIdle, now_ms);
            }
            OpencodeEvent::SessionBaselineIdle => {
                // #175: opencode sends no baseline, so each (re)connect
                // assumes idle (a turn that ended during an outage must not
                // stick on running). A guess, not a transition: applied
                // quietly (never notifies). Also clears `permission`, whose
                // pending ids the reconnect already forgot.
                self.forget_pending(m, id);
                m.set_baseline_waiting_from_adapter(id, S::L2c2OpencodeBaseline, now_ms);
            }
            OpencodeEvent::SessionEnd => {
                // Server disconnected after a successful connect. Drop
                // authoritative so L2a takes over until the adapter
                // reconnects (it keeps trying with backoff while the PTY
                // lives). #86: also drop any pending permission/question
                // ids; nothing will resolve them across the gap.
                self.forget_pending(m, id);
                m.release_permission(id, S::AdapterDetached, now_ms);
                m.detach_authoritative_source(id, now_ms);
            }
        }
    }
}
