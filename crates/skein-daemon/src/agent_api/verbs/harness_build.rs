//! What a new harness is, decided in the daemon (#734): which kind and agent
//! a request means, and the record that goes into the store. `open_harness`
//! uses both, and so does `create_room` for a room's first harness. Both used
//! to be a round trip to the webview (`resolveKind`, `resolveAgent`,
//! `buildHarness`).
//!
//! The per-folder memory of the last kind and agent lived in the webview's
//! localStorage and is deliberately gone: resolution uses the daemon-side
//! defaults (design decision D4, docs/skeind-p2-design.md).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use skein_proto::room_commands::RoomCommand;
use skein_proto::rooms::{HarnessCreatedBy, Room};

use super::close_apply::{current_room, execute_all, lock_verbs};
use super::{VerbError, VerbResult, internal};
use crate::agent_api::state::AgentApiState;
use crate::db::Harness;
use crate::harness_argv::{
    AgentVerdict, FreshCmdInput, FreshCmdOutput, fresh_cmd, fresh_cmd_with, validate_agent,
};
use crate::harness_kind::HarnessKind;
use crate::harness_runner::EnsureRequest;
use crate::room_store::RoomStore;

/// The kind and agent a request resolved to. `agent` is `None` for "the
/// tool's own default", and always `None` for a kind that takes none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedHarness {
    pub(super) kind: HarnessKind,
    pub(super) agent: Option<String>,
}

#[cfg(windows)]
const FALLBACK_SHELL: &str = "powershell.exe";
#[cfg(not(windows))]
const FALLBACK_SHELL: &str = "sh";

/// The refusal for a kind an agent cannot ask for (#568).
const REMOTE_REFUSAL: &str =
    "remote harnesses need a host only the user can enter; add one from + harness (#568)";

/// The kind: the requested one, else `claude`. An unknown name and `remote`
/// are refused.
pub(super) fn resolve_kind(requested: Option<&str>) -> VerbResult<HarnessKind> {
    let Some(requested) = requested else {
        return Ok(HarnessKind::Claude);
    };
    match HarnessKind::from_name(requested) {
        None => Err(VerbError::Refused(format!(
            "unknown harness kind \"{requested}\""
        ))),
        Some(HarnessKind::Remote) => Err(VerbError::Refused(REMOTE_REFUSAL.to_owned())),
        Some(kind) => Ok(kind),
    }
}

/// The wording for an agent the CLI no longer offers.
fn unknown_agent_message(agent: &str, kind: HarnessKind) -> String {
    let (tool, quiet) = if kind == HarnessKind::Claude {
        ("Claude Code", "a resumed")
    } else {
        ("opencode", "an opencode")
    };
    format!(
        "agent \"{agent}\" is not one {tool} offers any more. Not spawning: {quiet} harness \
         would have run as a different agent without saying so. Pick another agent for this \
         harness."
    )
}

/// The agent: the requested one (an empty string means none and beats the
/// default), else the per-kind default from `defaults`. A blank candidate is
/// not checked. Only the verdict `unknown` refuses: a degraded listing never
/// blocks a spawn. `check` is asked at most once.
pub(super) fn resolve_agent_with(
    requested: Option<&str>,
    kind: HarnessKind,
    defaults: Option<&BTreeMap<String, String>>,
    check: impl FnOnce(&str) -> AgentVerdict,
) -> VerbResult<Option<String>> {
    if !kind.takes_agent() {
        return Ok(None);
    }
    let candidate = match requested {
        Some(given) => Some(given.to_owned()),
        None => defaults.and_then(|d| d.get(kind.as_str())).cloned(),
    };
    let Some(candidate) = candidate.filter(|c| !c.trim().is_empty()) else {
        return Ok(None);
    };
    match check(&candidate) {
        AgentVerdict::Unknown { agent } => {
            Err(VerbError::Refused(unknown_agent_message(&agent, kind)))
        }
        AgentVerdict::Ok | AgentVerdict::Unverified { .. } => Ok(Some(candidate)),
    }
}

