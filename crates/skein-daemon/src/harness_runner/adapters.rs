//! The L2c adapters the daemon attaches itself (#731), a port of
//! `app/src/terminalSpawnAdapters.ts`. The adapters write the
//! `harness_actions` rows on their own; their event consumer is the phase
//! service (#732), which runs in shadow beside the client's machine. The
//! client attaches its own event channel through the existing commands and
//! replaces this one (see `HarnessRunner::claude_phase_sink`).
//!
//! `claude_attached` and friends come BEFORE the attach: the translator drops
//! events until it is attached, and the first attach replays the transcript.

use std::sync::Arc;

use crate::harness_actions_hermes::HermesActionsManager;
use crate::harness_events_claude::{ClaudeEventsManager, claude_events_attach_impl};
use crate::harness_events_opencode::{
    OpencodeAttachArgs, OpencodeEventsManager, opencode_events_attach_impl,
};
use crate::harness_kind::HarnessKind;
use crate::harness_phase::PhaseService;
use crate::harness_phase::sinks::{ClaudePhaseSink, OpencodePhaseSink};
use crate::opencode_sidecar::OpencodeSidecarManager;
use crate::spawn_env::state::SpawnEnvState;

/// What an attach needs to know about one running harness.
pub(super) struct AttachSpec<'a> {
    pub harness_id: &'a str,
    pub room_id: &'a str,
    pub kind: HarnessKind,
    pub cwd: &'a str,
    /// The harness record's session id, as it stands now.
    pub session_id: Option<&'a str>,
    /// v1 opencode: the `--port` of the stored argv. Unused with a sidecar.
    pub opencode_port: Option<u16>,
    /// v2 opencode: the sidecar generation the PTY is bound to.
    pub sidecar_generation: Option<u64>,
    /// argv[0] actually spawned (Hermes is only tailed when it is hermes).
    pub program: Option<&'a str>,
    pub hermes_profile: Option<&'a str>,
    /// The spawn reported working Hermes status hooks (#625).
    pub hermes_hooks: bool,
    /// The attach right after a spawn (#336); false for a re-attach to a
    /// process that has lived on.
    pub fresh_process: bool,
}

/// The three adapter managers, shared with the managed state.
pub(super) struct Adapters {
    pub claude: ClaudeEventsManager,
    pub opencode: OpencodeEventsManager,
    pub hermes: HermesActionsManager,
    pub sidecars: OpencodeSidecarManager,
    pub spawn_env: Arc<SpawnEnvState>,
    pub phase: Arc<PhaseService>,
}

/// `isHermesProgram`: the file stem of argv[0], case-insensitive.
pub(super) fn is_hermes_program(program: Option<&str>) -> bool {
    let Some(program) = program else {
        return false;
    };
    let base = program.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = base.rfind('.').map_or(base, |i| &base[..i]);
    stem.eq_ignore_ascii_case("hermes")
}

impl Adapters {
    /// Attach the adapter that fits the kind, under the conditions the client
    /// used. A failure is logged and never fails the caller. Blocking: the
    /// Claude attach reads the whole transcript.
    pub(super) fn attach(&self, spec: &AttachSpec<'_>) {
        match spec.kind {
            HarnessKind::Claude => self.attach_claude(spec),
            HarnessKind::Opencode => self.attach_opencode(spec),
            HarnessKind::Hermes => self.attach_hermes(spec),
            _ => {}
        }
    }

    /// Detach all three. Idempotent. The phase service is not told: a stop
    /// follows with `forget`, and the restart path replaces the record.
    pub(super) fn detach(&self, harness_id: &str) {
        self.claude.detach(harness_id);
        self.opencode.detach(harness_id);
        self.hermes.detach(harness_id);
    }

    /// [`detach`](Self::detach) for good while the harness stays known (the
    /// process exited): the translators are told their source is gone.
    pub(super) fn detach_for_good(&self, harness_id: &str) {
        self.detach(harness_id);
        self.phase.claude_detached(harness_id);
        self.phase.opencode_detached(harness_id);
        self.phase.hermes_detached(harness_id);
    }

    /// `hasClaudeTranscriptTail`: a Claude harness that owns a session id.
    fn attach_claude(&self, spec: &AttachSpec<'_>) {
        let Some(session_id) = spec.session_id.filter(|s| !s.is_empty()) else {
            return;
        };
        self.phase.claude_attached(spec.harness_id);
        if let Err(e) = claude_events_attach_impl(
            &self.claude,
            spec.harness_id.to_owned(),
            spec.room_id.to_owned(),
            session_id,
            spec.cwd,
            spec.fresh_process,
            ClaudePhaseSink {
                service: Arc::clone(&self.phase),
                harness_id: spec.harness_id.to_owned(),
            },
        ) {
            tracing::warn!(harness = spec.harness_id, error = %e, "harness runner: claude attach failed");
            self.phase.claude_detached(spec.harness_id);
        }
    }

    /// With a port (v1) or a sidecar generation (v2); without either there is
    /// nowhere to subscribe.
    fn attach_opencode(&self, spec: &AttachSpec<'_>) {
        if spec.opencode_port.is_none() && spec.sidecar_generation.is_none() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                harness = spec.harness_id,
                "harness runner: no async runtime for the opencode adapter"
            );
            return;
        };
        let _entered = runtime.enter();
        let args = OpencodeAttachArgs {
            harness_id: spec.harness_id.to_owned(),
            room_id: spec.room_id.to_owned(),
            cwd: spec.cwd.to_owned(),
            port: spec.opencode_port.unwrap_or(0),
            session_id: spec.session_id.map(str::to_owned),
            sidecar_generation: spec.sidecar_generation,
        };
        self.phase.opencode_attached(spec.harness_id);
        let sink = OpencodePhaseSink {
            service: Arc::clone(&self.phase),
            harness_id: spec.harness_id.to_owned(),
        };
        if let Err(e) = opencode_events_attach_impl(&self.opencode, &self.sidecars, args, sink) {
            tracing::warn!(harness = spec.harness_id, error = %e, "harness runner: opencode attach failed");
            self.phase.opencode_detached(spec.harness_id);
        }
    }

    /// The state.db tail, only when the status hooks work and a session is
    /// already bound. An unbound session is captured later by the client
    /// (`startHermesCapture`), which then attaches the tail itself.
    fn attach_hermes(&self, spec: &AttachSpec<'_>) {
        if !spec.hermes_hooks || !is_hermes_program(spec.program) {
            return;
        }
        let session_id = spec.session_id.filter(|s| !s.is_empty());
        // The hook-fed binding exists with or without a bound session
        // (`attachHermesEvents`); only the state.db tail needs one.
        self.phase
            .hermes_attached(spec.harness_id, session_id.map(str::to_owned));
        let Some(session_id) = session_id else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                harness = spec.harness_id,
                "harness runner: no async runtime for the hermes adapter"
            );
            return;
        };
        let settings = self.spawn_env.snapshot();
        let Some(state_db) = crate::resume::profile_db(&settings, spec.hermes_profile) else {
            tracing::warn!(
                harness = spec.harness_id,
                "harness runner: no Hermes home or invalid profile"
            );
            return;
        };
        // A session known at spawn was resumed: its prior spend is not this
        // turn's (`resumed`).
        let attached = runtime.block_on(self.hermes.attach(
            spec.harness_id.to_owned(),
            spec.room_id.to_owned(),
            session_id.to_owned(),
            state_db,
            true,
        ));
        if let Err(e) = attached {
            tracing::warn!(harness = spec.harness_id, error = %e, "harness runner: hermes attach failed");
        }
    }
}
