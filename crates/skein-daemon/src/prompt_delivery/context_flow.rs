//! The context-limit nudge's delivery (#689): when a harness reaches a
//! stopping point, compare its newest context size with the user's limit and,
//! once per crossing, paste the nudge through the [`Sender`]. The decision is
//! `context_nudge::decide`; this layer reads and stores the mark, sends, and
//! keeps rechecking for a while because opencode's `turn_cost` row can land
//! after the transition to `waiting` (separate SSE frames).
//!
//! Pure and clock-injected like [`Mail`]; [`DeliveryCore`] routes events in
//! (after the mail layer, so mail has priority) and the driver calls
//! `on_timer` at `next_deadline`.
//!
//! [`Mail`]: super::mail::Mail
//! [`DeliveryCore`]: super::core::DeliveryCore

use std::collections::BTreeMap;

use super::context_nudge::{Decision, Mark, decide, render};
use super::env::DeliveryEnv;
use super::gate::at_safe_stopping_point;
use super::mail_nudge::should_check_on_transition;
use super::send::{SendEvent, SendOpts, SendStart, Sender};
use crate::harness_phase::ActivityPhase;

/// How often a `waiting` harness is rechecked.
pub const RECHECK_INTERVAL_MS: i64 = 2_000;
/// How long rechecks last when the check found nothing due.
pub const QUIET_WINDOW_MS: i64 = 10_000;
/// How long rechecks last while a due nudge keeps being refused.
pub const REFUSED_WINDOW_MS: i64 = 60_000;

/// What one check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Nothing due (or the lookup failed): keep watching for a late row.
    Quiet,
    /// A nudge is due but was refused: keep trying.
    Refused,
    /// A mark was stored or a nudge sent: this crossing is dealt with.
    Settled,
}

#[derive(Debug, Clone, Copy)]
struct Recheck {
    armed_at: i64,
    due: i64,
    until: i64,
}

#[derive(Debug, Default)]
pub struct ContextFlow {
    rechecks: BTreeMap<String, Recheck>,
    /// The mark from before a nudge was pasted, kept until its send ends.
    sent: BTreeMap<String, Mark>,
}

fn persist(env: &mut dyn DeliveryEnv, id: &str, mark: Mark) {
    if let Err(e) = env.set_context_mark(id, mark) {
        tracing::warn!(harness_id = id, error = %e, "context nudge: storing the mark failed (#689)");
    }
}

impl ContextFlow {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A new process: stop rechecking the old one and mark the newest row as
    /// seen, so a row left by the previous session or written while the
    /// daemon was down is never judged a new turn. A harness with no mark
    /// yet gets its first one (a first turn that passes the limit is then
    /// nudged); an existing mark keeps its `armed` unless the stale row is
    /// below the limit (or there is no limit), which arms it.
    pub fn spawned(&mut self, env: &mut dyn DeliveryEnv, id: &str) {
        self.rechecks.remove(id);
        self.sent.remove(id);
        let (Ok(mark), Ok(newest)) = (env.context_mark(id), env.newest_context(id)) else {
            return;
        };
        let limit = env.context_settings().limit;
        match (mark, newest) {
            (None, _) => {
                if let Decision::Store(m) = decide(None, newest, limit) {
                    persist(env, id, m);
                }
            }
            (Some(m), Some((row, tokens))) if m.last_row_id != Some(row) => {
                let armed = m.armed || limit.is_none_or(|l| tokens < l);
                persist(
                    env,
                    id,
                    Mark {
                        last_row_id: Some(row),
                        armed,
                    },
                );
            }
            _ => {}
        }
    }

    /// What a finished send says about a context nudge in flight. A submit
    /// that was skipped leaves the text unsent, so the crossing is due
    /// again: put the previous mark back.
    pub fn on_send_event(&mut self, env: &mut dyn DeliveryEnv, ev: &SendEvent) {
        match ev {
            SendEvent::SubmitSkipped { harness_id, .. } => {
                if let Some(before) = self.sent.remove(harness_id) {
                    tracing::info!(harness_id, "context nudge submit skipped; due again (#689)");
                    persist(env, harness_id, before);
                }
            }
            SendEvent::Finished { harness_id } => {
                self.sent.remove(harness_id);
            }
            _ => {}
        }
    }

