//! The per-harness supervisor state machine and its log formatters (#423).
//! Port of `app/src/supervisor/supervisorState.ts` (`evaluate`, the recovery
//! rate limit and cap, `formatEffect`) plus the shapes the driver in
//! `supervisor.rs` keeps (`SupervisorFacts`, `SupervisorMemory`). The six
//! invariants it evaluates are in `invariants.rs`.

use std::collections::HashMap;

use super::invariants::{
    INVARIANTS, Invariant, MailState, Recovery, Severity, SupervisorCode, SupervisorSnapshot,
    TranscriptSample, Verdict,
};

pub const RECOVERY_MIN_INTERVAL_MS: i64 = 30_000;
pub const RECOVERY_CAP: usize = 3;
pub const HISTORY_CAP: usize = 50;
/// Attempts older than this stop counting toward the cap.
pub const RECOVERY_WINDOW_MS: i64 = 60 * 60_000;

#[derive(Debug, Clone, Default)]
pub struct CodeState {
    pub first_seen_at: Option<i64>,
    /// reported as violated
    pub active: bool,
    /// attempt timestamps within `RECOVERY_WINDOW_MS`; never reset by a
    /// clear, so flapping still hits the cap
    pub attempts: Vec<i64>,
    pub exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationRecord {
    pub code: SupervisorCode,
    pub severity: Severity,
    pub since: i64,
    pub evidence: String,
    pub cleared_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct HarnessSupervisorState {
    pub codes: HashMap<SupervisorCode, CodeState>,
    pub history: Vec<ViolationRecord>,
}

/// What the driver should carry out after one evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Violation {
        code: SupervisorCode,
        severity: Severity,
        evidence: String,
    },
    Cleared {
        code: SupervisorCode,
        after_ms: i64,
    },
    Recover {
        code: SupervisorCode,
        recovery: Recovery,
    },
    RecoveryExhausted {
        code: SupervisorCode,
        attempts: usize,
    },
}

impl Effect {
    #[must_use]
    pub const fn code(&self) -> SupervisorCode {
        match self {
            Self::Violation { code, .. }
            | Self::Cleared { code, .. }
            | Self::Recover { code, .. }
            | Self::RecoveryExhausted { code, .. } => *code,
        }
    }
}

impl HarnessSupervisorState {
    /// Advance this harness's supervisor state by one snapshot and return the
    /// effects the driver should carry out (`evaluate`).
    pub fn evaluate(&mut self, snapshot: &SupervisorSnapshot, now: i64) -> Vec<Effect> {
        self.evaluate_with(&INVARIANTS, snapshot, now)
    }

    /// `evaluate` over an explicit invariant list: the TS 4th parameter, which
    /// its tests use to drive the state machine with a switchable invariant.
    pub fn evaluate_with(
        &mut self,
        invariants: &[Invariant],
        snapshot: &SupervisorSnapshot,
        now: i64,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        for inv in invariants {
            let cs = self.codes.entry(inv.code).or_default();
            cs.attempts.retain(|&t| now - t < RECOVERY_WINDOW_MS);
            if cs.attempts.len() < RECOVERY_CAP {
                cs.exhausted = false;
            }
            let verdict = (inv.check)(snapshot, now);

            let Verdict::Violated { evidence } = verdict else {
                if cs.active {
                    let since = cs.first_seen_at.unwrap_or(now);
                    effects.push(Effect::Cleared {
                        code: inv.code,
                        after_ms: now - since,
                    });
                    if let Some(open) = self
                        .history
                        .iter_mut()
                        .rev()
                        .find(|h| h.code == inv.code && h.cleared_at.is_none())
                    {
                        open.cleared_at = Some(now);
                    }
                }
                cs.first_seen_at = None;
                cs.active = false;
                continue;
            };

            let first_seen = *cs.first_seen_at.get_or_insert(now);
            if !cs.active {
                if now - first_seen < inv.grace_ms {
                    continue;
                }
                cs.active = true;
                effects.push(Effect::Violation {
                    code: inv.code,
                    severity: inv.severity,
                    evidence: evidence.clone(),
                });
                self.history.push(ViolationRecord {
                    code: inv.code,
                    severity: inv.severity,
                    since: first_seen,
                    evidence,
                    cleared_at: None,
                });
                if self.history.len() > HISTORY_CAP {
                    let excess = self.history.len() - HISTORY_CAP;
                    self.history.drain(..excess);
                }
            }

            if let Some(recovery) = inv.recovery
                && !cs.exhausted
            {
                let due = cs
                    .attempts
                    .last()
                    .is_none_or(|&last| now - last >= RECOVERY_MIN_INTERVAL_MS);
                if due {
                    if cs.attempts.len() >= RECOVERY_CAP {
                        cs.exhausted = true;
                        effects.push(Effect::RecoveryExhausted {
                            code: inv.code,
                            attempts: cs.attempts.len(),
                        });
                    } else {
                        cs.attempts.push(now);
                        effects.push(Effect::Recover {
                            code: inv.code,
                            recovery,
                        });
                    }
                }
            }
        }
        effects
    }
}

