//! What the webview still tells the daemon about a harness (#732): the
//! daemon's phase machine is the authority and the webview mirrors its
//! stream.
//!
//! These are the inputs only the client can observe: keystrokes in the
//! xterm, the rendered Hermes screen, a seam-submitted prompt. The daemon
//! sees the PTY bytes and the adapter events itself. One tagged enum keeps
//! the wire to a single Tauri command and the mapping in one `match`.

use serde::Deserialize;

use super::{ScreenReading, SessionSwitch};

/// One client-observed input for a harness. The strings of `kind` and the
/// camelCase field names are the wire contract with the webview.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientPhaseInput {
    /// A keystroke or paste written to the PTY (`recordInput`).
    Input { data: String, empty_composer: bool },
    /// A prompt submitted through the #238 seam (`notePromptSubmitted`).
    PromptSubmitted,
    /// The client is about to cause an induced redraw (`muteInducedOutput`).
    MuteInducedOutput,
    /// The client read the rendered screen (`setScreenReading`, #623);
    /// `None` clears it.
    ScreenReading { reading: Option<ScreenReading> },
    /// One 500 ms Hermes launch poll (#625); `screen_waiting` is whether the
    /// screen read `waiting`.
    HermesLaunchPoll { screen_waiting: bool },
    /// The CLI reported its own launch (`noteLaunchSignal` with its default
    /// source).
    LaunchSignal,
    /// The tail was re-pointed onto a different session (#116, #539).
    SessionSwitched { switch: SessionSwitch },
    /// A mail nudge's lost first paste (#404).
    RecoverUnheardSilence,
    /// A start that produced no process (a refused agent, a failed ensure
    /// call): the record is created or replaced and reads `exited`. Only
    /// acted on for a stored harness with no live process, and under the
    /// stored room id (`HarnessRunner::client_phase_input`); wire:
    /// `{"kind":"startFailed"}`.
    StartFailed,
    /// The user (or the version auto-restart) asked for a respawn. Writes one
    /// `harness_events` row (`<current phase> -> spawning`) and changes
    /// nothing else: the new process's own spawn does the rest. Only acted on
    /// for a stored harness, under the stored room id; wire:
    /// `{"kind":"restartRequested","source":"user-restart"}`.
    RestartRequested { source: RestartSource },
}

/// Who asked for a restart: the `source` of the logged row. These are the two
/// [`TransitionSource`](super::types::TransitionSource) strings of the same
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum RestartSource {
    #[serde(rename = "user-restart")]
    UserRestart,
    #[serde(rename = "version-auto-restart")]
    VersionAutoRestart,
}

impl RestartSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserRestart => "user-restart",
            Self::VersionAutoRestart => "version-auto-restart",
        }
    }
}
