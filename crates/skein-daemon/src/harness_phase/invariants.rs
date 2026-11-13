//! The phase supervisor's invariants (#423). Port of
//! `app/src/supervisor/invariants.ts` and the data shapes of `mailFeed.ts` /
//! `transcriptStat.ts`. The per-harness state machine that evaluates them
//! (grace, recovery cap, history) and the log formatters are in
//! `supervisor_state.rs` and re-exported from here, so every public name
//! still lives at `invariants::`; the driver that gathers snapshots and
//! applies the repairs is `supervisor.rs` (the TS `runtime.ts`).
//!
//! Everything here is pure: the checks look at a [`SupervisorSnapshot`] and
//! a time, never at a store, timer or the backend.

use super::constants::DELEGATION_SETTLE_MS;
use super::types::{ActivityPhase, AdapterEvent, TurnSignal, TurnSignalKind};

pub(super) use super::supervisor_state::SupervisorEntry;
pub use super::supervisor_state::{
    CodeState, Effect, HISTORY_CAP, HarnessFacts, HarnessSupervisorState, LogLevel, RECOVERY_CAP,
    RECOVERY_MIN_INTERVAL_MS, RECOVERY_WINDOW_MS, SupervisorFacts, SupervisorMemory, Violation,
    ViolationRecord, format_effect, format_recovery_outcome,
};

/// Everything an invariant may look at, gathered by the driver.
#[derive(Debug, Clone)]
pub struct SupervisorSnapshot {
    pub harness_id: String,
    pub phase: ActivityPhase,
    /// ms of the last phase change
    pub phase_since: i64,
    /// last PTY chunk
    pub last_output_at: Option<i64>,
    pub authoritative: bool,
    /// a watchdog degraded the adapter
    pub adapter_silent: bool,
    /// an adapter attach is live for this harness
    pub live_attach: bool,
    pub last_adapter_event: Option<AdapterEvent>,
    pub authority_lost_at: Option<i64>,
    /// main session only
    pub last_turn_signal: Option<TurnSignal>,
    pub subagents_working: usize,
    pub background_working: usize,
    pub delegation_deferred_at: Option<i64>,
    pub delegation_emptied_at: Option<i64>,
    pub permission_at: Option<i64>,
    /// last user Enter / seam submit
    pub last_submit_at: Option<i64>,
    /// None = main-session dialog
    pub permission_agent_id: Option<String>,
    pub mail: Option<MailState>,
    /// stat samples of the transcript, oldest first
    pub transcript: Option<Vec<TranscriptSample>>,
}

/// #423: what `useMailDelivery` knows about a harness's mail
/// (`supervisor/mailFeed.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailState {
    pub unread: u32,
    pub last_refusal: Option<String>,
    /// Last DELIVERED nudge, kept after the settle window clears.
    pub last_nudge_at: Option<i64>,
}