/// [`resolve_kind`] then [`resolve_agent_with`], the agent checked against the
/// CLI's own listing for `cwd` (the same verdict `list_harness_agents`
/// gives). The listing runs the CLI, so it runs off the executor.
pub(super) async fn resolve(
    state: &AgentApiState,
    requested_kind: Option<&str>,
    requested_agent: Option<&str>,
    cwd: Option<&str>,
) -> VerbResult<ResolvedHarness> {
    let kind = resolve_kind(requested_kind)?;
    let settings = state.spawn_settings();
    let defaults = settings.default_agents.clone();
    let config = state.harness_config().cloned();
    let cwd = cwd.unwrap_or(".").to_owned();
    let requested_agent = requested_agent.map(str::to_owned);
    let agent = tokio::task::spawn_blocking(move || {
        resolve_agent_with(
            requested_agent.as_deref(),
            kind,
            defaults.as_ref(),
            |name| {
                let listing = crate::agents::list(kind, &cwd, &settings, config.as_ref());
                validate_agent(Some(name), &listing)
            },
        )
    })
    .await
    .map_err(|e| internal(format!("agent discovery panicked: {e}")))??;
    Ok(ResolvedHarness { kind, agent })
}

/// The room a harness is being added to, as the store holds it now: it must
/// exist and be open.
pub(super) fn open_room(store: &RoomStore, room_id: &str) -> VerbResult<Room> {
    let room = current_room(store, room_id)
        .ok_or_else(|| VerbError::NotFound(format!("not_found: room \"{room_id}\" not found")))?;
    if room.archived.is_some() {
        return Err(VerbError::Refused(format!(
            "archived: room \"{room_id}\" is already archived"
        )));
    }
    Ok(room)
}

/// The tab label of a kind, as `HARNESS_KINDS[kind].label` in `data.tsx`.
fn kind_label(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Claude => "CC",
        HarnessKind::Opencode => "oc",
        HarnessKind::Copilot => "gh",
        HarnessKind::Hermes => "hm",
        HarnessKind::Byoh => "sh",
        HarnessKind::Remote => "ssh",
        HarnessKind::Files => "files",
        HarnessKind::Design => "design",
    }
}

/// `<label>-<n>`, n counting the harnesses the room has now plus one.
pub(super) fn harness_display_name(kind: HarnessKind, existing: usize) -> String {
    format!("{}-{}", kind_label(kind), existing + 1)
}

/// `prefix` and five base36 characters, like the webview's `newId(prefix)`.
fn random_id_with(prefix: &str) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let mut id = String::from(prefix);
    for b in bytes.iter().take(5) {
        id.push(char::from(DIGITS[usize::from(*b) % DIGITS.len()]));
    }
    id
}

/// `h` and five base36 characters.
fn random_id() -> String {
    random_id_with("h")
}

