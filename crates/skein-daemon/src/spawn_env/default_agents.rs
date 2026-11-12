//! The per-kind default agent (#730), moved from a frontend
//! localStorage pref into `settings.json`. `None` in the stored settings
//! means "never imported"; `Some` means the daemon owns the value.

use std::collections::BTreeMap;

use crate::harness_kind::HarnessKind;
use serde::Serialize;

use super::state::SpawnEnvState;

/// Result of `default_agents_import_impl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultAgentsImport {
    /// True when this call stored the incoming map.
    pub imported: bool,
    /// What is stored now.
    pub default_agents: BTreeMap<String, String>,
}

/// Store `next` as the default-agents value and persist, under the
/// settings write lock held by the caller.
fn persist(
    state: &SpawnEnvState,
    settings: &mut crate::spawn_settings::SpawnSettings,
    next: BTreeMap<String, String>,
) -> Result<(), String> {
    let mut updated = settings.clone();
    updated.default_agents = Some(next);
    crate::spawn_settings::save(&state.data_dir, &updated)?;
    *settings = updated;
    Ok(())
}

/// While the settings file was unreadable at load, the in-memory settings
/// are defaults; saving them would replace the user's file (env profiles
/// included). Only an explicit settings save clears that state.
fn refuse_if_degraded(state: &SpawnEnvState) -> Result<(), String> {
    match state.degraded.read().as_deref() {
        Some(why) => Err(format!(
            "settings.json is unreadable ({why}); default agents are not changed until it is fixed or saved from Settings"
        )),
        None => Ok(()),
    }
}

/// One-time, non-overwriting import of the old frontend pref. Blank
/// names and keys that are not a harness kind are dropped.
///
/// # Errors
/// The settings file is unreadable (degraded), or could not be written.
pub fn default_agents_import_impl(
    state: &SpawnEnvState,
    incoming: BTreeMap<String, String>,
) -> Result<DefaultAgentsImport, String> {
    let mut settings = state.settings.write();
    refuse_if_degraded(state)?;
    if let Some(stored) = &settings.default_agents {
        return Ok(DefaultAgentsImport {
            imported: false,
            default_agents: stored.clone(),
        });
    }
    let cleaned: BTreeMap<String, String> = incoming
        .into_iter()
        .filter(|(k, _)| HarnessKind::from_name(k).is_some())
        .filter_map(|(k, v)| {
            let v = v.trim();
            (!v.is_empty()).then(|| (k, v.to_owned()))
        })
        .collect();
    persist(state, &mut settings, cleaned.clone())?;
    Ok(DefaultAgentsImport {
        imported: true,
        default_agents: cleaned,
    })
}

/// Set (or, for `None` / blank, remove) one kind's default agent.
///
/// # Errors
/// The settings file is unreadable (degraded), or could not be written.
pub fn default_agent_set_impl(
    state: &SpawnEnvState,
    kind: HarnessKind,
    agent: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let mut settings = state.settings.write();
    refuse_if_degraded(state)?;
    let mut map = settings.default_agents.clone().unwrap_or_default();
    match agent.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => {
            map.insert(kind.as_str().to_owned(), a.to_owned());
        }
        None => {
            map.remove(kind.as_str());
        }
    }
    persist(state, &mut settings, map.clone())?;
    Ok(map)
}

#[cfg(test)]
#[path = "default_agents_tests.rs"]
mod tests;
