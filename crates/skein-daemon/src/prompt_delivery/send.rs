//! The send flow of `sendPrompt` (`app/src/harnessInputSend.ts`) as a
//! pure, clock-injected state machine: paste now, Enter after the #380
//! gap, then (Claude only) Enter retries. No threads and no clock of its
//! own: the driver calls `on_timer` at `next_deadline`.

use std::collections::BTreeMap;

use skein_proto::harness_kind::HarnessKind;

use super::env::{DeliveryEnv, DraftState};
use super::gate::{GateMode, Refusal, RefusalCode, at_safe_stopping_point, can_insert, can_send};
use super::paste::{SUBMIT, paste_bytes};
use super::submit_retry::{
    RetryDecision, RetryInput, SUBMIT_GAP_MS, SUBMIT_RETRY_SCHEDULE_MS, decide_submit_retry,
};
use crate::harness_phase::ActivityPhase;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendOpts {
    /// An automatic sender (mail): the draft check applies.
    pub automatic: bool,
    /// The user released the delivery (#413): skips only the draft check.
    pub released_by_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendStart {
    Refused(Refusal),
    Pasted,
}

/// What a due timer did, in order. Every send that pasted ends with
/// exactly one `Finished`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendEvent {
    Submitted {
        harness_id: String,
    },
    /// The submit was skipped; the pasted text is left unsent.
    SubmitSkipped {
        harness_id: String,
        why: String,
    },
    /// Retry `attempt` (1-based) wrote another Enter.
    RetryEnter {
        harness_id: String,
        attempt: usize,
    },
    /// The retry sequence ended early.
    RetryStopped {
        harness_id: String,
        reason: String,
    },
    WriteFailed {
        harness_id: String,
    },
    Finished {
        harness_id: String,
    },
}

#[derive(Debug)]
enum Stage {
    /// Waiting out the paste-to-submit gap.
    Gap { due: i64 },
    /// Submitted; the next retry is `attempt` (0-based).
    Retry {
        submit_at: i64,
        attempt: usize,
        due: i64,
    },
}

#[derive(Debug)]
struct InFlight {
    kind: HarnessKind,
    token: String,
    input_seq: u64,
    deferred_at: Option<i64>,
    left_stopping_point: bool,
    stage: Stage,
}

impl InFlight {
    fn due(&self) -> i64 {
        match self.stage {
            Stage::Gap { due } | Stage::Retry { due, .. } => due,
        }
    }
}

#[derive(Debug, Default)]
pub struct Sender {
    flights: BTreeMap<String, InFlight>,
}

fn refused(code: RefusalCode, reason: impl Into<String>) -> SendStart {
    SendStart::Refused(Refusal {
        code,
        reason: reason.into(),
    })
}

impl Sender {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What [`begin`](Self::begin) (manual) or the insert gate would answer
    /// now, without pasting, arming or recording anything.
    pub fn check(
        &self,
        env: &dyn DeliveryEnv,
        harness_id: &str,
        body: &str,
        mode: GateMode,
        now_ms: i64,
    ) -> Result<(), Refusal> {
        match mode {
            GateMode::Send => {
                if self.flights.contains_key(harness_id) {
                    return Err(Refusal {
                        code: RefusalCode::Busy,
                        reason: "a prompt is already being delivered".to_owned(),
                    });
                }
                can_send(&env.gate_facts(harness_id, now_ms), body)
            }
            GateMode::Insert => can_insert(&env.gate_facts(harness_id, now_ms)),
        }
    }

    /// Re-check the gate at call time, paste, and arm the submit.
    pub fn begin(
        &mut self,
        env: &mut dyn DeliveryEnv,
        harness_id: &str,
        body: &str,
        opts: SendOpts,
        now_ms: i64,
    ) -> SendStart {
        if self.flights.contains_key(harness_id) {
            return refused(RefusalCode::Busy, "a prompt is already being delivered");
        }
        let facts = env.gate_facts(harness_id, now_ms);
        if let Err(r) = can_send(&facts, body) {
            tracing::info!(harness_id, reason = %r.reason, "sendPrompt refused (#404)");
            return SendStart::Refused(r);
        }
        if opts.automatic
            && !opts.released_by_user
            && let DraftState::Held { reason } = env.draft(harness_id)
        {
            tracing::info!(harness_id, %reason, "automatic send refused (#404)");
            return refused(RefusalCode::DraftHeld, reason);
        }
        let Some(token) = env.spawn_token(harness_id) else {
            return refused(
                RefusalCode::NotReady,
                "this harness's terminal isn't ready yet",
            );
        };
        if !env.write_pty(harness_id, &paste_bytes(body, facts.bracketed_paste)) {
            return refused(
                RefusalCode::NotReady,
                "writing to this harness's terminal failed",
            );
        }
        tracing::info!(
            harness_id,
            automatic = opts.automatic,
            "sendPrompt: pasted (#404)"
        );
        self.flights.insert(
            harness_id.to_string(),
            InFlight {
                kind: facts.kind,
                token,
                input_seq: env.user_input_seq(harness_id),
                deferred_at: facts.phase.and_then(|p| p.delegation_deferred_at),
                left_stopping_point: false,
                stage: Stage::Gap {
                    due: now_ms + SUBMIT_GAP_MS,
                },
            },
        );
        SendStart::Pasted
    }

