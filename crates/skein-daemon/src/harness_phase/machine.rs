//! The harness phase store and engine (epic #50, L1+L3+L2a; #259 watchdog;
//! #273 launch-silent watchdog; #277 delegation deferral). Port of
//! `app/src/harnessActivityCore.ts` plus the read methods of
//! `harnessActivity.ts`.
//!
//! [`PhaseMachine`] is the one struct. The rest of the TS store object is
//! split into sibling `impl PhaseMachine` blocks the way the TS files are:
//! `lifecycle.rs` (spawn/exit/forget, adapter authority), `io.rs` (user
//! input, PTY output) and `permission.rs` (#86).
//!
//! What differs from TS by construction:
//!
//! * Time is injected. Every method that reads `Date.now()` in TS takes
//!   `now_ms` as its last parameter. Nothing here schedules the tick: the
//!   driver calls [`PhaseMachine::tick`] once per `TICK_INTERVAL_MS`.
//! * Listeners are an outbox. [`PhaseMachine::drain_transitions`] yields what
//!   `transitionListeners` would have been called with (only on a real phase
//!   change); [`PhaseMachine::drain_changed`] yields the ids TS `emit`ted to
//!   per-id subscribers. `onTick` listeners are not ported (the driver owns
//!   the tick). The permission-id set (`recomputePermissionIds`) is a React
//!   subscription aid and is not ported either; derive it from `get`.
//! * Iteration order is insertion order, like a JS `Map`, because the tick
//!   walks the store in that order and the order of the transitions it emits
//!   is compared in the shadow run.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::background_tasks::BackgroundTasks;
use super::constants::{
    ADAPTER_SILENT_AFTER_MS, IDLE_AFTER_MS, LAUNCH_SILENT_AFTER_MS, PATTERN_WAITING_AFTER_MS,
};
use super::invariants::SupervisorMemory;
use super::patterns::matches_waiting_prompt;
use super::screen::ScreenState;
use super::subagents::Subagents;
use super::types::{ActivityPhase, DegradedBy, HarnessActivity, Transition, TransitionSource};

/// What the work-deferral part of the tick decided for one harness (the
/// boolean `tickWork` returns in TS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TickWork {
    /// `tickWork` returned `true`: the deferral/stale-work rules owned this
    /// harness for this tick, so the rest of the tick skips it.
    Handled,
    /// `tickWork` returned `false`: carry on with the authoritative /
    /// screen / L2a / L2b rules.
    NotHandled,
}

/// A closure applied to the record after `set_phase` has updated the phase:
/// the TS `patch?: Partial<HarnessActivity>` argument. Its presence (not its
/// content) is what lets a same-phase call through.
pub(super) type Patch = Box<dyn FnOnce(&mut HarnessActivity)>;

/// Per-harness phase store, keyed by harness id.
#[derive(Debug, Default)]
pub struct PhaseMachine {
    pub(super) store: HashMap<String, HarnessActivity>,
    /// Ids in `Map` insertion order (re-setting an existing id keeps its
    /// place; deleting and re-adding moves it to the end).
    order: Vec<String>,
    /// Per-harness end of the induced-output mute window (epoch ms).
    pub(super) mute_until: HashMap<String, i64>,
    transitions: Vec<Transition>,
    changed: BTreeSet<String>,
    /// #381: harnesses whose #277 deferral just armed (null -> non-null).
    /// Filled by the deferral port, drained by the driver; the TS
    /// `delegationDeferredListeners` as an outbox.
    pub(super) delegation_armed: Vec<String>,
    /// Live Claude subagents per harness (`subagents.ts`).
    pub(super) subagents: Subagents,
    /// Live background tasks per harness (`backgroundTasks.ts`).
    pub(super) background_tasks: BackgroundTasks,
    /// The Hermes screen-phase state and readings (`screen.rs`, #623).
    pub(super) screen: ScreenState,
    /// The supervisor's per-harness memory (`supervisor.rs`, #423).
    pub(super) supervisor: SupervisorMemory,
}

