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

use super::state::{AgentApiEndpoint, AgentApiStatus};
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
