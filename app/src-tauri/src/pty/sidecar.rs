//! The environment of the opencode `serve` sidecar (#676): exactly what a
//! PTY spawn of the same harness would get, as owned pairs.

use std::ffi::OsString;

use portable_pty::CommandBuilder;

use super::env::{apply_env, env_pairs};
use super::probe::probe_result;
use crate::agent_api::state::HarnessIdentity;
use crate::harness_config::HarnessConfig;
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;

/// Variables the sidecar manager (server) and the TUI binding (client) set
/// themselves; an inherited value must never survive into either.
const PASSWORD_VARS: [&str; 2] = ["OPENCODE_PASSWORD", "OPENCODE_SERVER_PASSWORD"];

fn is_password_var(key: &std::ffi::OsStr) -> bool {
    let key = key.to_string_lossy();
    PASSWORD_VARS.iter().any(|v| key.eq_ignore_ascii_case(v))
}

/// Drop every inherited opencode password variable from a builder (any
/// case). A bound TUI spawn runs this before its own secret is applied, so
/// an inherited `OPENCODE_SERVER_PASSWORD` never reaches the TUI either.
pub(super) fn strip_password_vars(builder: &mut CommandBuilder) {
    let keys: Vec<OsString> = env_pairs(builder)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| is_password_var(k))
        .collect();
    for key in keys {
        builder.env_remove(key);
    }
}

/// Build the sidecar's environment: the PTY policy (`apply_env` with the
/// login-shell probe, the room's agent identity, #215 injection for
/// opencode so `OPENCODE_CONFIG` is set, and the env profile), minus any
/// inherited opencode password. Blocks on the probe like `spawn` does.
pub(crate) fn sidecar_env(
    exe: &str,
    settings: &SpawnSettings,
    agent: Option<&HarnessIdentity>,
    harness_config: Option<&HarnessConfig>,
    env_profile: Option<&str>,
) -> Vec<(OsString, OsString)> {
    let program = HarnessKind::Opencode
        .program()
        .expect("opencode has a program");
    let injection = crate::harness_config::injection_for(
        HarnessKind::Opencode,
        program,
        harness_config,
        settings,
        agent,
    );
    let mut builder = CommandBuilder::new(exe);
    apply_env(
        &mut builder,
        settings,
        probe_result(),
        agent,
        &injection,
        env_profile,
    );
    env_pairs(&builder)
        .into_iter()
        .filter(|(k, _)| !is_password_var(k))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{is_password_var, strip_password_vars};
    use portable_pty::CommandBuilder;
    use std::ffi::OsStr;

    #[test]
    fn strip_removes_password_vars_only() {
        let mut b = CommandBuilder::new("x");
        b.env("OPENCODE_SERVER_PASSWORD", "a");
        b.env("opencode_password", "b");
        b.env("SKEIN_KEEP", "c");
        strip_password_vars(&mut b);
        let keys: Vec<String> = super::env_pairs(&b)
            .into_iter()
            .map(|(k, _)| k.to_string_lossy().to_ascii_uppercase())
            .collect();
        assert!(keys.contains(&"SKEIN_KEEP".to_owned()));
        assert!(!keys.iter().any(|k| k.contains("OPENCODE_")));
    }

    #[test]
    fn password_vars_match_case_insensitively() {
        assert!(is_password_var(OsStr::new("OPENCODE_PASSWORD")));
        assert!(is_password_var(OsStr::new("opencode_server_password")));
        assert!(!is_password_var(OsStr::new("OPENCODE_CONFIG")));
    }
}
