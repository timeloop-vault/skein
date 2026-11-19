//! Which terminal path a daemon runs (design T4).

use serde::Serialize;

/// Which terminal path a daemon runs (design T4). Read once at daemon start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalMode {
    /// Raw PTY bytes to the viewer's own xterm.js: the path that ships today.
    #[default]
    Raw,
    /// The daemon's engine cuts redraw frames and viewers render those.
    Frames,
}

impl TerminalMode {
    /// The environment variable that selects the mode.
    pub const ENV: &'static str = "SKEIN_TERMINAL";

    /// `Some("frames")` (trimmed, ASCII case-insensitive) is `Frames`;
    /// anything else, unset included, is `Raw`.
    #[must_use]
    pub fn from_value(v: Option<&str>) -> Self {
        match v {
            Some(s) if s.trim().eq_ignore_ascii_case("frames") => Self::Frames,
            _ => Self::Raw,
        }
    }

    /// Reads `SKEIN_TERMINAL` from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        #[cfg(test)]
        if let Some(mode) = FORCED.with(std::cell::Cell::get) {
            return mode;
        }
        Self::from_value(std::env::var(Self::ENV).ok().as_deref())
    }

    /// Test-only: what `from_env` answers on this thread from now on, so a
    /// daemon built by the test runs in `mode` without touching the process
    /// environment (which other tests share).
    #[cfg(test)]
    pub(crate) fn force_on_this_thread(mode: Self) {
        FORCED.with(|f| f.set(Some(mode)));
    }
}

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::Cell<Option<TerminalMode>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::TerminalMode;

    #[test]
    fn frames_is_the_only_value_that_turns_frames_on() {
        assert_eq!(
            TerminalMode::from_value(Some("frames")),
            TerminalMode::Frames
        );
        assert_eq!(
            TerminalMode::from_value(Some("  FrAmEs\n")),
            TerminalMode::Frames
        );
        for other in ["", "raw", "frame", "1", "true", "frames!"] {
            assert_eq!(TerminalMode::from_value(Some(other)), TerminalMode::Raw);
        }
        assert_eq!(TerminalMode::from_value(None), TerminalMode::Raw);
        assert_eq!(TerminalMode::default(), TerminalMode::Raw);
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&TerminalMode::Frames).unwrap(),
            "\"frames\""
        );
        assert_eq!(
            serde_json::to_string(&TerminalMode::Raw).unwrap(),
            "\"raw\""
        );
    }
}
