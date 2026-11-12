//! Harness lifecycle and adapter authority. Port of
//! `app/src/harnessActivityLifecycle.ts`; the shared state lives in
//! `machine.rs`.

// The doc comments are the ported spec and quote TS identifiers verbatim.
#![allow(clippy::doc_markdown)]

use super::machine::PhaseMachine;
use super::types::{ActivityPhase, DegradedBy, HarnessActivity, TransitionSource};

/// Why the JSONL tail was re-pointed onto a different session id (#116, #539).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionSwitch {
    Clear,
    Resume,
    Fork,
    /// #539, a verified adoption of a second startup id: forgets and
    /// disarms like the others but changes NO phase.
    Startup,
}

impl PhaseMachine {
    /// Record a fresh spawn. Call once `harness_ensure_running` resolves
    /// successfully. Replaces any existing record for `id` (keeping its
    /// place in the iteration order).
    pub fn spawned(&mut self, id: &str, now_ms: i64) {
        self.insert(
            id,
            HarnessActivity::fresh(ActivityPhase::Spawning, None, now_ms),
        );
        self.emit(id);
    }

    /// #731: the view attached to a process that was already running, so the
    /// fresh `spawning` record is a lie. Call after the adapters attached: a
    /// harness with an authoritative adapter is left to it; any other kind
    /// (shell, copilot) moves to `idle`, what a quiet running process reads as.
    /// Never notifies (`isNotifiable`). A pending permission state is not
    /// carried across a re-attach; it moves with the phase machine (#732).
    pub fn reattached(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.get(id) else {
            return;
        };
        if cur.authoritative || cur.phase != ActivityPhase::Spawning {
            return;
        }
        self.set_phase(
            id,
            ActivityPhase::Idle,
            TransitionSource::Reattached,
            now_ms,
        );
    }

