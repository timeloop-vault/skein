//! What the prompt gate reads, gathered by the caller so the gate itself
//! stays pure.

use skein_proto::harness_kind::HarnessKind;

use crate::harness_phase::{ActivityPhase, HarnessActivity};

/// The fields of a harness's phase record the gate looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct PhaseFacts {
    pub phase: ActivityPhase,
    pub question_open: bool,
    pub delegation_deferred_at: Option<i64>,
    pub authoritative: bool,
    pub adapter_heard: bool,
    pub adapter_silent: bool,
    pub launch_signal_at: Option<i64>,
    pub injected: bool,
}

impl From<&HarnessActivity> for PhaseFacts {
    fn from(a: &HarnessActivity) -> Self {
        Self {
            phase: a.phase,
            question_open: a.question_open,
            delegation_deferred_at: a.delegation_deferred_at,
            authoritative: a.authoritative,
            adapter_heard: a.adapter_heard,
            adapter_silent: a.adapter_silent,
            launch_signal_at: a.launch_signal_at,
            injected: a.injected,
        }
    }
}

/// Everything `can_send` needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct GateFacts {
    pub kind: HarnessKind,
    /// A live PTY is attached to the harness (TS "registered").
    pub pty_live: bool,
    /// The pane runs its tool, not a post-exit shell.
    pub runs_its_tool: bool,
    /// `None` when the phase service has no record for the harness.
    pub phase: Option<PhaseFacts>,
    /// Epoch ms of the most recent PTY output.
    pub last_output_at: Option<i64>,
    /// The terminal has bracketed paste on.
    pub bracketed_paste: bool,
    pub now_ms: i64,
}
