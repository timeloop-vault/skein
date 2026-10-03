//! Which Skein build the agent runs under (#535).

use serde::Serialize;

use super::super::state::AgentApiState;

/// What `skein_info` reports. camelCase like the other verbs' output.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoOut {
    pub version: &'static str,
    pub profile: &'static str,
    pub identifier: String,
    pub commit: Option<&'static str>,
}

/// The running build: version, profile, bundle identifier and commit.
///
/// Reads no database; authentication happens before any verb runs.
pub fn skein_info(state: &AgentApiState) -> InfoOut {
    InfoOut {
        version: crate::build_info::VERSION,
        profile: crate::build_info::profile_for_identifier(&state.identifier),
        identifier: state.identifier.clone(),
        commit: crate::build_info::commit(),
    }
}