    /// Record whether #215's config injection happened for this spawn.
    /// Call once `harness_ensure_running` resolves. No-op for a harness we've
    /// already forgotten (a cancelled spawn racing unmount).
    pub fn set_injected(&mut self, id: &str, injected: bool) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if cur.injected == injected {
            return;
        }
        cur.injected = injected;
        self.emit(id);
    }

    /// Mark a harness as having an authoritative L2c adapter
    /// attached (Claude JSONL tail, opencode event stream, ...).
    /// While set, the L2a idle tick skips this harness: only
    /// `setRunningFromAdapter` / `setWaitingFromAdapter` and
    /// `exited` write its phase. Idempotent. Does not emit.
    pub fn attach_authoritative_source(&mut self, id: &str) {
        if let Some(cur) = self.store.get_mut(id) {
            cur.authoritative = true;
        }
    }

    /// The L2c adapter delivered an event: call before translating
    /// every one. Disarms the silent-adapter watchdog for this spawn,
    /// and if the watchdog had already given up (a false alarm: say the
    /// Enter answered a trust dialog and no prompt followed in time),
    /// hands authority back so the adapter's phases win again. #259.
    ///
    /// #422: an adapter that speaks while `authoritative` is false and
    /// no watchdog fired (its own `session_end` detached it, and the
    /// transcript then reappeared on the same channel) takes authority
    /// back too. Pass `restores_authority: false` for the event that
    /// itself announces the adapter's loss (`session_end`); the TS default
    /// when the option is omitted is `true`.
    pub fn adapter_delivered(&mut self, id: &str, restores_authority: bool, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        // #423: stamped on every call, ahead of the early return below.
        cur.last_adapter_event = Some(super::types::AdapterEvent {
            at: now_ms,
            restores_authority,
        });
        let regrant = restores_authority
            && !cur.authoritative
            && !cur.adapter_silent
            && cur.phase != ActivityPhase::Exited;
        if cur.adapter_heard && !cur.adapter_silent && !regrant {
            return;
        }
        if cur.adapter_silent {
            tracing::info!(
                target: "skein::activity",
                "[skein] harness {id}: adapter recovered; handing phase back to it"
            );
        } else if regrant {
            tracing::info!(
                target: "skein::activity",
                "[skein] harness {id}: adapter delivered while not authoritative; restoring its authority (#422)"
            );
        }
        let restore = cur.adapter_silent || regrant;
        cur.adapter_heard = true;
        cur.adapter_silent = false;
        cur.degraded_by = None;
        if restore {
            cur.authoritative = true;
        }
    }

    /// #273: the harness's own CLI reported its own launch (Claude's
    /// `SessionStart` hook, relayed via `skein://harness-session-start`).
    /// No-op for an unknown harness or one already `exited`. `source` is
    /// `L2c1ClaudeSessionStart` unless the caller (Hermes) says otherwise.
    ///
    /// Always records `launch_signal_at` (even on a second call: Claude
    /// is known to fire `SessionStart` twice for a "phantom" session
    /// within a few hundred ms, anthropics/claude-code#78455, so this
    /// must be idempotent and carry no consume-once side effect; a
    /// re-record is harmless since nothing here treats it as an edge).
    ///
    /// Only the launch-silent watchdog's diagnosis is voided by this
    /// event: `recovering` below checks `degraded_by`, not just
    /// `adapter_silent`. A launch signal means the harness IS alive and
    /// sitting at its prompt, which is exactly what
    /// `degrade_launch_silent_adapter` had no proof of; it says nothing at
    /// all about what `degrade_silent_adapter` (#259) diagnosed: a
    /// PROMPTED tail that stayed mute, which usually means the tail is
    /// watching the wrong file entirely. A `SessionStart` hook firing
    /// proves nothing about that, so an adapter-silent harness must
    /// stay exactly as the #259 watchdog left it: `authoritative`
    /// false, `adapter_silent` true, phase untouched. Only
    /// `adapter_delivered` (the tail actually speaking) may recover
    /// that one. `launch_signal_at` is still recorded either way; that
    /// fact is true regardless of which watchdog fired.
    ///
    /// When it does recover (the launch-silent case), it restores the
    /// harness exactly the way `adapter_delivered` does: authority
    /// back, `adapter_silent` and `degraded_by` cleared, same
    /// recovery log line: the "accepted the trust dialog late" case.
    ///
    /// Moves phase to `waiting` only when `!adapter_heard` AND the
    /// phase is `spawning` or the harness had just been recovered from
    /// `adapter_silent`. Never while `phase == Permission` (#86
    /// outranks everything). The `!adapter_heard` guard is load-
    /// bearing, not incidental: the hook entry is matcher-less, so it
    /// fires for every `SessionStart` source (`startup`, but also
    /// `resume`/`clear`/`compact`/`fork` mid-session) and `source`
    /// is deliberately not on the event payload, so this guard is the
    /// only thing telling a genuine launch apart from a `clear`/
    /// `compact` firing mid-conversation: by the time either of those
    /// happens, the transcript tail has always already spoken, and a
    /// live adapter is the better authority (it may correctly know the
    /// session is mid-turn, which this event can't say either way).
    /// A second `note_launch_signal` while already `waiting` is a no-op
    /// by construction: `set_phase`'s same-phase short-circuit.
    pub fn note_launch_signal(&mut self, id: &str, source: TransitionSource, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        let recovering = cur.adapter_silent && cur.degraded_by == Some(DegradedBy::LaunchSilent);
        if recovering {
            tracing::info!(
                target: "skein::activity",
                "[skein] harness {id}: launch signal arrived; handing phase back to it"
            );
        }
        let (phase, adapter_heard) = (cur.phase, cur.adapter_heard);
        cur.launch_signal_at = Some(now_ms);
        if recovering {
            cur.adapter_silent = false;
            cur.degraded_by = None;
            cur.authoritative = true;
        }
        if phase == ActivityPhase::Permission {
            return;
        }
        if !adapter_heard && (phase == ActivityPhase::Spawning || recovering) {
            self.set_phase(id, ActivityPhase::Waiting, source, now_ms);
        }
    }

    /// #116: `/clear` or an in-tool `/resume` re-points the JSONL tail
    /// onto a different session id mid-process (see
    /// `sessionTracking.ts`'s `followedSession`, which decides *whether*
    /// to follow; this is what the harness's state needs once the
    /// caller has decided to). `source` only picks which
    /// `TransitionSource` the phase move is attributed to; the rest of
    /// the handling is identical for all of them.
    ///
    /// The subagent registry is keyed by harness, not session, so a
    /// subagent still tracked as live under the old session would
    /// otherwise keep `subagents.workingCount` non-zero forever: the
    /// new session's own tail never reports its end, since it never
    /// started it. Forgetting here means the new session starts clean;
    /// disarming any armed #277 deferral for the same reason
    /// `detach_authoritative_source` does: the old session's
    /// `awaiting_prompt` that would have resolved it is never coming.
    ///
    /// If the phase is `running`, move it to `waiting`: by the time
    /// this fires, the picker has already returned Claude to its prompt:
    /// for `/clear` that's because nothing is written to the
    /// transcript for `/clear` itself (sampled: no row); for `/resume`
    /// it's because the picker only returns once a conversation has been
    /// chosen and reattached. Either way no ordinary end-of-turn event
    /// is coming to do this move, and the re-attach's own initial event
    /// (LiveTerminal, keyed off the `sessionId` prop) then reflects the
    /// resumed/cleared transcript from here on. Every other phase is
    /// left alone: `permission` outranks it (#86), and `exited` /
    /// `spawning` / `idle` / `waiting` aren't this event's business.
    /// No-op for an unknown id.
    ///
    /// `Startup` (#539) forgets and disarms the same way but changes NO
    /// phase: Claude never returned to a prompt, and the re-attached
    /// tail's replay owns the phase.
    pub fn session_switched(&mut self, id: &str, source: SessionSwitch, now_ms: i64) {
        let Some(cur) = self.get(id) else {
            return;
        };
        let phase = cur.phase;
        self.subagents.forget(id);
        self.background_tasks.forget(id);
        self.disarm_delegation(id);
        if source == SessionSwitch::Startup || phase != ActivityPhase::Running {
            return;
        }
        let transition = match source {
            SessionSwitch::Clear => TransitionSource::L2c1ClaudeSessionClear,
            SessionSwitch::Resume => TransitionSource::L2c1ClaudeSessionResume,
            SessionSwitch::Fork | SessionSwitch::Startup => TransitionSource::L2c1ClaudeSessionFork,
        };
        self.set_phase(id, ActivityPhase::Waiting, transition, now_ms);
    }

    /// Adapter detached: fall back to the L2a heuristic for this
    /// harness. The next chunk or tick will re-evaluate.
    ///
    /// #277: also disarms any armed deferral. An adapter that is gone
    /// can never resolve the end of turn it deferred: its own
    /// `awaiting_prompt` won't come again, and neither will the
    /// subagent events `delegation_activity_at` needs, since (today) the
    /// same tail feeds both. Left armed, the deferral would keep
    /// steering the harness's phase (Rule 3/4 evaluate it BEFORE the
    /// tick's `authoritative` skip) even though authority has already
    /// reverted to L2a, so the dot would read `running` /
    /// "delegating · N agents" regardless of PTY truth for up to
    /// `DELEGATION_CEILING_MS`, instead of `session_end`'s promised
    /// fallback to chunk-driven detection. Called unconditionally,
    /// ahead of the `authoritative` guard below: `disarm_delegation` is
    /// itself idempotent (no-ops once `delegation_deferred_at` is
    /// already `None`), and a second detach call on an already-
    /// detached harness has nothing left to disarm, so running it
    /// on that path too is free, not merely harmless.
    pub fn detach_authoritative_source(&mut self, id: &str, now_ms: i64) {
        self.disarm_delegation(id);
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if !cur.authoritative {
            return;
        }
        cur.authoritative = false;
        cur.authority_lost_at = Some(now_ms);
    }

    /// Record PTY exit. Transitions to `exited` regardless of prior
    /// phase. Idempotent.
    pub fn exited(&mut self, id: &str, code: Option<i32>, now_ms: i64) {
        let Some(cur) = self.get(id) else {
            // A late exit for a harness we never saw spawn (shouldn't
            // normally happen but worth recording so consumers see
            // the truth). Never had a deferral armed, so the #277
            // fields are just their spawn defaults.
            self.insert(
                id,
                HarnessActivity::fresh(ActivityPhase::Exited, code, now_ms),
            );
            self.emit(id);
            return;
        };
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        // #277 Rule 2: an exited harness can't still be waiting on its
        // subagents.
        self.disarm_delegation(id);
        self.set_phase_with(
            id,
            ActivityPhase::Exited,
            TransitionSource::PtyExit,
            move |a| a.exit_code = code,
            now_ms,
        );
    }

    /// Drop a harness from the store entirely. Call on unmount so we don't
    /// accumulate dead entries. Per-id subscriptions are not touched; the
    /// id is marked changed so subscribers re-read `None`.
    pub fn forget(&mut self, id: &str) {
        // A stored screen reading outlives the record (a reading can arrive
        // before the spawn), so the screen and mute state go regardless.
        self.mute_until.remove(id);
        self.forget_screen_phase(id);
        if !self.store.contains_key(id) {
            return;
        }
        self.remove(id);
        self.emit(id);
    }
}