    /// #383: the draft cleared or user input was reported. A waiting harness
    /// whose nudge was held back by a draft is checked now, past the
    /// refused-recheck window.
    pub fn on_client_facts(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        now_ms: i64,
    ) {
        let waiting = env
            .gate_facts(id, now_ms)
            .phase
            .is_some_and(|p| p.phase == ActivityPhase::Waiting);
        if !waiting {
            return;
        }
        match check(sender, env, id, now_ms, &mut self.sent) {
            Outcome::Settled => {
                self.rechecks.remove(id);
            }
            Outcome::Quiet => {}
            Outcome::Refused => {
                self.rechecks.entry(id.to_owned()).or_insert(Recheck {
                    armed_at: now_ms,
                    due: now_ms + RECHECK_INTERVAL_MS,
                    until: now_ms + REFUSED_WINDOW_MS,
                });
            }
        }
    }

    /// The phase record was dropped (the harness stopped, was removed or its
    /// room archived): stop rechecking. The persisted mark stays; the room
    /// sweep removes it with the room.
    pub fn forget(&mut self, id: &str) {
        self.rechecks.remove(id);
        self.sent.remove(id);
    }

    pub fn on_transition(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        to: ActivityPhase,
        now_ms: i64,
    ) {
        if to != ActivityPhase::Waiting {
            self.rechecks.remove(id);
        }
        if to == ActivityPhase::Exited {
            return;
        }
        let at_point_now = env
            .gate_facts(id, now_ms)
            .phase
            .is_some_and(|p| at_safe_stopping_point(&p));
        if !should_check_on_transition(to, at_point_now) {
            return;
        }
        let outcome = check(sender, env, id, now_ms, &mut self.sent);
        if to != ActivityPhase::Waiting {
            return;
        }
        let window = match outcome {
            Outcome::Settled => {
                self.rechecks.remove(id);
                return;
            }
            Outcome::Quiet => QUIET_WINDOW_MS,
            Outcome::Refused => REFUSED_WINDOW_MS,
        };
        self.rechecks.insert(
            id.to_owned(),
            Recheck {
                armed_at: now_ms,
                due: now_ms + RECHECK_INTERVAL_MS,
                until: now_ms + window,
            },
        );
    }

    /// Run every recheck that is due.
    pub fn on_timer(&mut self, sender: &mut Sender, env: &mut dyn DeliveryEnv, now_ms: i64) {
        let due: Vec<String> = self
            .rechecks
            .iter()
            .filter(|(_, r)| r.due <= now_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let waiting = env
                .gate_facts(&id, now_ms)
                .phase
                .is_some_and(|p| p.phase == ActivityPhase::Waiting);
            let Some(mut r) = self.rechecks.remove(&id) else {
                continue;
            };
            if !waiting {
                continue;
            }
            match check(sender, env, &id, now_ms, &mut self.sent) {
                Outcome::Settled => continue,
                Outcome::Quiet => {}
                Outcome::Refused => r.until = r.until.max(r.armed_at + REFUSED_WINDOW_MS),
            }
            r.due = now_ms + RECHECK_INTERVAL_MS;
            if r.due <= r.until {
                self.rechecks.insert(id, r);
            }
        }
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<i64> {
        self.rechecks.values().map(|r| r.due).min()
    }
}

/// One check: read, decide, store or send.
fn check(
    sender: &mut Sender,
    env: &mut dyn DeliveryEnv,
    id: &str,
    now_ms: i64,
    sent: &mut BTreeMap<String, Mark>,
) -> Outcome {
    let settings = env.context_settings();
    let (mark, newest) = match (env.context_mark(id), env.newest_context(id)) {
        (Ok(mark), Ok(newest)) => (mark, newest),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(harness_id = id, error = %e, "context nudge: lookup failed (#689)");
            return Outcome::Quiet;
        }
    };
    match decide(mark, newest, settings.limit) {
        Decision::Nothing => Outcome::Quiet,
        Decision::Store(m) => {
            persist(env, id, m);
            Outcome::Settled
        }
        Decision::Due {
            tokens,
            limit,
            after,
        } => {
            // A crossing that happened while the nudge was off must not
            // fire when it is switched on later.
            if !settings.enabled {
                persist(env, id, after);
                return Outcome::Settled;
            }
            let body = render(settings.body.as_deref(), tokens, limit);
            let opts = SendOpts {
                automatic: true,
                released_by_user: false,
            };
            match sender.begin(env, id, &body, opts, now_ms) {
                SendStart::Pasted => {
                    tracing::info!(harness_id = id, tokens, limit, "context nudge sent (#689)");
                    persist(env, id, after);
                    if let Some(before) = mark {
                        sent.insert(id.to_owned(), before);
                    }
                    Outcome::Settled
                }
                SendStart::Refused(_) => Outcome::Refused,
            }
        }
    }
}

#[cfg(test)]
mod tests;