    /// A phase transition. Leaving `waiting` after the submit proves the
    /// first Enter landed (only counted once the submit is written).
    pub fn on_transition(&mut self, harness_id: &str, from: ActivityPhase, _to: ActivityPhase) {
        if from != ActivityPhase::Waiting {
            return;
        }
        if let Some(f) = self.flights.get_mut(harness_id)
            && matches!(f.stage, Stage::Retry { .. })
        {
            f.left_stopping_point = true;
        }
    }

    /// Run every deadline that is due at `now_ms`.
    pub fn on_timer(&mut self, env: &mut dyn DeliveryEnv, now_ms: i64) -> Vec<SendEvent> {
        let mut events = Vec::new();
        let ids: Vec<String> = self
            .flights
            .iter()
            .filter(|(_, f)| f.due() <= now_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            while let Some(f) = self.flights.get_mut(&id) {
                if f.due() > now_ms {
                    break;
                }
                if step(f, env, &id, now_ms, &mut events) {
                    self.flights.remove(&id);
                    events.push(SendEvent::Finished {
                        harness_id: id.clone(),
                    });
                }
            }
        }
        events
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<i64> {
        self.flights.values().map(InFlight::due).min()
    }

    /// Respawn or removal: drop the in-flight state and write nothing.
    pub fn forget(&mut self, harness_id: &str) {
        self.flights.remove(harness_id);
    }

    #[must_use]
    pub fn in_flight(&self, harness_id: &str) -> bool {
        self.flights.contains_key(harness_id)
    }
}

/// Why the gap-time recheck refuses to submit, if it does (the TS
/// target / user-input / stopping-point checks, in that order).
fn gap_skip_reason(
    f: &InFlight,
    id: &str,
    same_target: bool,
    input_moved: bool,
    phase: Option<super::facts::PhaseFacts>,
) -> Option<String> {
    if !same_target {
        return Some(format!(
            "harness {id}'s terminal changed before the submit — leaving the paste unsent (#380)"
        ));
    }
    if input_moved {
        return Some(format!(
            "user typed into harness {id} before the submit — leaving the paste unsent (#380)"
        ));
    }
    match phase {
        Some(p) if at_safe_stopping_point(&p) && p.delegation_deferred_at == f.deferred_at => None,
        Some(p) if p.phase == ActivityPhase::Permission => {
            Some("opened a permission dialog before the submit".to_string())
        }
        Some(p) if at_safe_stopping_point(&p) => Some(
            "moved to a different delegation deferral than the one it was pasted under before the submit"
                .to_string(),
        ),
        Some(p) => Some(format!("moved to {} before the submit", p.phase.as_str())),
        None => Some("moved to no activity record before the submit".to_string()),
    }
}

/// Run the due stage of one send; true when the send is over.
fn step(
    f: &mut InFlight,
    env: &mut dyn DeliveryEnv,
    id: &str,
    now_ms: i64,
    events: &mut Vec<SendEvent>,
) -> bool {
    let harness_id = id.to_string();
    let same_target = env.spawn_token(id).as_deref() == Some(f.token.as_str());
    let input_moved = env.user_input_seq(id) != f.input_seq;
    let facts = env.gate_facts(id, now_ms);
    let phase = facts.phase;
    // A post-exit shell or a dead PTY never gets an Enter.
    let no_tool = if !facts.pty_live {
        Some(format!("harness {id}'s terminal is gone"))
    } else if !facts.runs_its_tool {
        Some(format!("harness {id} is in a post-exit shell"))
    } else {
        None
    };
    match f.stage {
        Stage::Gap { .. } => {
            let why = no_tool.or_else(|| gap_skip_reason(f, id, same_target, input_moved, phase));
            if let Some(why) = why {
                tracing::warn!(harness_id = id, %why, "sendPrompt: submit skipped (#380, #381)");
                events.push(SendEvent::SubmitSkipped { harness_id, why });
                return true;
            }
            if !env.write_pty(id, SUBMIT) {
                events.push(SendEvent::WriteFailed { harness_id });
                return true;
            }
            env.note_prompt_submitted(id);
            events.push(SendEvent::Submitted { harness_id });
            if !f.kind.submit_retry() {
                return true;
            }
            f.stage = Stage::Retry {
                submit_at: now_ms,
                attempt: 0,
                due: now_ms + SUBMIT_RETRY_SCHEDULE_MS[0],
            };
            false
        }
        Stage::Retry {
            submit_at, attempt, ..
        } => {
            if let Some(reason) = no_tool {
                events.push(SendEvent::RetryStopped { harness_id, reason });
                return true;
            }
            let decision = decide_submit_retry(&RetryInput {
                capable: true,
                watched: phase.is_some_and(|p| p.authoritative && !p.adapter_silent),
                left_stopping_point_since_send: f.left_stopping_point,
                phase: phase.map(|p| p.phase),
                deferred_at_send: f.deferred_at,
                deferred_at_now: phase.and_then(|p| p.delegation_deferred_at),
                user_input_since_send: input_moved,
                same_target,
            });
            match decision {
                RetryDecision::Stop(reason) => {
                    events.push(SendEvent::RetryStopped { harness_id, reason });
                    true
                }
                RetryDecision::Retry => {
                    if !env.write_pty(id, SUBMIT) {
                        events.push(SendEvent::WriteFailed { harness_id });
                        return true;
                    }
                    events.push(SendEvent::RetryEnter {
                        harness_id,
                        attempt: attempt + 1,
                    });
                    if let Some(delay) = SUBMIT_RETRY_SCHEDULE_MS.get(attempt + 1) {
                        f.stage = Stage::Retry {
                            submit_at,
                            attempt: attempt + 1,
                            due: submit_at + delay,
                        };
                        false
                    } else {
                        true
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
