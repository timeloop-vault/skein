//! Parser for Hermes shell-hook payloads (issue #625).
//!
//! Hermes runs a configured shell command per lifecycle event and pipes
//! one JSON object to its stdin: top-level `hook_event_name`,
//! `tool_name`, `tool_input`, `session_id`, `cwd`, `profile` and an
//! `extra` object carrying everything event-specific, including
//! `telemetry_schema_version` (`hermes.observer.v1`). The shape belongs
//! to Hermes, not Skein, so nothing here is required: every field is
//! optional, a wrong-typed field reads as absent and unknown keys are
//! ignored. Verified against payloads recorded on v0.21.7+184 (the
//! fixtures under `fixtures/`).
//!
//! Facts the fixtures pin down:
//!
//! - `turn_id` lives in `extra`, never top-level, and is absent on
//!   `on_session_start` and on `subagent_*` (which carry
//!   `extra.parent_turn_id` instead).
//! - `on_session_end` fires once per TURN, not per session; the real
//!   end of a session is `on_session_finalize` (parsed as
//!   [`HermesHookEvent::Other`]).
//! - A subagent's own events carry the CHILD `session_id` top-level and
//!   `extra.platform == "subagent"`; `subagent_start`/`subagent_stop`
//!   carry the PARENT `session_id` and `extra.child_session_id`.
//! - Every approval yields two `pre_approval_request`s: first
//!   `surface: "smart"` (Hermes' own LLM pre-check, never followed by a
//!   post), then the real dialog, paired with `post_approval_response`.

use serde_json::Value;

/// The lifecycle event named by `hook_event_name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HermesHookEvent {
    /// `on_session_start`.
    SessionStart,
    /// `pre_llm_call`: a turn begins.
    PreLlmCall,
    /// `pre_tool_call`.
    PreToolCall,
    /// `on_session_end`: the end of one TURN, not of the session.
    SessionEnd,
    /// `pre_approval_request`.
    ApprovalRequest,
    /// `post_approval_response`: the dialog was answered.
    ApprovalResponse,
    /// `subagent_start`.
    SubagentStart,
    /// `subagent_stop`.
    SubagentStop,
    /// Any other name (`post_llm_call`, `post_tool_call`,
    /// `on_session_finalize`, events from a newer Hermes).
    Other(String),
}

impl HermesHookEvent {
    fn from_name(name: &str) -> Self {
        match name {
            "on_session_start" => Self::SessionStart,
            "pre_llm_call" => Self::PreLlmCall,
            "pre_tool_call" => Self::PreToolCall,
            "on_session_end" => Self::SessionEnd,
            "pre_approval_request" => Self::ApprovalRequest,
            "post_approval_response" => Self::ApprovalResponse,
            "subagent_start" => Self::SubagentStart,
            "subagent_stop" => Self::SubagentStop,
            other => Self::Other(other.to_string()),
        }
    }
}

/// Identity that pairs a `post_approval_response` with the request it
/// answers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ApprovalKey {
    /// The tool call the approval gates.
    ToolCall(String),
    /// Fallback: `(turn_id, description)`.
    TurnDescription(String, String),
}

/// One parsed hook payload. See the module docs for which fields exist
/// on which events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesHook {
    pub event: HermesHookEvent,
    /// Top-level `session_id`.
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    /// `extra.platform`: `cli`, `subagent`, ...
    pub platform: Option<String>,
    /// `extra.surface` on approval events: `smart`, `cli`, `gateway`.
    pub surface: Option<String>,
    /// `extra.description` on approval events (the matched pattern).
    pub description: Option<String>,
    /// `extra.choice` on `post_approval_response`: `once`, `deny`, ...
    pub choice: Option<String>,
    pub completed: Option<bool>,
    pub interrupted: Option<bool>,
    pub failed: Option<bool>,
    pub turn_exit_reason: Option<String>,
    pub model: Option<String>,
    /// `extra.child_session_id` on `subagent_start`/`subagent_stop`.
    pub child_session_id: Option<String>,
    pub schema_version: Option<String>,
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

fn boolean(v: Option<&Value>) -> Option<bool> {
    v.and_then(Value::as_bool)
}

/// Parse one hook payload (the JSON a hook command reads on stdin).
/// `None` when it is not an object or names no event.
#[must_use]
pub fn parse_hook(v: &Value) -> Option<HermesHook> {
    let obj = v.as_object()?;
    let name = obj.get("hook_event_name")?.as_str()?;
    let extra = obj.get("extra").and_then(Value::as_object);
    let x = |k: &str| extra.and_then(|e| e.get(k));
    Some(HermesHook {
        event: HermesHookEvent::from_name(name),
        session_id: string(obj.get("session_id")),
        turn_id: string(x("turn_id")),
        tool_call_id: string(x("tool_call_id")),
        tool_name: string(obj.get("tool_name")),
        platform: string(x("platform")),
        surface: string(x("surface")),
        description: string(x("description")),
        choice: string(x("choice")),
        completed: boolean(x("completed")),
        interrupted: boolean(x("interrupted")),
        failed: boolean(x("failed")),
        turn_exit_reason: string(x("turn_exit_reason")),
        model: string(x("model")),
        child_session_id: string(x("child_session_id")),
        schema_version: string(x("telemetry_schema_version")),
    })
}

impl HermesHook {
    /// True for events fired inside a delegated child. Its
    /// `session_id` is the child's, so counting its `on_session_end`
    /// as the parent harness finishing a turn would be wrong.
    #[must_use]
    pub fn is_subagent(&self) -> bool {
        self.platform.as_deref() == Some("subagent")
    }

    /// True for the `surface: "smart"` request Hermes emits from its
    /// own LLM pre-check. No dialog is shown for it and no
    /// `post_approval_response` follows, so treating it as a waiting
    /// dialog would stick the harness in `permission` forever.
    #[must_use]
    pub fn is_smart_precheck(&self) -> bool {
        self.surface.as_deref() == Some("smart")
    }

    /// What pairs an approval request with its response. The shell
    /// payload carries no request id, so prefer the `tool_call_id` of
    /// the call being gated, else `(turn_id, description)`. `None`
    /// when neither is present.
    #[must_use]
    pub fn approval_key(&self) -> Option<ApprovalKey> {
        if let Some(id) = &self.tool_call_id {
            return Some(ApprovalKey::ToolCall(id.clone()));
        }
        match (&self.turn_id, &self.description) {
            (Some(t), Some(d)) => Some(ApprovalKey::TurnDescription(t.clone(), d.clone())),
            _ => None,
        }
    }

    /// True for a turn end that was a user interrupt (Ctrl+C): the
    /// turn did not complete and Hermes reported `interrupted`, so it
    /// is not a normal hand-back to the user.
    #[must_use]
    pub fn ended_interrupted(&self) -> bool {
        self.event == HermesHookEvent::SessionEnd && self.interrupted == Some(true)
    }
}

#[cfg(test)]
mod tests;
