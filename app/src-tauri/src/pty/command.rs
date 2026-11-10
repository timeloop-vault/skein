//! The body of the `pty_spawn` command, free of any shell types (#578):
//! the command resolves its managed state and a [`StreamSink`] and calls
//! [`pty_spawn_impl`], so a network transport can drive the same spawn.

use std::path::Path;
use std::sync::Arc;

use super::{PtyEvent, PtyManager, SidecarBinding, SpawnRequest};
use crate::agent_api::state::{AgentApiEndpoint, HarnessIdentity};
use crate::commands::spawn_env::SpawnEnvState;
use crate::db::Database;
use crate::harness_config::HarnessConfig;
use crate::harness_kind::HarnessKind;
use crate::opencode_sidecar::OpencodeSidecarManager;
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
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtySpawnResult {
    pub(crate) id: String,
    pub(crate) injected: bool,
    /// A Hermes harness whose status hooks are installed, approved and on
    /// a verified version (#625); false for every other kind.
    pub(crate) hermes_hooks: bool,
    pub(crate) mouse_clicks_disabled: bool,
    /// The env profile applied to this spawn (#615), so the frontend can
    /// record which profile a running harness started with.
    pub(crate) env_profile: Option<String>,
}

/// The managed state a spawn reads, borrowed from wherever it is stored.
#[derive(Clone, Copy)]
pub(crate) struct PtySpawnDeps<'a> {
    pub(crate) manager: &'a PtyManager,
    pub(crate) spawn_env: &'a SpawnEnvState,
    pub(crate) db: &'a Database,
    pub(crate) endpoint: &'a AgentApiEndpoint,
    pub(crate) harness_config: &'a HarnessConfig,
    pub(crate) sidecars: &'a OpencodeSidecarManager,
}

/// The command's own arguments, owned.
pub(crate) struct PtySpawnArgs {
    pub(crate) cmd: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) room_id: String,
    pub(crate) harness_id: String,
    pub(crate) kind: HarnessKind,
    pub(crate) env_profile: Option<String>,
    pub(crate) sidecar_generation: Option<u64>,
}

/// Spawn a child on a fresh PTY and stream its events into `sink`.
///
/// `stop_sidecar` ends the sidecar generation a PTY was bound to (#676); it
/// is called when the child exits and when the spawn itself fails. The
/// caller supplies it because the managed states are not `Arc`-wrapped, so
/// the exit closure cannot borrow them.
pub(crate) fn pty_spawn_impl(
    deps: PtySpawnDeps<'_>,
    args: PtySpawnArgs,
    sink: impl StreamSink<PtyEvent>,
    stop_sidecar: impl Fn(&SidecarBinding) + Send + Sync + 'static,
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
        sidecar_generation,
    } = args;
    let id = uuid::Uuid::new_v4().to_string();
    let settings = deps.spawn_env.snapshot();
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
        .spawn(
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
            move |event| {
                // A send only fails if the frontend dropped its end;
                // nothing useful we can do at that point.
                let is_data = matches!(event, PtyEvent::Data { .. });
                if matches!(event, PtyEvent::Exit { .. })
                    && let Some(binding) = exit_binding.as_ref()
                {
                    exit_stop(binding);
                }
                let t0 = std::time::Instant::now();
                let _ = sink.send(event);
                if is_data {
                    crate::perf_spans::PTY_CHUNK_SEND.record(t0.elapsed());
                }
            },
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
    })
}

/// #213: mint (or reuse) the room's review token and hand the harness its
/// endpoint. A failure is not a reason to refuse a spawn — the agent
/// simply has no review tools — but it is logged rather than swallowed
/// (#176).
pub(crate) fn harness_identity(
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
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "id": "abc",
                "injected": true,
                "hermesHooks": true,
                "mouseClicksDisabled": true,
                "envProfile": "proxy"
            })
        );
    }
}
