//! The Tauri boundary — one command, for the settings readout.
//!
//! The frontend never calls the review API itself; it has the Tauri
//! commands from #212 for that. All it needs to know is whether the
//! server came up, so a failed bind is visible rather than showing as
//! an agent that mysteriously has no tools (#176).

use super::state::{AgentApiEndpoint, AgentApiStatus};

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn agent_api_status(endpoint: tauri::State<'_, AgentApiEndpoint>) -> AgentApiStatus {
    endpoint.snapshot()
}
