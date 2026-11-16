//! The safety gate for sending a prompt into a harness's PTY: a port of
//! `canSendPrompt` in `app/src/harnessInputGate.ts` (with `launchSettled`
//! from `launchReady.ts`), plus one check the webview never needed: a
//! post-exit shell gets no prompt. Refusal strings are the TS ones,
//! verbatim.

use skein_proto::harness_kind::HarnessKind;

use super::facts::{GateFacts, PhaseFacts};
use crate::harness_phase::ActivityPhase;

/// How long the PTY must stay quiet after the launch signal (`LAUNCH_QUIET_MS`).
pub const LAUNCH_QUIET_MS: i64 = 1500;
/// Upper bound on waiting for quiet (`LAUNCH_READY_CAP_MS`).
pub const LAUNCH_READY_CAP_MS: i64 = 15_000;

/// Why a prompt was refused, for callers that branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefusalCode {
    NoTerminal,
    NotReady,
    RunsShell,
    Permission,
    QuestionOpen,
    NotAtStoppingPoint,
    NotWatched,
    NotInjected,
    Settling,
    NeedsPasteMode,
    /// An automatic send met a composer draft the user hasn't released.
    DraftHeld,
    /// A prompt is already being delivered to this harness (or, for mail,
    /// a previous nudge is still settling).
    Busy,
    /// A "deliver now" with no undelivered mail to deliver.
    NothingToDeliver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub reason: String,
}

fn refuse(code: RefusalCode, reason: impl Into<String>) -> Result<(), Refusal> {
    Err(Refusal {
        code,
        reason: reason.into(),
    })
}

/// Phase `waiting`, or `running` with an armed #277 delegation deferral
/// (the main session already ended its turn). Never `permission`.
#[must_use]
pub fn at_safe_stopping_point(p: &PhaseFacts) -> bool {
    p.phase == ActivityPhase::Waiting
        || (p.phase == ActivityPhase::Running && p.delegation_deferred_at.is_some())
}

/// `launchSettled`: true when nothing about the launch should hold a paste.
#[must_use]
fn launch_settled(p: &PhaseFacts, last_output_at: Option<i64>, now_ms: i64) -> bool {
    if p.adapter_heard {
        return true;
    }
    let Some(signal) = p.launch_signal_at else {
        return true;
    };
    let quiet_since = signal.max(last_output_at.unwrap_or(signal));
    now_ms - quiet_since >= LAUNCH_QUIET_MS || now_ms - signal >= LAUNCH_READY_CAP_MS
}

/// Whether `body` may be pasted and submitted now.
///
/// # Errors
/// The first failing check, in the TS order.
pub fn can_send(facts: &GateFacts, body: &str) -> Result<(), Refusal> {
    let kind: HarnessKind = facts.kind;
    if !kind.has_pty() {
        return refuse(
            RefusalCode::NoTerminal,
            "this harness has no terminal to paste into",
        );
    }
    let Some(p) = facts.phase.as_ref().filter(|_| facts.pty_live) else {
        return refuse(
            RefusalCode::NotReady,
            "this harness's terminal isn't ready yet",
        );
    };
    if !facts.runs_its_tool {
        return refuse(
            RefusalCode::RunsShell,
            "the pane is running a shell, not its tool",
        );
    }
    if p.phase == ActivityPhase::Permission {
        return refuse(
            RefusalCode::Permission,
            "the harness is waiting on a permission dialog",
        );
    }
    if p.question_open {
        return refuse(
            RefusalCode::QuestionOpen,
            "the harness is waiting on an answer to its own question",
        );
    }
    if !at_safe_stopping_point(p) {
        return refuse(
            RefusalCode::NotAtStoppingPoint,
            format!(
                "the harness isn't at a safe stopping point (currently {})",
                p.phase.as_str()
            ),
        );
    }
    if !p.authoritative || (!p.adapter_heard && p.launch_signal_at.is_none()) || p.adapter_silent {
        return refuse(
            RefusalCode::NotWatched,
            "no confirmed adapter is watching this harness",
        );
    }
    if !p.injected {
        return refuse(
            RefusalCode::NotInjected,
            "this harness wasn't started with the review tools wired up",
        );
    }
    if !launch_settled(p, facts.last_output_at, facts.now_ms) {
        return refuse(
            RefusalCode::Settling,
            "the harness is still settling after launch (#404)",
        );
    }
    if body.contains('\n') && !facts.bracketed_paste {
        return refuse(
            RefusalCode::NeedsPasteMode,
            "this harness's terminal isn't in paste mode, so a multi-line prompt isn't safe to send",
        );
    }
    if kind.bracketed_paste_required() && !facts.bracketed_paste {
        return refuse(
            RefusalCode::NeedsPasteMode,
            "this harness's terminal isn't in paste mode, so a prompt isn't safe to send",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
