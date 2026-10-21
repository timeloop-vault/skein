//! The one event every Hermes shell-hook POST turns into (#625).

use serde::Serialize;
use tauri::Emitter;

use super::state::AgentApiState;

/// Fired by `POST /api/harness/{permission,session-start,hermes-turn}`
/// when the body is a Hermes hook payload (a string `hook_event_name`
/// at top level). Hermes' hooks reuse the Claude routes' URLs for the
/// two events that have a Claude twin, so for such a body ONLY this
/// event is emitted, never the Claude-shaped one.
pub const HARNESS_HERMES_HOOK_EVENT: &str = "skein://harness-hermes-hook";

/// Payload of [`HARNESS_HERMES_HOOK_EVENT`]. Frontend contract — do not
/// rename a field without checking `app/src/` for the listener. Every
/// string is length-capped and ids are validated before they get here
/// (see `http/hermes_hooks.rs`); no tool input or prompt text is ever
/// carried. `description` (at most 500 chars: the approval's matched
/// command description) IS carried, and is never logged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessHermesHook {
    pub room_id: String,
    pub harness_id: String,
    /// The raw `hook_event_name` (`pre_llm_call`, `on_session_end`, …).
    pub event: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub platform: Option<String>,
    /// `smart` marks Hermes' own LLM pre-check: no dialog is shown.
    pub surface: Option<String>,
    pub description: Option<String>,
    pub choice: Option<String>,
    pub completed: Option<bool>,
    pub interrupted: Option<bool>,
    pub failed: Option<bool>,
    pub turn_exit_reason: Option<String>,
    pub model: Option<String>,
    pub child_session_id: Option<String>,
}

impl AgentApiState {
    /// Tell the frontend a Hermes hook fired for this harness.
    pub fn notify_harness_hermes_hook(&self, payload: HarnessHermesHook) {
        #[cfg(test)]
        self.test_hermes_events.lock().push(payload.clone());
        let Some(app) = self.app.as_ref() else { return };
        let (room_id, harness_id) = (payload.room_id.clone(), payload.harness_id.clone());
        if let Err(e) = app.emit(HARNESS_HERMES_HOOK_EVENT, payload) {
            tracing::warn!(room_id, harness_id, error = %e, "agent api: harness-hermes-hook emit failed");
        }
    }
}
