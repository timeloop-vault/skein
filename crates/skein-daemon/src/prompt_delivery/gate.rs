//! The safety gate for sending a prompt into a harness's PTY, and the
//! looser one for inserting text without submitting it. This is the only
//! gate: the webview's `canSendPrompt` / `canInsertText` are gone and ask
//! the daemon (`harness_gate_check`). It began as a port of them (with
//! `launchSettled`), plus one check the webview never had: a post-exit
//! shell gets no prompt. Refusal strings are the old TS ones, verbatim.

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

/// The two checks a send and an insert share: a confirmed adapter is
/// watching, and the review tools were injected at spawn.
fn watched_and_injected(p: &PhaseFacts) -> Result<(), Refusal> {
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
    Ok(())
}

/// Which gate a read-only check asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateMode {
    /// Paste and submit ([`can_send`]).
    Send,
    /// Paste without submitting, e.g. a dropped file path ([`can_insert`]).
    Insert,
}

/// Whether text may be pasted WITHOUT submitting it now (a file drop).
/// Looser than [`can_send`]: no settling, question or paste-mode checks,
/// and the phase may be `running`/`idle`/`waiting`.
///
/// # Errors
/// The first failing check.
pub fn can_insert(facts: &GateFacts) -> Result<(), Refusal> {
    if !facts.kind.has_pty() {
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
    match p.phase {
        ActivityPhase::Permission => {
            return refuse(
                RefusalCode::Permission,
                "the harness is waiting on a permission dialog \u{2014} a dropped path could answer it",
            );
        }
        ActivityPhase::Spawning | ActivityPhase::Exited => {
            return refuse(
                RefusalCode::NotReady,
                format!(
                    "the harness isn't ready to receive input (currently {})",
                    p.phase.as_str()
                ),
            );
        }
        _ => {}
    }
    watched_and_injected(p)
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
    watched_and_injected(p)?;
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