/// A room id (`s` and five base36 characters) not in `taken`.
pub(super) fn mint_room_id(taken: &HashSet<String>) -> String {
    loop {
        let id = random_id_with("s");
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// An id no harness in the store has.
fn mint_id(store: &RoomStore) -> String {
    let taken: HashSet<String> = store
        .rooms()
        .iter()
        .flat_map(|r| r.harnesses.iter().map(|h| h.id.clone()))
        .collect();
    loop {
        let id = random_id();
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// What a new harness record is made of.
pub(super) struct NewHarness {
    pub(super) kind: HarnessKind,
    pub(super) agent: Option<String>,
    pub(super) name: String,
    /// The room's folder.
    pub(super) cwd: Option<String>,
    pub(super) created_by: Option<HarnessCreatedBy>,
}

/// The record, from `fresh` (the argv decisions, pty kinds only).
pub(super) fn assemble(id: String, spec: NewHarness, fresh: Option<FreshCmdOutput>) -> Harness {
    let pty = spec.kind.has_pty();
    Harness {
        id,
        kind: spec.kind.as_str().to_owned(),
        name: spec.name,
        status: if pty { "running" } else { "idle" }.to_owned(),
        model: match (pty, spec.kind) {
            (false, _) => "",
            (true, HarnessKind::Copilot) => "gpt-5",
            (true, _) => "sonnet-4.5",
        }
        .to_owned(),
        tokens: "0".to_owned(),
        live: pty.then_some(true),
        cmd: fresh.as_ref().map(|f| f.cmd.clone()),
        cwd: spec.cwd,
        session_id: fresh.as_ref().and_then(|f| f.session_id.clone()),
        agent: spec.agent.filter(|a| !a.trim().is_empty()),
        design_entry: None,
        design_device: None,
        pending_notifications: None,
        created_by: spec.created_by,
        shell_claim: None,
        remote: None,
        env_profile: None,
        hermes_profile: fresh.and_then(|f| f.hermes_profile),
        opencode_version: None,
        launch: None,
    }
}

/// A new harness record with a fresh id: the argv, a Claude conversation id
/// and the Hermes profile come from the daemon's `harness_fresh_cmd`
/// (probing the default shell, so off the executor). The agent is dropped
/// for a kind that takes none.
pub(super) async fn build_harness(
    state: &AgentApiState,
    store: &RoomStore,
    mut spec: NewHarness,
) -> VerbResult<Harness> {
    if !spec.kind.takes_agent() {
        spec.agent = None;
    }
    let fresh = if spec.kind.has_pty() {
        let input = FreshCmdInput {
            kind: spec.kind,
            session_id: None,
            new_harness: true,
            agent: spec.agent.clone().filter(|a| !a.trim().is_empty()),
            remote: None,
            hermes_profile: None,
        };
        let env = state.spawn_env.clone();
        let out = tokio::task::spawn_blocking(move || match env {
            Some(env) => fresh_cmd(&env, &input),
            // Only a unit test has no spawn environment to ask for the shell.
            None => fresh_cmd_with(&input, &[FALLBACK_SHELL.to_owned()], || {
                uuid::Uuid::new_v4().to_string()
            }),
        })
        .await
        .map_err(|e| internal(format!("building the argv panicked: {e}")))?;
        Some(out)
    } else {
        None
    };
    Ok(assemble(mint_id(store), spec, fresh))
}

/// Put `harness` into `room_id` and return the stored record. The room's
/// active harness is not changed: a verb never takes over a screen.
///
/// The final step, under the room-verb lock: the room is re-read, must still
/// exist and be open, must hold fewer than `max_harnesses`, and the harness
/// is named from the fresh count so two concurrent opens never share a name.
pub(super) async fn store_harness(
    state: &AgentApiState,
    room_id: &str,
    mut harness: Harness,
    max_harnesses: usize,
) -> VerbResult<Harness> {
    let (_guard, store) = lock_verbs(state).await?;
    let room = open_room(store, room_id)?;
    if room.harnesses.len() >= max_harnesses {
        return Err(VerbError::Refused(format!(
            "room_full: {} already has {} harnesses (cap {max_harnesses}); \
             close one before opening another",
            room.name,
            room.harnesses.len()
        )));
    }
    if let Some(kind) = HarnessKind::from_name(&harness.kind) {
        // Skip a name a surviving harness already has (one was closed).
        let mut existing = room.harnesses.len();
        while room
            .harnesses
            .iter()
            .any(|h| h.name == harness_display_name(kind, existing))
        {
            existing += 1;
        }
        harness.name = harness_display_name(kind, existing);
    }
    execute_all(
        store,
        vec![RoomCommand::AddHarness {
            room_id: room_id.to_owned(),
            harness: harness.clone(),
            activate: false,
        }],
        "the new harness",
    )?;
    Ok(harness)
}

/// Start a new harness here when no client is attached: a client's view mounts
/// it and spawns at the real terminal size, and without one nobody would.
/// A failed spawn does not undo the record; the harness reads as exited, as a
/// failed spawn from a view does.
pub(super) async fn spawn_without_client(state: &AgentApiState, room_id: &str, harness: &Harness) {
    if state.has_client()
        || !HarnessKind::from_name(&harness.kind).is_some_and(HarnessKind::has_pty)
    {
        return;
    }
    let Some(runner) = state.runner().map(Arc::clone) else {
        return;
    };
    let req = EnsureRequest {
        room_id: room_id.to_owned(),
        harness_id: harness.id.clone(),
        rows: 0,
        cols: 0,
        env_profile: None,
        restart: false,
        expect_cmd: None,
        expect_launch: None,
        run_shell: None,
    };
    match tokio::task::spawn_blocking(move || runner.ensure_running(&req, None)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(
            harness = %harness.id,
            code = e.code(),
            error = ?e,
            "agent_api: spawning the new harness failed"
        ),
        Err(e) => tracing::warn!(harness = %harness.id, error = %e, "agent_api: spawn panicked"),
    }
}

#[cfg(test)]
mod tests;