impl PhaseMachine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The record for `id`, if this process has one.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&HarnessActivity> {
        self.store.get(id)
    }

    /// #423: every harness id in the store, in insertion order.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.order.clone()
    }

    /// #356: every harness id currently in the store, mapped to its phase,
    /// for the `harness_phases` agent-request kind. A harness this process
    /// never spawned (or has since `forget`-ten) is simply absent: the
    /// request handler reads that omission as "unknown" per issue #356's
    /// phase rule, rather than this guessing at one.
    #[must_use]
    pub fn phase_snapshot(&self) -> BTreeMap<String, ActivityPhase> {
        self.store
            .iter()
            .map(|(id, a)| (id.clone(), a.phase))
            .collect()
    }

    /// The subagent registry, for decision-time reads.
    #[must_use]
    pub fn subagents(&self) -> &Subagents {
        &self.subagents
    }

    /// The subagent registry, for the event translators to record/finish.
    pub fn subagents_mut(&mut self) -> &mut Subagents {
        &mut self.subagents
    }

    /// The background-task registry, for decision-time reads.
    #[must_use]
    pub fn background_tasks(&self) -> &BackgroundTasks {
        &self.background_tasks
    }

    /// The background-task registry, for the event translators.
    pub fn background_tasks_mut(&mut self) -> &mut BackgroundTasks {
        &mut self.background_tasks
    }

    /// Take every transition queued since the last call, oldest first.
    pub fn drain_transitions(&mut self) -> Vec<Transition> {
        std::mem::take(&mut self.transitions)
    }

    /// Take the ids whose record changed in a way TS `emit`s for, since the
    /// last call (sorted, each once).
    pub fn drain_changed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.changed).into_iter().collect()
    }

    /// #381: take the ids whose deferral armed since the last call.
    pub fn drain_delegation_armed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.delegation_armed)
    }

    /// TS `emit(id)`: a state change per-id subscribers must hear about.
    pub(super) fn emit(&mut self, id: &str) {
        self.changed.insert(id.to_owned());
    }

    /// `store.set(id, record)`: keeps the position of an existing id.
    pub(super) fn insert(&mut self, id: &str, record: HarnessActivity) {
        if self.store.insert(id.to_owned(), record).is_none() {
            self.order.push(id.to_owned());
        }
    }

    /// `store.delete(id)`.
    pub(super) fn remove(&mut self, id: &str) -> Option<HarnessActivity> {
        let removed = self.store.remove(id);
        if removed.is_some() {
            self.order.retain(|x| x != id);
        }
        removed
    }

    /// Move `id` to `phase`. Suppresses no-op transitions so consumers don't
    /// churn on continuous output (every chunk would otherwise emit): only
    /// real phase changes queue a [`Transition`]; `last_output_at` mutates
    /// silently inside `record_output`. A patch-only call (see
    /// [`Self::set_phase_with`], e.g. re-entering `permission` with a fresh
    /// tool name) still goes through even when the phase itself didn't
    /// change, so `get` reflects the new tool. No-op for an unknown id.
    pub(super) fn set_phase(
        &mut self,
        id: &str,
        phase: ActivityPhase,
        source: TransitionSource,
        now_ms: i64,
    ) {
        self.apply_phase(id, phase, source, None, now_ms);
    }

    /// [`Self::set_phase`] with a patch, applied last (after the
    /// leaving-`permission` reset and `phase_since`), so it wins over both,
    /// as the TS spread order does.
    pub(super) fn set_phase_with(
        &mut self,
        id: &str,
        phase: ActivityPhase,
        source: TransitionSource,
        patch: impl FnOnce(&mut HarnessActivity) + 'static,
        now_ms: i64,
    ) {
        self.apply_phase(id, phase, source, Some(Box::new(patch)), now_ms);
    }

    fn apply_phase(
        &mut self,
        id: &str,
        phase: ActivityPhase,
        source: TransitionSource,
        patch: Option<Patch>,
        now_ms: i64,
    ) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if cur.phase == phase && patch.is_none() {
            return;
        }
        let from = cur.phase;
        cur.phase = phase;
        // Leaving `permission` drops the stale tool name by default; a
        // caller reporting a fresh one (re-entering `permission`) passes
        // it in the patch, which is applied after and wins.
        if from == ActivityPhase::Permission && phase != ActivityPhase::Permission {
            cur.permission_tool = None;
            cur.permission_agent_type = None;
            cur.permission_agent_id = None;
            cur.permission_at = None;
        }
        if from != phase {
            cur.phase_since = now_ms;
        }
        if let Some(patch) = patch {
            patch(cur);
        }
        let has_user_input = cur.has_user_input;
        self.emit(id);
        if from != phase {
            self.transitions.push(Transition {
                harness_id: id.to_owned(),
                from,
                to: phase,
                source,
                at_ms: now_ms,
                has_user_input,
            });
        }
    }

    /// #277: void an armed end-of-turn deferral. Called from every
    /// main-transcript work/settle signal (`setRunningFromAdapter`,
    /// `setWaitingFromAdapter`, `exited`) so the deferral can never
    /// survive past whatever event proves the session isn't waiting on
    /// its subagents anymore. Mutates in place, the way `record_output`
    /// bumps `last_output_at`: this is bookkeeping, not a phase change, so
    /// it must not emit; a caller that goes on to call `set_phase` gets
    /// its own emit from that.
    ///
    /// Deliberately unconditional on the harness's current phase: Rule 2
    /// requires this to run even when the caller it's called from is
    /// about to no-op (`setRunningFromAdapter` while still `permission`
    /// without `clearsPermission`, or `set_phase`'s own same-phase
    /// short-circuit): the disarm must not depend on a phase write
    /// actually happening.
    pub(super) fn disarm_delegation(&mut self, id: &str) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if cur.delegation_deferred_at.is_none() {
            return;
        }
        cur.delegation_deferred_at = None;
        cur.delegation_emptied_at = None;
    }

    /// The adapter never spoke after a prompt was submitted: give up on it.
    /// Without this an adapter watching the wrong place freezes the harness
    /// for good: authority silences PTY output and the tick skips it (#259,
    /// a JSONL path encoded differently from Claude's). `last_output_at`
    /// restarts so the idle window is measured from here, not from spawn.
    /// The `spawning` branch is defensive: `should_arm_watchdog` excludes
    /// `spawning` (#644), so the normal arm path never reaches it; it only
    /// matters if state is written directly or a future caller degrades a
    /// spawning harness, which the tick (it only acts on `running`) would
    /// otherwise leave stuck.
    fn degrade_silent_adapter(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        tracing::warn!(
            target: "skein::activity",
            "[skein] harness {id}: adapter delivered nothing {}s after a prompt; falling back to the idle heuristic",
            ADAPTER_SILENT_AFTER_MS / 1000
        );
        let was_spawning = cur.phase == ActivityPhase::Spawning;
        if cur.authoritative {
            cur.authority_lost_at = Some(now_ms);
        }
        cur.authoritative = false;
        cur.adapter_silent = true;
        cur.degraded_by = Some(DegradedBy::AdapterSilent);
        cur.last_output_at = Some(now_ms);
        if was_spawning {
            self.set_phase(
                id,
                ActivityPhase::Running,
                TransitionSource::AdapterSilent,
                now_ms,
            );
        }
    }

    /// #273: neither the tail nor the launch hook has said anything at all,
    /// this long after spawn, and the harness is still `spawning`: give up
    /// on the adapter the same way `degrade_silent_adapter` does, but under
    /// its own transition source (`LaunchSilent`) because the two are
    /// diagnosed differently: this one needed no prompt to arm, it fires
    /// on pure silence since spawn. Sets `adapter_silent` (not a separate
    /// flag) so `note_launch_signal`'s recovery path (the same one
    /// `adapter_delivered` already uses) applies here too if the launch
    /// signal turns up late (e.g. the user finally accepts a trust
    /// dialog). Unlike `degrade_silent_adapter`, the phase write here is
    /// unconditional: the caller's guard already confirmed `phase ==
    /// Spawning`.
    fn degrade_launch_silent_adapter(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        tracing::warn!(
            target: "skein::activity",
            "[skein] harness {id}: no launch signal {}s after spawn; falling back to the idle heuristic",
            LAUNCH_SILENT_AFTER_MS / 1000
        );
        if cur.authoritative {
            cur.authority_lost_at = Some(now_ms);
        }
        cur.authoritative = false;
        cur.adapter_silent = true;
        cur.degraded_by = Some(DegradedBy::LaunchSilent);
        cur.last_output_at = Some(now_ms);
        self.set_phase(
            id,
            ActivityPhase::Running,
            TransitionSource::LaunchSilent,
            now_ms,
        );
    }

    /// One tick of the background scan: the driver calls this once per
    /// `TICK_INTERVAL_MS` with the current time. Walks every harness in
    /// insertion order and applies, in order, the work-deferral rules, the
    /// silent-adapter watchdogs (authoritative harnesses only), the screen
    /// phase, then the L2b pattern and L2a idle heuristics.
    pub fn tick(&mut self, now_ms: i64) {
        for id in self.order.clone() {
            // #446/#441: see `tickWork` in deferral.ts (work.rs).
            if self.tick_work(&id, now_ms) == TickWork::Handled {
                continue;
            }
            let Some(a) = self.store.get(&id).cloned() else {
                continue;
            };
            // Harnesses with an authoritative source (Claude JSONL
            // tail, opencode SSE adapter) write their own phase
            // from the L2c module. The idle heuristic would fight
            // the adapter: e.g. when Claude is mid-turn but the
            // PTY is briefly quiet during a tool call, L2a would
            // flip to `idle` while L2c says `running`. Skip these,
            // unless the adapter has proven silent (#259).
            if a.authoritative {
                if !a.adapter_heard
                    && a.prompt_submitted_at
                        .is_some_and(|at| now_ms - at >= ADAPTER_SILENT_AFTER_MS)
                {
                    self.degrade_silent_adapter(&id, now_ms);
                } else if
                // #273: mirrors the #259 check's own `!adapter_heard`
                // guard: if the tail has already proven itself, this
                // timer has nothing to add, so an `else if` keeps the
                // two mutually exclusive rather than both firing (and
                // double-transitioning) in the same tick.
                !a.adapter_heard
                    && a.launch_signal_at.is_none()
                    && a.phase == ActivityPhase::Spawning
                    && now_ms - a.spawned_at >= LAUNCH_SILENT_AFTER_MS
                {
                    self.degrade_launch_silent_adapter(&id, now_ms);
                }
                continue;
            }
            // #623: a screen-read phase (Hermes) outranks the byte-stream
            // heuristics whenever the screen is recognised.
            if self.tick_screen_phase(&id, now_ms) {
                continue;
            }
            let Some(last_output_at) = a.last_output_at else {
                continue;
            };
            if a.phase != ActivityPhase::Running {
                continue;
            }
            let quiet = now_ms - last_output_at;
            // L2b: pattern-match -> waiting on a shorter threshold
            // than idle. Triggered when output has been quiet for
            // PATTERN_WAITING_AFTER_MS (500 ms) AND the tail buffer
            // matches a known prompt regex (sudo password, [y/n],
            // "Press Enter", ...). The shorter threshold makes the
            // dot flip blue near-instantly when a sudo prompt
            // appears, instead of waiting the full 8 s idle window.
            //
            // If the tail doesn't match a known prompt, the
            // harness falls through to the L2a `-> idle` check
            // below: same behaviour as pre-L2b.
            if quiet >= PATTERN_WAITING_AFTER_MS && matches_waiting_prompt(&a.tail) {
                self.set_phase(
                    &id,
                    ActivityPhase::Waiting,
                    TransitionSource::L2bPattern,
                    now_ms,
                );
                continue;
            }
            if quiet >= IDLE_AFTER_MS {
                self.set_phase(&id, ActivityPhase::Idle, TransitionSource::L2aIdle, now_ms);
            }
        }
    }
}

