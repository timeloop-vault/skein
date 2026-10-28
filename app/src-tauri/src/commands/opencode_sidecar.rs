//! Starting and stopping a harness's `opencode serve` sidecar (#676).
//! On opencode v2 the TUI attaches to it with `--server <url>`; the PTY
//! side of that binding is `pty_spawn`'s `sidecar_generation`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::{Emitter, Manager};

use super::spawn_env::SpawnEnvState;
use crate::db::Database;
use crate::opencode_sidecar::http::{self, READY_POLL, READY_TIMEOUT};
use crate::opencode_sidecar::{
    ExitNotice, OpencodeSidecarManager, SidecarInfo, SidecarState, StartRequest,
};

/// Wire shape of `opencode_sidecar_start`'s return value.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarStartResult {
    url: String,
    port: u16,
    session_id: String,
    generation: u64,
}

/// Payload of the `skein://opencode-sidecar-exit` event: a sidecar that
/// exited on its own (never sent after a stop).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarExitPayload {
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

const EXIT_EVENT: &str = "skein://opencode-sidecar-exit";

/// Resolve the exe, build the PTY-equivalent env, pick a port and start
/// the child. Blocks (login-shell probe, process spawn).
fn start_blocking(
    app: &tauri::AppHandle,
    room_id: &str,
    harness_id: &str,
    cwd: &str,
    env_profile: Option<&str>,
) -> Result<SidecarInfo, String> {
    let spawn_env = app.state::<SpawnEnvState>();
    let db = app.state::<Arc<Database>>();
    let endpoint = app.state::<crate::agent_api::state::AgentApiEndpoint>();
    let harness_config = app.state::<crate::harness_config::HarnessConfig>();
    let manager = app.state::<OpencodeSidecarManager>();
    let settings = spawn_env.snapshot();

    let program = crate::harness_kind::HarnessKind::Opencode
        .program()
        .expect("opencode has a program");
    let (resolved, _path) = crate::pty::harness_program_lookup(program, &settings);
    let exe = resolved.ok_or_else(|| format!("{program} was not found on PATH"))?;

    let agent = super::pty::harness_identity(&endpoint, &db, room_id, harness_id);
    let env = crate::pty::sidecar_env(
        &exe,
        &settings,
        agent.as_ref(),
        Some(&harness_config),
        env_profile,
    );
    let port = super::pty::pick_free_port()?;

    let exit_app = app.clone();
    manager.start(StartRequest {
        harness_id: harness_id.to_owned(),
        cwd: PathBuf::from(cwd),
        env,
        exe: PathBuf::from(OsString::from(exe)),
        port,
        lower_priority: settings.child_priority() == crate::spawn_settings::ChildPriority::Lowered,
        on_exit: Arc::new(move |notice| {
            tracing::info!(
                harness = %notice.harness_id,
                generation = notice.generation,
                code = ?notice.code,
                "opencode sidecar exited"
            );
            let (harness_id, generation) = (notice.harness_id.clone(), notice.generation);
            // Event first, so the frontend knows why the TUI died when the
            // PTY's own Exit arrives; then end the TUI, which would
            // otherwise reconnect forever to a dead server.
            let _ = exit_app.emit(EXIT_EVENT, SidecarExitPayload::from(notice));
            let ended = exit_app
                .state::<crate::pty::PtyManager>()
                .terminate_bound(&harness_id, generation);
            if ended > 0 {
                tracing::info!(harness = %harness_id, generation, ended, "terminated TUI of a dead sidecar");
            }
        }),
        args_override: None,
    })
}

/// Only one sidecar initialises at a time: concurrent boots share one
/// `opencode.db` and race on its lock. Held from spawn until ready/failed.
///
/// If one sidecar hangs at startup this is held for up to `READY_TIMEOUT`
/// (plus the failed start's tree kill) and delays other harnesses' v2
/// starts. Accepted: a hung start is rare and loud.
static START_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The wait before each retry (so at most this many retries).
const RETRY_BACKOFF_MS: [u64; 2] = [300, 900];

/// A start that died because another opencode held the shared database
/// lock is worth retrying (on a new port); every other failure is not.
fn is_retryable(msg: &str) -> bool {
    msg.starts_with("opencode server exited") && msg.contains("database is locked")
}

struct Ready {
    info: SidecarInfo,
    url: String,
    password: String,
    client: reqwest::Client,
    guard: StartGuard,
}

/// Stops the started sidecar on drop (the command future was dropped, or a
/// later step failed) unless disarmed right before a successful return.
struct StartGuard {
    app: tauri::AppHandle,
    harness_id: String,
    generation: u64,
    armed: bool,
}

impl StartGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if self.armed {
            self.app
                .state::<OpencodeSidecarManager>()
                .stop_if_generation(&self.harness_id, self.generation);
        }
    }
}

