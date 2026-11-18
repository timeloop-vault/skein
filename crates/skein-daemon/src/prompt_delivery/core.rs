//! The one entry point of prompt delivery (#733): [`DeliveryCore`] owns the
//! [`Sender`] and the [`Mail`] layer, takes every event the delivery worker
//! sees through `handle`, and every deadline through `on_timer` /
//! `next_deadline`. Pure and clock-injected; because everything runs through
//! `&mut self`, deliveries are serialized (TS `runSerialized`) for free.

use super::context_flow::ContextFlow;
use super::env::DeliveryEnv;
use super::gate::{GateMode, Refusal};
use super::mail::Mail;
use super::send::{SendOpts, SendStart, Sender};
use crate::harness_phase::ActivityPhase;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryEvent {
    /// Unread mail changed (a message landed, or the harness read some).
    MailChanged { harness_id: String },
    Transition {
        harness_id: String,
        from: ActivityPhase,
        to: ActivityPhase,
    },
    /// #381: a delegation deferral armed with no phase change.
    DeferralArmed { harness_id: String },
    /// #383: the draft cleared / user input was reported: re-check.
    ClientFactsChanged { harness_id: String },
    /// A new process: reset the harness's delivery state, then check.
    Spawned { harness_id: String },
    /// The harness was removed or exited for good: drop its state.
    Forgotten { harness_id: String },
    /// #413: the user released held mail (skips only the draft guard).
    DeliverNow { harness_id: String },
    /// A review nudge / prompt-library item: not automatic, so no draft
    /// check, no hold, no retry.
    Manual { harness_id: String, body: String },
    /// Read-only: would the gate allow this now? Writes and records nothing.
    Check {
        harness_id: String,
        body: Option<String>,
        mode: GateMode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    None,
    /// Answered at paste time.
    Manual(Result<(), Refusal>),
    DeliverNow(Result<(), Refusal>),
    Check(Result<(), Refusal>),
}

#[derive(Debug, Default)]
pub struct DeliveryCore {
    sender: Sender,
    mail: Mail,
    context: ContextFlow,
}

impl DeliveryCore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, env: &mut dyn DeliveryEnv, ev: DeliveryEvent, now_ms: i64) -> Reply {
        match ev {
            DeliveryEvent::MailChanged { harness_id }
            | DeliveryEvent::DeferralArmed { harness_id } => {
                self.mail
                    .automatic(&mut self.sender, env, &harness_id, now_ms);
                Reply::None
            }
            DeliveryEvent::ClientFactsChanged { harness_id } => {
                self.mail
                    .automatic(&mut self.sender, env, &harness_id, now_ms);
                self.context
                    .on_client_facts(&mut self.sender, env, &harness_id, now_ms);
                Reply::None
            }
            DeliveryEvent::Transition {
                harness_id,
                from,
                to,
            } => {
                self.sender.on_transition(&harness_id, from, to);
                self.mail
                    .on_transition(&mut self.sender, env, &harness_id, from, to, now_ms);
                self.context
                    .on_transition(&mut self.sender, env, &harness_id, to, now_ms);
                Reply::None
            }
            DeliveryEvent::Spawned { harness_id } => {
                self.sender.forget(&harness_id);
                self.mail.reset(env, &harness_id);
                self.mail
                    .automatic(&mut self.sender, env, &harness_id, now_ms);
                self.context.spawned(env, &harness_id);
                Reply::None
            }
            DeliveryEvent::Forgotten { harness_id } => {
                self.sender.forget(&harness_id);
                self.mail.reset(env, &harness_id);
                self.context.forget(&harness_id);
                Reply::None
            }
            DeliveryEvent::DeliverNow { harness_id } => {
                Reply::DeliverNow(
                    self.mail
                        .deliver_now(&mut self.sender, env, &harness_id, now_ms),
                )
            }
            DeliveryEvent::Manual { harness_id, body } => {
                match self
                    .sender
                    .begin(env, &harness_id, &body, SendOpts::default(), now_ms)
                {
                    SendStart::Refused(r) => Reply::Manual(Err(r)),
                    SendStart::Pasted => Reply::Manual(Ok(())),
                }
            }
            DeliveryEvent::Check {
                harness_id,
                body,
                mode,
            } => Reply::Check(self.sender.check(
                env,
                &harness_id,
                body.as_deref().unwrap_or(""),
                mode,
                now_ms,
            )),
        }
    }

    /// Run every deadline due at `now_ms`: the sender's submit stages
    /// first (so a finished send frees its harness), then the mail layer's
    /// settlements and retry ticks.
    pub fn on_timer(&mut self, env: &mut dyn DeliveryEnv, now_ms: i64) {
        for ev in self.sender.on_timer(env, now_ms) {
            self.mail.on_send_event(&ev);
            self.context.on_send_event(env, &ev);
        }
        self.mail.on_timer(&mut self.sender, env, now_ms);
        self.context.on_timer(&mut self.sender, env, now_ms);
    }

    /// The earliest moment `on_timer` has work, over sender, mail and the context nudge.
    #[must_use]
    pub fn next_deadline(&self) -> Option<i64> {
        [
            self.sender.next_deadline(),
            self.mail.next_deadline(),
            self.context.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

#[cfg(test)]
mod tests;
