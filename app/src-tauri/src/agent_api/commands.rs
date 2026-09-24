//! The Tauri boundary — commands the frontend calls directly, as
//! opposed to the HTTP/MCP surface an agent calls.
//!
//! The frontend never calls the review API itself; it has the Tauri
//! commands from #212 for that. `agent_api_status` says whether the
//! server came up, so a failed bind is visible rather than showing as
//! an agent that mysteriously has no tools (#176). `mail_unread`
//! (#329) is the same idea for the mailbox badge: a read-only summary
//! a harness tab polls, never marking anything read — only
//! `read_messages`, over the agent-facing surface, does that.

use std::sync::Arc;

use super::state::{AgentApiEndpoint, AgentApiState, AgentApiStatus};
use super::verbs::{self, MailUnread};
use crate::db::Database;

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn agent_api_status(endpoint: tauri::State<'_, AgentApiEndpoint>) -> AgentApiStatus {
    endpoint.snapshot()
}

/// How much unread mail a harness has, and who it's from (#329). Pure
/// derivation lives in `verbs::unread_mail`; this only fetches the two
/// row sets and gets off the main thread for the sqlite reads (#171).
#[tauri::command]
pub async fn mail_unread(
    room_id: String,
    harness_id: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<MailUnread, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        let messages = db.unread_harness_messages(&room_id, &harness_id)?;
        let rooms = db.all_rooms()?;
        Ok(verbs::unread_mail(&messages, &rooms))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The frontend's answer to a `skein://agent-request` it received
/// (#328). `error` wins over `ok` when both are somehow present — an
/// error is the frontend actively saying something went wrong, and
/// that must not be masked by whatever placeholder `ok` value came
/// along with it. `ok: None` with no `error` completes with `null`,
/// for requests whose answer is "done", not a value.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn agent_request_complete(
    id: String,
    ok: Option<serde_json::Value>,
    error: Option<String>,
    state: tauri::State<'_, Arc<AgentApiState>>,
) -> Result<(), String> {
    let result = match error {
        Some(e) => Err(e),
        None => Ok(ok.unwrap_or(serde_json::Value::Null)),
    };
    state.complete_request(&id, result)
}
