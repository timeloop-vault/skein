//! A tap on what the phase service publishes (#733): one registered
//! listener hears, per published update, the harness's transitions and
//! deferral arming, plus spawns and forgets. It exists for prompt
//! delivery; it changes no phase decision and no wire type.
//!
//! The listener runs under the service lock, in publish order. It must
//! only hand the notice on (a channel send) and never call back into the
//! service.

use std::sync::Arc;

use super::types::{ActivityPhase, Transition};

/// What the listener is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseNotice {
    /// A new process: the record was replaced.
    Spawned { harness_id: String },
    /// One phase change, oldest first within an update.
    Transition {
        harness_id: String,
        from: ActivityPhase,
        to: ActivityPhase,
    },
    /// A delegation deferral armed in this update (#381).
    DeferralArmed { harness_id: String },
    /// The harness's record is gone.
    Forgotten { harness_id: String },
}

impl PhaseNotice {
    pub(super) fn transition(t: &Transition) -> Self {
        Self::Transition {
            harness_id: t.harness_id.clone(),
            from: t.from,
            to: t.to,
        }
    }
}

/// See the module docs.
pub type PhaseListener = Arc<dyn Fn(PhaseNotice) + Send + Sync>;