/// #381: is this harness at a point where pasting and submitting a
/// prompt through the #238 seam (`canSendPrompt`/`sendPrompt`) is safe?
/// `waiting` (end of turn) always is. So is `running` with an armed
/// #277 delegation deferral: the main session already ended its own
/// turn and only stayed `running` because background subagents were
/// still working (see `awaitingPromptFromAdapter`); a prompt landing
/// now is exactly as safe as one landing once the deferral flushes to
/// `waiting` on its own. `permission` is never a stopping point even
/// with the deferral armed underneath it: a dialog is a harder stop
/// than "not waiting" everywhere else this interaction shows up (#86
/// outranks #277), and this reads the same way: a `permission` phase
/// fails the `phase == Running` check below regardless of
/// `delegation_deferred_at`.
#[must_use]
pub fn at_safe_stopping_point(a: &HarnessActivity) -> bool {
    a.phase == ActivityPhase::Waiting
        || (a.phase == ActivityPhase::Running && a.delegation_deferred_at.is_some())
}

/// Shared arm-guard for the #259 silent-adapter watchdog, factored out
/// so `record_input` (a keystroke) and `note_prompt_submitted` (#363, a
/// seam-submitted prompt) can't drift apart: true only when an authoritative
/// adapter is attached, has said nothing yet, and no timer is already running
/// for this spawn. Each caller adds its own proof a prompt was actually
/// submitted on top of this: `record_input` checks `data` for a `\r`/`\n`;
/// `note_prompt_submitted`'s caller, `sendPrompt`, IS the submit, so it needs
/// nothing further. Never while `spawning` (#644): until the CLI has proven
/// it is at its prompt (launch signal #273 or the adapter speaking) an Enter
/// can't be a prompt submit: it answers the trust dialog. A harness stuck in
/// spawning is the #273 launch-silent watchdog's job, so nothing is lost.
#[must_use]
pub(super) fn should_arm_watchdog(cur: &HarnessActivity) -> bool {
    cur.authoritative
        && !cur.adapter_heard
        && cur.prompt_submitted_at.is_none()
        && cur.phase != ActivityPhase::Spawning
}

/// Is this keystroke "decisive" enough to count as answering a
/// permission dialog (#86)? Claude gives us no explicit "answered"
/// signal (approval and denial both just let the tool proceed or
/// error out, and that can take minutes) so the only prompt signal
/// left is the user's own keystroke. Deliberately narrow: a lone ESC
/// (dismiss) counts, but an escape *sequence* (arrow keys `\x1b[A`,
/// xterm's focus events `\x1b[I` / `\x1b[O`) must not, since those
/// fire on ordinary navigation and window-focus changes that have
/// nothing to do with answering anything. Pure so it can be unit tested
/// without the store.
#[must_use]
pub fn is_decisive_input(data: &str) -> bool {
    if data.contains('\r') || data.contains('\n') {
        return true;
    }
    if data == "\u{1b}" {
        return true;
    }
    if data.contains('\u{03}') {
        return true;
    }
    let mut chars = data.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_digit() || "yYnN".contains(c))
}
