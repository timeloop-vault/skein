//! The body of the `pty_spawn` command, free of any shell types (#578):
//! the command resolves its managed state and a [`StreamSink`] and calls
//! [`pty_spawn_impl`], so a network transport can drive the same spawn.

use std::path::Path;
use std::sync::Arc;

use super::opencode_prepare::{Prepared, prepare_opencode_spawn};
use super::{PtyEvent, PtyManager, PtyObserver, SidecarBinding, SpawnRequest};
use crate::agent_endpoint::{AgentApiEndpoint, HarnessIdentity};
use crate::db::Database;
use crate::harness_config::HarnessConfig;
use crate::harness_kind::HarnessKind;
use crate::opencode_sidecar::launch::{SidecarStartInput, start_sidecar_blocking};
use crate::opencode_sidecar::start::SidecarStartDeps;
use crate::opencode_sidecar::{OnExit, OpencodeSidecarManager};
use crate::spawn_env::lookups::{OpencodeVersionDto, opencode_cli_version_impl};
use crate::spawn_env::state::SpawnEnvState;
use crate::stream_sink::StreamSink;

/// Wire shape for `pty_spawn`'s return value.
///
/// `injected` names whether #215's config injection actually happened
/// for this spawn (a non-empty `Injection`) — the one thing the
/// frontend cannot compute itself, since `injection_for` lives entirely
/// on the Rust side. #238's nudge gate refuses to send a prompt to a
/// harness whose CLI might not even have the review tools wired up.
///
/// `mouse_clicks_disabled` is whether `CLAUDE_CODE_DISABLE_MOUSE_CLICKS`
/// is truthy in the final spawn environment. Only Rust sees that env
/// (login shell, inherited, user extras): the frontend's link-click
/// deferral (#269) must not stand aside when Claude ignores clicks (#401).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySpawnResult {
    pub id: String,
    pub injected: bool,
    /// A Hermes harness whose status hooks are installed, approved and on
    /// a verified version (#625); false for every other kind.
    pub hermes_hooks: bool,
    pub mouse_clicks_disabled: bool,
    /// The env profile applied to this spawn (#615), so the frontend can
    /// record which profile a running harness started with.
    pub env_profile: Option<String>,
    /// The sidecar generation the PTY is bound to: the one the daemon
    /// started for an opencode v2 spawn (#730).
    pub sidecar_generation: Option<u64>,
    /// The opencode CLI version, whenever this spawn probed it (v1 too).
    pub opencode_version: Option<OpencodeVersionDto>,
    /// Set when the daemon-started sidecar created or confirmed the session.
    pub session_id: Option<String>,
}

/// The managed state a spawn reads, borrowed from wherever it is stored.
#[derive(Clone, Copy)]
pub struct PtySpawnDeps<'a> {
    pub manager: &'a PtyManager,
    pub spawn_env: &'a SpawnEnvState,
    pub db: &'a Database,
    pub endpoint: &'a AgentApiEndpoint,
    pub harness_config: &'a HarnessConfig,
    pub sidecars: &'a OpencodeSidecarManager,
}

/// The command's own arguments, owned.
pub struct PtySpawnArgs {
    pub cmd: Vec<String>,
    pub cwd: String,
    pub rows: u16,
    pub cols: u16,
    pub room_id: String,
    pub harness_id: String,
    pub kind: HarnessKind,
    pub env_profile: Option<String>,
    /// The harness record's agent and opencode session id (#730), never
    /// parsed from `cmd`. Read only when the daemon prepares an opencode v2
    /// spawn.
    pub agent: Option<String>,
    pub session_id: Option<String>,
}

/// Records how long each `Data` send takes. A failed send is ignored: it
/// only means the frontend dropped its end.
struct TimedSink(Box<dyn StreamSink<PtyEvent>>);

impl StreamSink<PtyEvent> for TimedSink {
    fn send(&self, event: PtyEvent) -> Result<(), String> {
        let is_data = matches!(event, PtyEvent::Data { .. });
        let t0 = std::time::Instant::now();
        let r = self.0.send(event);
        if is_data {
            crate::perf_spans::PTY_CHUNK_SEND.record(t0.elapsed());
        }
        r
    }
}

/// Spawn a child on a fresh PTY and stream its events into `sink`.
///
/// `stop_sidecar` ends the sidecar generation a PTY was bound to (#676); it
/// is called when the child exits and when the spawn itself fails. The
/// caller supplies it because the managed states are not `Arc`-wrapped, so
/// the exit closure cannot borrow them.
pub fn pty_spawn_impl(
    deps: PtySpawnDeps<'_>,
    args: PtySpawnArgs,
    sink: impl StreamSink<PtyEvent>,
    stop_sidecar: impl Fn(&SidecarBinding) + Send + Sync + 'static,
    sidecar_on_exit: &OnExit,
) -> Result<PtySpawnResult, String> {
    pty_spawn_with_sink(
        deps,
        args,
        Some(Box::new(sink)),
        stop_sidecar,
        sidecar_on_exit,
        None,
    )
}