/// Log level of a line the TS runtime would write with `logBoth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// `formatEffect`: the log line for an effect.
#[must_use]
pub fn format_effect(harness_id: &str, effect: &Effect) -> (LogLevel, String) {
    let head = format!(
        "[skein] supervisor harness={harness_id} code={}",
        effect.code().as_str()
    );
    match effect {
        Effect::Violation {
            severity, evidence, ..
        } => (
            match severity {
                Severity::Warn => LogLevel::Warn,
                Severity::Error => LogLevel::Error,
            },
            format!("{head} violation evidence={evidence}"),
        ),
        Effect::Cleared { after_ms, .. } => {
            (LogLevel::Info, format!("{head} cleared afterMs={after_ms}"))
        }
        Effect::Recover { recovery, .. } => (
            LogLevel::Info,
            format!("{head} recover recovery={}", recovery.as_str()),
        ),
        Effect::RecoveryExhausted { attempts, .. } => (
            LogLevel::Error,
            format!("{head} recovery_exhausted attempts={attempts}"),
        ),
    }
}

/// `formatRecoveryOutcome`.
#[must_use]
pub fn format_recovery_outcome(
    harness_id: &str,
    code: SupervisorCode,
    recovery: Recovery,
    applied: bool,
) -> (LogLevel, String) {
    (
        if applied {
            LogLevel::Info
        } else {
            LogLevel::Warn
        },
        format!(
            "[skein] supervisor harness={harness_id} code={} recovery_outcome recovery={} applied={applied}",
            code.as_str(),
            recovery.as_str()
        ),
    )
}

/// What the driver reports for one line the TS runtime would have logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub harness_id: String,
    pub code: SupervisorCode,
    /// The state-machine effect, or `None` for a `recovery_outcome` line.
    pub effect: Option<Effect>,
    /// `Some(applied)` on a `recovery_outcome` line.
    pub recovery_applied: Option<bool>,
    pub level: LogLevel,
    pub message: String,
}

/// What the caller supplies per harness: facts the machine cannot know.
/// Every field is optional and absent makes the invariants that need it not
/// fire.
#[derive(Debug, Clone, Default)]
pub struct HarnessFacts {
    /// An adapter attach is live (`liveAttach(id) !== null`, harnessEvents.ts).
    /// Absent reads as no attach: `adapter_without_authority` and
    /// `tail_silent_file_growing` cannot fire.
    pub live_attach: Option<bool>,
    /// `mailState(id)` (`mailFeed.ts`); absent = `mail_held` cannot fire.
    pub mail: Option<MailState>,
    /// Transcript stat samples, oldest first, only for a live Claude attach
    /// (`runtime.ts`: sampled every `TRANSCRIPT_SAMPLE_MS`, capped at 8);
    /// absent = `tail_silent_file_growing` cannot fire.
    pub transcript: Option<Vec<TranscriptSample>>,
}

/// Facts for one supervise pass, keyed by harness id. A harness with no entry
/// has all facts absent.
#[derive(Debug, Clone, Default)]
pub struct SupervisorFacts {
    pub harnesses: HashMap<String, HarnessFacts>,
}

/// The runtime's per-harness memory (`entries` in `runtime.ts`, minus the
/// transcript sampler, which the caller owns).
#[derive(Debug, Default)]
pub struct SupervisorMemory {
    pub(super) entries: HashMap<String, SupervisorEntry>,
}

#[derive(Debug)]
pub(super) struct SupervisorEntry {
    pub(super) spawned_at: i64,
    pub(super) state: HarnessSupervisorState,
}

impl SupervisorMemory {
    /// Violation history for one harness (newest last), `supervisorViolations`.
    #[must_use]
    pub fn violations(&self, harness_id: &str) -> &[ViolationRecord] {
        self.entries
            .get(harness_id)
            .map_or(&[], |e| e.state.history.as_slice())
    }
}
