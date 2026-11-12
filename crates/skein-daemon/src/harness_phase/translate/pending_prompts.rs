//! Pure tracking of opencode's outstanding permission and question
//! prompts (#86), ported from `app/src/pendingPrompts.ts`. opencode's SSE
//! stream tells us when a prompt is asked and when it's answered, but not
//! which activity phase that implies on its own: a harness can have more
//! than one prompt in flight at once (a subagent child session's prompt
//! blocks the user too), so the phase has to be derived from set
//! membership rather than the latest event alone.
//!
//! Permission always outranks a question: a tool waiting on approval is a
//! harder stop than a question the agent asked, so as long as any
//! permission is outstanding the harness reads as `permission` even if a
//! question also landed in the meantime.
//!
//! One instance per harness (the TS keeps a module-level map keyed by
//! harness id; here the owner is the per-harness translator).

use std::collections::HashSet;

/// What phase should follow a pending-set mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingPhase {
    Permission,
    Waiting,
    Running,
}

#[derive(Debug, Default)]
pub struct PendingPrompts {
    permissions: HashSet<String>,
    questions: HashSet<String>,
}

impl PendingPrompts {
    /// `permission` while any is outstanding, else `waiting` while a
    /// question is, else `running` (everything drained, resume work).
    fn resolve(&self) -> PendingPhase {
        if !self.permissions.is_empty() {
            PendingPhase::Permission
        } else if !self.questions.is_empty() {
            PendingPhase::Waiting
        } else {
            PendingPhase::Running
        }
    }

    /// A permission dialog appeared. Always yields `permission`.
    pub fn permission_asked(&mut self, request_id: &str) -> PendingPhase {
        self.permissions.insert(request_id.to_owned());
        PendingPhase::Permission
    }

    /// The permission dialog was answered. The SSE event doesn't say
    /// approved vs. denied, and it doesn't matter here: either way the
    /// gate this one request held is gone.
    pub fn permission_replied(&mut self, request_id: &str) -> PendingPhase {
        self.permissions.remove(request_id);
        self.resolve()
    }

    /// A non-permission question was asked (opencode's own prompt
    /// primitive, distinct from a tool permission). Outranked by any
    /// permission already pending.
    pub fn question_asked(&mut self, request_id: &str) -> PendingPhase {
        self.questions.insert(request_id.to_owned());
        self.resolve()
    }

    pub fn question_resolved(&mut self, request_id: &str) -> PendingPhase {
        self.questions.remove(request_id);
        self.resolve()
    }

    /// Is any question prompt outstanding (#672)?
    pub fn has_question(&self) -> bool {
        !self.questions.is_empty()
    }

    /// Drop every pending id: `session_idle` (opencode finished its turn,
    /// whatever was in flight is moot), `session_end` / `connected` (a
    /// reconnect replays nothing) and adapter detach.
    pub fn forget(&mut self) {
        self.permissions.clear();
        self.questions.clear();
    }
}
