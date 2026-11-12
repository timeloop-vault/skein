//! The #86 permission phase. Port of `app/src/harnessActivityPermission.ts`;
//! the shared state lives in `machine.rs`.

// The doc comments are the ported spec and quote TS identifiers verbatim.
#![allow(clippy::doc_markdown)]

use super::machine::PhaseMachine;
use super::types::{ActivityPhase, TransitionSource};

impl PhaseMachine {
    /// L2c adapter reports a permission dialog is on screen: Claude's
    /// PermissionRequest hook fired, or opencode's permission-asked SSE
    /// event landed. `tool_name` is `None` when the adapter can't say
    /// (opencode's event carries no tool name); every notification
    /// surface shows it when present. `agent_type` names the subagent
    /// the dialog belongs to when it isn't the main session (#298);
    /// `None` for the main session or when the adapter can't say.
    /// `agent_id` is that same subagent's id (what `clear_permission`
    /// correlates on), `None` likewise.
    /// No-op once exited. Always goes through `set_phase` even when
    /// already in `permission`, so a second request with a different
    /// tool name still updates `permission_tool` for anything reading
    /// `get` live. #86.
    pub fn set_permission_from_adapter(
        &mut self,
        id: &str,
        source: TransitionSource,
        tool_name: Option<&str>,
        agent_type: Option<&str>,
        agent_id: Option<&str>,
        now_ms: i64,
    ) {
        let Some(cur) = self.get(id) else {
            return;
        };
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        let tool = tool_name.map(str::to_owned);
        let agent_type = agent_type.map(str::to_owned);
        let agent_id = agent_id.map(str::to_owned);
        self.set_phase_with(
            id,
            ActivityPhase::Permission,
            source,
            move |a| {
                a.permission_tool = tool;
                a.permission_agent_type = agent_type;
                a.permission_agent_id = agent_id;
                a.permission_at = Some(now_ms);
            },
            now_ms,
        );
    }

    /// Mirror whether an opencode question prompt is outstanding (#672).
    /// Changes no phase and logs no transition; notifies subscribers
    /// only when the value actually changes. No-op for an unknown id.
    pub fn set_question_open(&mut self, id: &str, open: bool) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if cur.question_open == open {
            return;
        }
        cur.question_open = open;
        self.emit(id);
    }

    /// Leave `permission` for `running`, and touch no other phase. For
    /// an adapter detaching mid-dialog: nothing it would have sent can
    /// arrive now, and the L2a tick never moves a non-running phase, so
    /// without this the harness would read "permission needed" until
    /// the user typed or the PTY died. `running` hands it back to L2a.
    pub fn release_permission(&mut self, id: &str, source: TransitionSource, now_ms: i64) {
        if self.get(id).map(|a| a.phase) != Some(ActivityPhase::Permission) {
            return;
        }
        self.set_phase(id, ActivityPhase::Running, source, now_ms);
    }

    /// A subagent's tool result landed (#298). Proves *that subagent's*
    /// gate is gone, and only that one: with several subagents running
    /// concurrently, an unrelated subagent finishing its own tool call
    /// must not clear a different subagent's still-open dialog. No-op
    /// unless the harness is actually in `permission`. When it is:
    /// `agent_id` is compared against the stored `permission_agent_id`:
    /// a match clears; a mismatch (a *different* subagent's result) is
    /// left alone. A stored `None` still clears unconditionally, for
    /// two folded-together cases that read the same way from here: a
    /// main-session dialog (there is no subagent to disambiguate
    /// against), and an adapter event where `agent_id` wasn't reported
    /// at all (the field is confirmed live as of 2026-09-20, so that
    /// second case is now the rare one: injection off, or a future CLI
    /// change), but falling back to the pre-#298 behaviour is still the
    /// safe direction; a dialog cleared slightly early is recoverable,
    /// one left stuck for minutes is the bug this exists to fix. Unlike
    /// `release_permission` (adapter vanished entirely) this fires on a
    /// live, healthy adapter mid-conversation, so it must do the one
    /// narrow thing it's proof of and nothing more: never touch
    /// `waiting`, `idle`, `spawning` or `exited`. A subagent tool
    /// result is not "the harness is now doing work" (it might be the
    /// main session sitting at `waiting` while a background subagent
    /// finishes up): it is only "whatever dialog was on screen has
    /// been answered." All further phase/notification policy belongs
    /// to #277.
    pub fn clear_permission(
        &mut self,
        id: &str,
        source: TransitionSource,
        agent_id: Option<&str>,
        now_ms: i64,
    ) {
        let Some(cur) = self.get(id) else {
            return;
        };
        if cur.phase != ActivityPhase::Permission {
            return;
        }
        if cur.permission_agent_id.is_some() && cur.permission_agent_id.as_deref() != agent_id {
            return;
        }
        self.set_phase(id, ActivityPhase::Running, source, now_ms);
    }
}