/// One start attempt, up to readiness. On failure the sidecar is stopped.
async fn start_attempt(
    app: &tauri::AppHandle,
    room_id: &str,
    harness_id: &str,
    cwd: &str,
    env_profile: Option<&str>,
) -> Result<Ready, String> {
    // The guard is built inside the blocking task, the moment the sidecar
    // exists: if this future is dropped while the task runs, the task's
    // result (and with it the guard) is dropped too, stopping the sidecar.
    let (info, guard) = {
        let app = app.clone();
        let (room_id, harness_id, cwd) =
            (room_id.to_owned(), harness_id.to_owned(), cwd.to_owned());
        let env_profile = env_profile.map(str::to_owned);
        tauri::async_runtime::spawn_blocking(move || {
            let info = start_blocking(&app, &room_id, &harness_id, &cwd, env_profile.as_deref())?;
            let guard = StartGuard {
                app,
                harness_id,
                generation: info.generation,
                armed: true,
            };
            Ok::<_, String>((info, guard))
        })
        .await
        .map_err(|e| e.to_string())??
    };
    let manager = app.state::<OpencodeSidecarManager>();
    let fail = |msg: String| {
        manager.stop_if_generation(harness_id, info.generation);
        msg
    };
    let url = format!("http://127.0.0.1:{}", info.port);
    let Some(password) = manager.password_for(harness_id, info.generation) else {
        return Err(fail("opencode server exited during start".to_owned()));
    };
    let client = http::client().map_err(fail)?;
    http::wait_ready(
        &client,
        &url,
        &password,
        READY_POLL,
        READY_TIMEOUT,
        || match manager.exited(harness_id, info.generation) {
            Some(SidecarState::Running) => None,
            Some(SidecarState::Exited(code)) => Some(format!(
                "opencode server exited with code {code}: {}",
                manager.stderr_tail(harness_id).join(" | ")
            )),
            None => Some("opencode server was stopped during start".to_owned()),
        },
    )
    .await
    .map_err(fail)?;
    Ok(Ready {
        info,
        url,
        password,
        client,
        guard,
    })
}

/// Start (or restart) the harness's sidecar, wait until it answers, and
/// return a session id: the given one as-is (resume probing is separate),
/// otherwise a freshly created one.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn opencode_sidecar_start(
    room_id: String,
    harness_id: String,
    cwd: String,
    agent: Option<String>,
    session_id: Option<String>,
    env_profile: Option<String>,
    app: tauri::AppHandle,
) -> Result<SidecarStartResult, String> {
    let mut attempt = 0;
    let ready = loop {
        let result = {
            let _gate = START_GATE.lock().await;
            start_attempt(&app, &room_id, &harness_id, &cwd, env_profile.as_deref()).await
        };
        match result {
            Ok(ready) => break ready,
            Err(msg) => {
                if is_retryable(&msg)
                    && let Some(wait) = RETRY_BACKOFF_MS.get(attempt)
                {
                    attempt += 1;
                    tracing::warn!(harness = %harness_id, attempt, error = %msg, "opencode sidecar start hit a locked database, retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(*wait)).await;
                    continue;
                }
                tracing::warn!(harness = %harness_id, error = %msg, "opencode sidecar start failed");
                return Err(msg);
            }
        }
    };
    let Ready {
        info,
        url,
        password,
        client,
        mut guard,
    } = ready;
    let manager = app.state::<OpencodeSidecarManager>();
    let session_id = match session_id.filter(|s| !s.trim().is_empty()) {
        Some(existing) => existing,
        None => {
            match http::create_session(&client, &url, &password, &cwd, agent.as_deref()).await {
                Ok(id) => id,
                Err(msg) => {
                    manager.stop_if_generation(&harness_id, info.generation);
                    tracing::warn!(harness = %harness_id, generation = info.generation, error = %msg, "opencode sidecar start failed");
                    return Err(msg);
                }
            }
        }
    };
    tracing::info!(
        harness = %harness_id,
        url = %url,
        pid = info.pid,
        generation = info.generation,
        session = %session_id,
        agent = ?agent,
        "opencode sidecar ready"
    );
    guard.disarm();
    Ok(SidecarStartResult {
        url,
        port: info.port,
        session_id,
        generation: info.generation,
    })
}

/// Stop the harness's sidecar: only the given `generation` when one is
/// passed (so a late stop cannot kill a restart's newer sidecar),
/// otherwise whatever is running.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn opencode_sidecar_stop(
    harness_id: String,
    generation: Option<u64>,
    sidecars: tauri::State<'_, OpencodeSidecarManager>,
) {
    match generation {
        Some(g) => {
            sidecars.stop_if_generation(&harness_id, g);
        }
        None => sidecars.stop(&harness_id),
    }
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
