//! The Tauri-free half of starting a harness's `opencode serve` sidecar
//! (#676): the wire shapes, the retry rule and the blocking start. The
//! `tauri::command`, the start guard and the exit hook (which emits an
//! event and ends the bound TUI through an `AppHandle`) stay in the app.

use std::ffi::OsString;
use std::path::PathBuf;

use super::{ExitNotice, OnExit, OpencodeSidecarManager, SidecarInfo, StartRequest};
use crate::agent_endpoint::AgentApiEndpoint;
use crate::db::Database;
use crate::harness_config::HarnessConfig;
use crate::spawn_env::state::SpawnEnvState;

/// What a sidecar start returns.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStartResult {
    pub url: String,
    pub port: u16,
    pub session_id: String,
    pub generation: u64,
}

/// Payload of the `skein://opencode-sidecar-exit` event: a sidecar that
/// exited on its own (never sent after a stop).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarExitPayload {
    harness_id: String,
    generation: u64,
    code: Option<i32>,
}

impl From<ExitNotice> for SidecarExitPayload {
    fn from(n: ExitNotice) -> Self {
        Self {
            harness_id: n.harness_id,
            generation: n.generation,
            code: n.code,
        }
    }
}

pub const EXIT_EVENT: &str = "skein://opencode-sidecar-exit";

/// The managed state a start reads, borrowed from wherever it is stored.
#[derive(Clone, Copy)]
pub struct SidecarStartDeps<'a> {
    pub spawn_env: &'a SpawnEnvState,
    pub db: &'a Database,
    pub endpoint: &'a AgentApiEndpoint,
    pub harness_config: &'a HarnessConfig,
    pub manager: &'a OpencodeSidecarManager,
}

/// Resolve the exe, build the PTY-equivalent env, pick a port and start
/// the child. Blocks (login-shell probe, process spawn).
///
/// # Errors
/// The program is not on `PATH`, no port could be picked, or the spawn failed.
pub fn start_blocking(
    deps: SidecarStartDeps<'_>,
    room_id: &str,
    harness_id: &str,
    cwd: &str,
    env_profile: Option<&str>,
    on_exit: OnExit,
) -> Result<SidecarInfo, String> {
    let SidecarStartDeps {
        spawn_env,
        db,
        endpoint,
        harness_config,
        manager,
    } = deps;
    let settings = spawn_env.snapshot();

    let program = crate::harness_kind::HarnessKind::Opencode
        .program()
        .expect("opencode has a program");
    let (resolved, _path) = crate::pty::harness_program_lookup(program, &settings);
    let exe = resolved.ok_or_else(|| format!("{program} was not found on PATH"))?;

    let agent = crate::pty::command::harness_identity(endpoint, db, room_id, harness_id);
    let env = crate::pty::sidecar_env(
        &exe,
        &settings,
        agent.as_ref(),
        Some(harness_config),
        env_profile,
    );
    let port = crate::pty::opencode::pick_free_port()?;

    manager.start(StartRequest {
        harness_id: harness_id.to_owned(),
        cwd: PathBuf::from(cwd),
        env,
        exe: PathBuf::from(OsString::from(exe)),
        port,
        lower_priority: settings.child_priority() == crate::spawn_settings::ChildPriority::Lowered,
        on_exit,
        args_override: None,
    })
}

/// The wait before each retry (so at most this many retries).
pub const RETRY_BACKOFF_MS: [u64; 2] = [300, 900];

/// A start that died because another opencode held the shared database
/// lock is worth retrying (on a new port); every other failure is not.
#[must_use]
pub fn is_retryable(msg: &str) -> bool {
    msg.starts_with("opencode server exited") && msg.contains("database is locked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_locked_database_exit_is_retryable() {
        assert!(is_retryable(
            "opencode server exited with code 1: SQLiteError: database is locked | at x"
        ));
        assert!(!is_retryable("opencode server exited with code 1: boom"));
        assert!(!is_retryable(
            "opencode server not ready after 20s: database is locked"
        ));
    }

    #[test]
    fn start_result_and_exit_payload_are_camel_case() {
        let json = serde_json::to_value(SidecarStartResult {
            url: "http://127.0.0.1:1".into(),
            port: 1,
            session_id: "ses_a".into(),
            generation: 3,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"url":"http://127.0.0.1:1","port":1,"sessionId":"ses_a","generation":3})
        );
        let json = serde_json::to_value(SidecarExitPayload::from(ExitNotice {
            harness_id: "h".into(),
            generation: 2,
            code: None,
        }))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"harnessId":"h","generation":2,"code":null})
        );
    }
}
