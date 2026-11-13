//! What the webview still tells the daemon about a harness while the phase
//! machine runs in shadow (#732).
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
    /// A view attached to a process that was already running (#731).
    Reattached,
    /// The webview threw away and rebuilt its own record for a harness that
    /// keeps running (reload, view remount). Touches no machine state; the
    /// shadow comparator resyncs.
    ViewReset,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_is_camel_case_tagged() {
        let i: ClientPhaseInput =
            serde_json::from_str(r#"{"kind":"input","data":"a","emptyComposer":true}"#).unwrap();
        assert!(matches!(
            i,
            ClientPhaseInput::Input {
                empty_composer: true,
                ..
            }
        ));
        let i: ClientPhaseInput =
            serde_json::from_str(r#"{"kind":"sessionSwitched","switch":"fork"}"#).unwrap();
        assert!(matches!(
            i,
            ClientPhaseInput::SessionSwitched {
                switch: SessionSwitch::Fork
            }
        ));
        let i: ClientPhaseInput =
            serde_json::from_str(r#"{"kind":"screenReading","reading":null}"#).unwrap();
        assert!(matches!(
            i,
            ClientPhaseInput::ScreenReading { reading: None }
        ));
        let i: ClientPhaseInput =
            serde_json::from_str(r#"{"kind":"hermesLaunchPoll","screenWaiting":true}"#).unwrap();
        assert!(matches!(
            i,
            ClientPhaseInput::HermesLaunchPoll {
                screen_waiting: true
            }
        ));
    }

    #[test]
    fn every_frontend_shape_deserializes() {
        let parse = |s: &str| serde_json::from_str::<ClientPhaseInput>(s).unwrap();
        assert!(matches!(
            parse(r#"{"kind":"promptSubmitted"}"#),
            ClientPhaseInput::PromptSubmitted
        ));
        assert!(matches!(
            parse(r#"{"kind":"muteInducedOutput"}"#),
            ClientPhaseInput::MuteInducedOutput
        ));
        for (s, want) in [
            ("waiting", ScreenReading::Waiting),
            ("running", ScreenReading::Running),
            ("unknown", ScreenReading::Unknown),
        ] {
            let i = parse(&format!(r#"{{"kind":"screenReading","reading":"{s}"}}"#));
            assert!(matches!(i, ClientPhaseInput::ScreenReading { reading: Some(r) } if r == want));
        }
        assert!(matches!(
            parse(r#"{"kind":"launchSignal"}"#),
            ClientPhaseInput::LaunchSignal
        ));
        for (s, want) in [
            ("clear", SessionSwitch::Clear),
            ("resume", SessionSwitch::Resume),
            ("fork", SessionSwitch::Fork),
            ("startup", SessionSwitch::Startup),
        ] {
            let i = parse(&format!(r#"{{"kind":"sessionSwitched","switch":"{s}"}}"#));
            assert!(matches!(i, ClientPhaseInput::SessionSwitched { switch } if switch == want));
        }
        assert!(matches!(
            parse(r#"{"kind":"recoverUnheardSilence"}"#),
            ClientPhaseInput::RecoverUnheardSilence
        ));
        assert!(matches!(
            parse(r#"{"kind":"reattached"}"#),
            ClientPhaseInput::Reattached
        ));
        assert!(matches!(
            parse(r#"{"kind":"viewReset"}"#),
            ClientPhaseInput::ViewReset
        ));
    }
}