/// One stat of a Claude harness's main transcript (`runtime.ts` `Sample`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranscriptSample {
    /// when the stat was taken, epoch ms
    pub at: i64,
    pub size: i64,
    pub mtime_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Violated { evidence: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupervisorCode {
    AdapterWithoutAuthority,
    EndedTurnNotWaiting,
    MailHeld,
    TailSilentFileGrowing,
    DeferralWithoutWork,
    PermissionOrphaned,
}

impl SupervisorCode {
    /// The exact TS string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdapterWithoutAuthority => "adapter_without_authority",
            Self::EndedTurnNotWaiting => "ended_turn_not_waiting",
            Self::MailHeld => "mail_held",
            Self::TailSilentFileGrowing => "tail_silent_file_growing",
            Self::DeferralWithoutWork => "deferral_without_work",
            Self::PermissionOrphaned => "permission_orphaned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    RegrantAuthority,
    SetWaiting,
}

impl Recovery {
    /// The exact TS string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegrantAuthority => "regrant_authority",
            Self::SetWaiting => "set_waiting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Error,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

pub struct Invariant {
    pub code: SupervisorCode,
    pub grace_ms: i64,
    pub severity: Severity,
    pub recovery: Option<Recovery>,
    pub check: fn(&SupervisorSnapshot, i64) -> Verdict,
}

pub const ADAPTER_WITHOUT_AUTHORITY_GRACE_MS: i64 = 5_000;
pub const ENDED_TURN_GRACE_MS: i64 = 10_000;
/// Output this recent means someone is typing into a non-authoritative harness.
pub const ENDED_TURN_QUIET_MS: i64 = 10_000;
pub const MAIL_HELD_GRACE_MS: i64 = 60_000;
pub const TAIL_SILENT_GRACE_MS: i64 = 0;
/// How long the adapter must have been silent while the file grows.
pub const TAIL_SILENT_MIN_MS: i64 = 60_000;
pub const DEFERRAL_WITHOUT_WORK_GRACE_MS: i64 = 15_000;
pub const PERMISSION_ORPHANED_GRACE_MS: i64 = 0;
/// A main-session dialog is orphaned once the turn moved on this long ago.
pub const PERMISSION_TURN_MOVED_MS: i64 = 10_000;
pub const PERMISSION_CEILING_MS: i64 = 15 * 60_000;

fn fired(evidence: String) -> Verdict {
    Verdict::Violated { evidence }
}

/// `now - at` as the `${…}ms` / `none` rendering the evidence strings use.
fn ago(now: i64, at: Option<i64>) -> String {
    at.map_or_else(|| "none".to_owned(), |t| format!("{}ms", now - t))
}

fn adapter_without_authority(s: &SupervisorSnapshot, now: i64) -> Verdict {
    let Some(e) = s.last_adapter_event else {
        return Verdict::Ok;
    };
    if s.live_attach
        && !s.authoritative
        && !s.adapter_silent
        && s.phase != ActivityPhase::Exited
        && e.restores_authority
        && s.authority_lost_at.is_none_or(|lost| e.at > lost)
    {
        return fired(format!(
            "phase={} authoritative=false lastAdapterEvent={}ms authorityLostAt={}",
            s.phase.as_str(),
            now - e.at,
            ago(now, s.authority_lost_at),
        ));
    }
    Verdict::Ok
}

fn ended_turn_not_waiting(s: &SupervisorSnapshot, now: i64) -> Verdict {
    let Some(t) = s.last_turn_signal else {
        return Verdict::Ok;
    };
    if t.kind == TurnSignalKind::End
        && s.last_submit_at.is_none_or(|sub| t.at > sub)
        && s.subagents_working + s.background_working == 0
        && s.delegation_deferred_at.is_none()
        && (s.phase == ActivityPhase::Running || s.phase == ActivityPhase::Idle)
        && s.last_output_at
            .is_none_or(|out| now - out >= ENDED_TURN_QUIET_MS)
    {
        return fired(format!(
            "phase={} lastTurn=end@{}ms authoritative={} output={}",
            s.phase.as_str(),
            now - t.at,
            s.authoritative,
            ago(now, s.last_output_at),
        ));
    }
    Verdict::Ok
}

fn mail_held(s: &SupervisorSnapshot, now: i64) -> Verdict {
    let Some(m) = &s.mail else {
        return Verdict::Ok;
    };
    if m.unread > 0
        && s.phase == ActivityPhase::Waiting
        && m.last_nudge_at.is_none_or(|n| n < s.phase_since)
    {
        return fired(format!(
            "unread={} waiting={}ms lastRefusal={}",
            m.unread,
            now - s.phase_since,
            m.last_refusal.as_deref().unwrap_or("none"),
        ));
    }
    Verdict::Ok
}

fn tail_silent_file_growing(s: &SupervisorSnapshot, _now: i64) -> Verdict {
    let Some(samples) = &s.transcript else {
        return Verdict::Ok;
    };
    if !s.live_attach || samples.len() < 2 {
        return Verdict::Ok;
    }
    // `Number.NEGATIVE_INFINITY` when the adapter has not spoken.
    let since = s.last_adapter_event.map_or(i64::MIN, |e| e.at);
    let Some(i0) = samples.iter().position(|x| x.at >= since) else {
        return Verdict::Ok;
    };
    let s0 = samples[i0];
    for sn in samples[i0 + 1..].iter().rev() {
        if (sn.size > s0.size || sn.mtime_ms > s0.mtime_ms) && sn.at - s0.at >= TAIL_SILENT_MIN_MS {
            return fired(format!(
                "grewBytes={} silentFor={}ms",
                sn.size - s0.size,
                sn.at - s0.at
            ));
        }
    }
    Verdict::Ok
}

fn deferral_without_work(s: &SupervisorSnapshot, now: i64) -> Verdict {
    let Some(deferred) = s.delegation_deferred_at else {
        return Verdict::Ok;
    };
    if s.subagents_working + s.background_working == 0
        && s.phase != ActivityPhase::Permission
        && s.phase != ActivityPhase::Exited
        && s.delegation_emptied_at
            .is_none_or(|em| now - em >= DELEGATION_SETTLE_MS)
    {
        return fired(format!(
            "phase={} deferred={}ms emptied={}",
            s.phase.as_str(),
            now - deferred,
            ago(now, s.delegation_emptied_at),
        ));
    }
    Verdict::Ok
}

fn permission_orphaned(s: &SupervisorSnapshot, now: i64) -> Verdict {
    let (ActivityPhase::Permission, Some(perm_at)) = (s.phase, s.permission_at) else {
        return Verdict::Ok;
    };
    if let Some(t) = s.last_turn_signal
        && s.permission_agent_id.is_none()
        // subagents only: background tasks open no dialogs
        && s.subagents_working == 0
        && t.at > perm_at
        && now - t.at >= PERMISSION_TURN_MOVED_MS
    {
        return fired(format!(
            "arm=turn_moved_on permission={}ms lastTurn={}@{}ms",
            now - perm_at,
            match t.kind {
                TurnSignalKind::Work => "work",
                TurnSignalKind::End => "end",
            },
            now - t.at,
        ));
    }
    if now - perm_at >= PERMISSION_CEILING_MS {
        return fired(format!(
            "arm=ceiling permission={}ms agent={}",
            now - perm_at,
            s.permission_agent_id.as_deref().unwrap_or("main"),
        ));
    }
    Verdict::Ok
}

/// The six invariants, in evaluation order.
pub const INVARIANTS: [Invariant; 6] = [
    Invariant {
        code: SupervisorCode::AdapterWithoutAuthority,
        grace_ms: ADAPTER_WITHOUT_AUTHORITY_GRACE_MS,
        severity: Severity::Error,
        recovery: Some(Recovery::RegrantAuthority),
        check: adapter_without_authority,
    },
    Invariant {
        code: SupervisorCode::EndedTurnNotWaiting,
        grace_ms: ENDED_TURN_GRACE_MS,
        severity: Severity::Warn,
        recovery: Some(Recovery::SetWaiting),
        check: ended_turn_not_waiting,
    },
    Invariant {
        code: SupervisorCode::MailHeld,
        grace_ms: MAIL_HELD_GRACE_MS,
        severity: Severity::Warn,
        recovery: None,
        check: mail_held,
    },
    Invariant {
        code: SupervisorCode::TailSilentFileGrowing,
        grace_ms: TAIL_SILENT_GRACE_MS,
        severity: Severity::Warn,
        recovery: None,
        check: tail_silent_file_growing,
    },
    Invariant {
        code: SupervisorCode::DeferralWithoutWork,
        grace_ms: DEFERRAL_WITHOUT_WORK_GRACE_MS,
        severity: Severity::Warn,
        recovery: None,
        check: deferral_without_work,
    },
    Invariant {
        code: SupervisorCode::PermissionOrphaned,
        grace_ms: PERMISSION_ORPHANED_GRACE_MS,
        severity: Severity::Warn,
        recovery: None,
        check: permission_orphaned,
    },
];