/// [`pty_spawn_impl`] where the view is optional: with `None` the child runs
/// unattended, its output kept for the first `PtyManager::attach` (#731).
/// `observer` (#732) hears the process whether or not a view is attached.
pub fn pty_spawn_with_sink(
    deps: PtySpawnDeps<'_>,
    args: PtySpawnArgs,
    sink: Option<Box<dyn StreamSink<PtyEvent>>>,
    stop_sidecar: impl Fn(&SidecarBinding) + Send + Sync + 'static,
    sidecar_on_exit: &OnExit,
    observer: Option<Arc<dyn PtyObserver>>,
) -> Result<PtySpawnResult, String> {
    let PtySpawnArgs {
        cmd,
        cwd,
        rows,
        cols,
        room_id,
        harness_id,
        kind,
        env_profile,
        agent: record_agent,
        session_id: record_session,
    } = args;
    let id = uuid::Uuid::new_v4().to_string();
    let settings = deps.spawn_env.snapshot();
    // #730: an opencode harness is probed, and
    // on v2 gets its sidecar and `--server` argv here.
    let prepared = prepare_opencode_spawn(
        cmd,
        kind,
        record_agent.as_deref(),
        record_session.as_deref(),
        || opencode_cli_version_impl(&settings),
        |agent, session_id| {
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|e| format!("no async runtime to start the opencode server: {e}"))?;
            start_sidecar_blocking(
                &handle,
                SidecarStartDeps {
                    spawn_env: deps.spawn_env,
                    db: deps.db,
                    endpoint: deps.endpoint,
                    harness_config: deps.harness_config,
                    manager: deps.sidecars,
                },
                sidecar_on_exit,
                SidecarStartInput {
                    room_id: &room_id,
                    harness_id: &harness_id,
                    cwd: &cwd,
                    agent,
                    session_id,
                    env_profile: env_profile.as_deref(),
                },
            )
        },
    )?;
    let Prepared {
        cmd,
        generation: sidecar_generation,
        version: opencode_version,
        session_id: sidecar_session_id,
        started_here,
    } = prepared;
    // #213: mint (or reuse) the room's review token and hand the
    // harness its endpoint. A failure here is not a reason to
    // refuse the spawn — the terminal still works, the agent
    // simply has no review tools — but it is logged rather than
    // swallowed (#176).
    let agent = harness_identity(deps.endpoint, deps.db, &room_id, &harness_id);
    // #676: the TUI of an opencode v2 harness attaches to the sidecar
    // with its password; the password is gone with the sidecar.
    let secret_env = match sidecar_generation {
        None => Vec::new(),
        Some(generation) => {
            let Some(password) = deps.sidecars.password_for(&harness_id, generation) else {
                if started_here {
                    deps.sidecars.stop_if_generation(&harness_id, generation);
                }
                return Err("opencode server for this harness is gone".to_owned());
            };
            vec![("OPENCODE_PASSWORD".to_owned(), password)]
        }
    };
    let binding = sidecar_generation.map(|g| (harness_id.clone(), g));
    let stop_sidecar = Arc::new(stop_sidecar);
    let exit_stop = Arc::clone(&stop_sidecar);
    let exit_binding = binding.clone();
    let bound_for_failure = binding.clone();
    let outcome = deps
        .manager
        .spawn_with_sink(
            SpawnRequest {
                id: id.clone(),
                cmd: &cmd,
                cwd: Path::new(&cwd),
                rows,
                cols,
                settings: &settings,
                agent: agent.as_ref(),
                kind,
                harness_config: Some(deps.harness_config),
                env_profile: env_profile.as_deref(),
                secret_env,
                sidecar: binding,
            },
            sink.map(|s| Box::new(TimedSink(s)) as Box<dyn StreamSink<PtyEvent>>),
            // Runs whether or not a view is attached (#731).
            Some(Box::new(move |_code| {
                if let Some(binding) = exit_binding.as_ref() {
                    exit_stop(binding);
                }
            })),
            observer,
        )
        .map_err(|e| {
            // The spawn failed: nothing will ever attach to the sidecar.
            if let Some(binding) = bound_for_failure.as_ref() {
                stop_sidecar(binding);
            }
            e.to_string()
        })?;
    Ok(PtySpawnResult {
        id,
        injected: outcome.injected,
        hermes_hooks: outcome.hermes_hooks,
        mouse_clicks_disabled: outcome.mouse_clicks_disabled,
        env_profile: outcome.env_profile,
        sidecar_generation,
        opencode_version,
        session_id: sidecar_session_id,
    })
}

/// #213: mint (or reuse) the room's review token and hand the harness its
/// endpoint. A failure is not a reason to refuse a spawn — the agent
/// simply has no review tools — but it is logged rather than swallowed
/// (#176).
pub fn harness_identity(
    endpoint: &AgentApiEndpoint,
    db: &Database,
    room_id: &str,
    harness_id: &str,
) -> Option<HarnessIdentity> {
    endpoint.mcp_url().and_then(|url| {
        match db.ensure_room_token(room_id, crate::review::now_ms()) {
            Ok(token) => Some(HarnessIdentity {
                url,
                token,
                room_id: room_id.to_owned(),
                harness_id: harness_id.to_owned(),
            }),
            Err(e) => {
                tracing::error!(room_id, error = %e, "agent api: minting a room token failed");
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::PtySpawnResult;

    /// #238: the frontend's `harnessInput` gate reads `injected` off
    /// `pty_spawn`'s resolved value — pin the wire shape so a field
    /// rename here doesn't silently turn into `undefined` there.
    #[test]
    fn pty_spawn_result_serializes_as_camel_case() {
        let json = serde_json::to_value(PtySpawnResult {
            id: "abc".to_owned(),
            injected: true,
            hermes_hooks: true,
            mouse_clicks_disabled: true,
            env_profile: Some("proxy".to_owned()),
            sidecar_generation: Some(4),
            opencode_version: Some(super::OpencodeVersionDto {
                major: 2,
                version: "2.2.0".to_owned(),
            }),
            session_id: None,
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "id": "abc",
                "injected": true,
                "hermesHooks": true,
                "mouseClicksDisabled": true,
                "envProfile": "proxy",
                "sidecarGeneration": 4,
                "opencodeVersion": {"major": 2, "version": "2.2.0"},
                "sessionId": null
            })
        );
    }
}
