//! Start a harness's sidecar and wait until it is usable (#676, #730):
//! the locked-database retry, readiness polling and session creation that
//! used to live in the app's `opencode_sidecar_start` command. Blocking,
//! so it is called from a blocking thread by the daemon's `pty_spawn`.

use std::time::Duration;

use tokio::runtime::Handle;

use super::http::{self, READY_POLL, READY_TIMEOUT};
use super::start::{
    RETRY_BACKOFF_MS, SidecarStartDeps, SidecarStartResult, is_retryable, start_blocking,
};
use super::{OnExit, SidecarInfo, SidecarState};

/// Only one sidecar initialises at a time: concurrent boots share one
/// `opencode.db` and race on its lock. Held from spawn until ready/failed.
///
/// If one sidecar hangs at startup this is held for up to `READY_TIMEOUT`
/// (plus the failed start's tree kill) and delays other harnesses' v2
/// starts. Accepted: a hung start is rare and loud.
static START_GATE: parking_lot::Mutex<()> = parking_lot::const_mutex(());

/// What a start is for. Blank `agent` / `session_id` mean none.
#[derive(Clone, Copy)]
pub struct SidecarStartInput<'a> {
    pub room_id: &'a str,
    pub harness_id: &'a str,
    pub cwd: &'a str,
    pub agent: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub env_profile: Option<&'a str>,
}

struct Ready {
    info: SidecarInfo,
    url: String,
    password: String,
    client: reqwest::Client,
}

/// One start attempt, up to readiness. On failure the sidecar is stopped.
fn start_attempt(
    handle: &Handle,
    deps: SidecarStartDeps<'_>,
    on_exit: &OnExit,
    input: SidecarStartInput<'_>,
) -> Result<Ready, String> {
    let harness_id = input.harness_id;
    let manager = deps.manager;
    let info = start_blocking(
        deps,
        input.room_id,
        harness_id,
        input.cwd,
        input.env_profile,
        on_exit.clone(),
    )?;
    let fail = |msg: String| {
        manager.stop_if_generation(harness_id, info.generation);
        msg
    };
    let url = format!("http://127.0.0.1:{}", info.port);
    let Some(password) = manager.password_for(harness_id, info.generation) else {
        return Err(fail("opencode server exited during start".to_owned()));
    };
    let client = http::client().map_err(fail)?;
    handle
        .block_on(http::wait_ready(
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
        ))
        .map_err(fail)?;
    Ok(Ready {
        info,
        url,
        password,
        client,
    })
}

/// Start (or restart) the harness's sidecar, wait until it answers, and
/// return a session id: the given one as-is (resume probing is separate),
/// otherwise a freshly created one. Blocks; call from a blocking thread
/// inside a tokio runtime (`handle`).
///
/// # Errors
/// The program is missing, the sidecar died or never became ready, or the
/// session could not be created. A started sidecar is stopped on failure.
pub fn start_sidecar_blocking(
    handle: &Handle,
    deps: SidecarStartDeps<'_>,
    on_exit: &OnExit,
    input: SidecarStartInput<'_>,
) -> Result<SidecarStartResult, String> {
    let harness_id = input.harness_id;
    let mut attempt = 0;
    let ready = loop {
        let result = {
            let _gate = START_GATE.lock();
            start_attempt(handle, deps, on_exit, input)
        };
        match result {
            Ok(ready) => break ready,
            Err(msg) => {
                if is_retryable(&msg)
                    && let Some(wait) = RETRY_BACKOFF_MS.get(attempt)
                {
                    attempt += 1;
                    tracing::warn!(harness = %harness_id, attempt, error = %msg, "opencode sidecar start hit a locked database, retrying");
                    std::thread::sleep(Duration::from_millis(*wait));
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
    } = ready;
    let session_id = match input.session_id.filter(|s| !s.trim().is_empty()) {
        Some(existing) => existing.to_owned(),
        None => {
            match handle.block_on(http::create_session(
                &client,
                &url,
                &password,
                input.cwd,
                input.agent,
            )) {
                Ok(id) => id,
                Err(msg) => {
                    deps.manager.stop_if_generation(harness_id, info.generation);
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
        agent = ?input.agent,
        "opencode sidecar ready"
    );
    Ok(SidecarStartResult {
        url,
        port: info.port,
        session_id,
        generation: info.generation,
    })
}
