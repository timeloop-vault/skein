//! Tracks whether a PTY's terminal application has bracketed paste on
//! (DEC private mode 2004), from the output stream alone. The frontend
//! reads the same fact off xterm.js; the daemon has no terminal emulator,
//! so this small parser stands in for it.

/// Longest CSI parameter string kept; anything longer is no sequence we
/// care about and is dropped when its final byte arrives.
const MAX_PARAMS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    /// Saw ESC.
    Escape,
    /// Saw ESC `[`; collecting parameter and intermediate bytes.
    Csi { params: String, overflow: bool },
}

/// A state machine fed PTY output chunks. A sequence split across chunks,
/// at any byte, is handled; RIS (`ESC c`) and DECSTR (`ESC [ ! p`) reset
/// the mode to off, as a terminal would.
#[derive(Debug, Clone, Default)]
pub struct PasteModeTracker {
    on: bool,
    state: State,
}

impl PasteModeTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether bracketed paste is currently on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Consume the next chunk of PTY output.
    pub fn feed(&mut self, chunk: &str) {
        for c in chunk.chars() {
            self.step(c);
        }
    }

    fn step(&mut self, c: char) {
        let state = std::mem::take(&mut self.state);
        self.state = match state {
            State::Ground => {
                if c == '\u{1b}' {
                    State::Escape
                } else {
                    State::Ground
                }
            }
            State::Escape => match c {
                '[' => State::Csi {
                    params: String::new(),
                    overflow: false,
                },
                'c' => {
                    self.on = false;
                    State::Ground
                }
                '\u{1b}' => State::Escape,
                _ => State::Ground,
            },
            State::Csi {
                mut params,
                mut overflow,
            } => match c {
                // Parameter and intermediate bytes.
                '\u{20}'..='\u{3f}' => {
                    if params.len() < MAX_PARAMS {
                        params.push(c);
                    } else {
                        overflow = true;
                    }
                    State::Csi { params, overflow }
                }
                // Final byte.
                '\u{40}'..='\u{7e}' => {
                    if !overflow {
                        self.finish_csi(&params, c);
                    }
                    State::Ground
                }
                // A new escape aborts this sequence and starts another.
                '\u{1b}' => State::Escape,
                // Other C0 controls are executed in place by a terminal.
                '\u{00}'..='\u{1f}' => State::Csi { params, overflow },
                _ => State::Ground,
            },
        };
    }

    fn finish_csi(&mut self, params: &str, final_byte: char) {
        match (params.strip_prefix('?'), final_byte) {
            (Some(list), 'h' | 'l') => {
                if list.split(';').any(|p| p == "2004") {
                    self.on = final_byte == 'h';
                }
            }
            (None, 'p') if params == "!" => self.on = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> bool {
        let mut t = PasteModeTracker::new();
        for c in chunks {
            t.feed(c);
        }
        t.is_on()
    }

    #[test]
    fn starts_off() {
        assert!(!PasteModeTracker::new().is_on());
    }

    #[test]
    fn table() {
        let cases: [(&str, &[&str], bool); 14] = [
            ("on", &["\x1b[?2004h"], true),
            ("on then off", &["\x1b[?2004h", "\x1b[?2004l"], false),
            ("combined on", &["\x1b[?1004;2004h"], true),
            ("combined on, 2004 first", &["\x1b[?2004;1004h"], true),
            ("combined off", &["\x1b[?2004h\x1b[?1004;2004l"], false),
            ("other mode ignored", &["\x1b[?1004h"], false),
            (
                "other mode leaves on alone",
                &["\x1b[?2004h\x1b[?1004l"],
                true,
            ),
            ("2004x is not 2004", &["\x1b[?20041h"], false),
            ("text around", &["hello \x1b[?2004h world"], true),
            ("RIS resets", &["\x1b[?2004h", "\x1bc"], false),
            ("DECSTR resets", &["\x1b[?2004h", "\x1b[!p"], false),
            ("non-private h is ignored", &["\x1b[2004h"], false),
            ("aborted sequence", &["\x1b[?20\x1b[?2004h"], true),
            (
                "colour codes pass through",
                &["\x1b[31m\x1b[?2004h\x1b[0m"],
                true,
            ),
        ];
        for (name, chunks, want) in cases {
            assert_eq!(run(chunks), want, "{name}");
        }
    }

    #[test]
    fn every_split_point_of_on_and_off() {
        let on = "\x1b[?1004;2004h";
        let off = "\x1b[?2004l";
        for i in 0..=on.len() {
            let (head, tail) = on.split_at(i);
            assert!(run(&[head, tail]), "on split at {i}");
            let mut tracker = PasteModeTracker::new();
            tracker.feed("\x1b[?2004h");
            let (head, tail) = off.split_at(i.min(off.len()));
            tracker.feed(head);
            tracker.feed(tail);
            assert!(!tracker.is_on(), "off split at {i}");
        }
    }

    #[test]
    fn byte_at_a_time() {
        let mut t = PasteModeTracker::new();
        for c in "\x1b[?2004h".chars() {
            t.feed(&c.to_string());
        }
        assert!(t.is_on());
        for c in "\x1b[!p".chars() {
            t.feed(&c.to_string());
        }
        assert!(!t.is_on());
    }

    #[test]
    fn overlong_params_are_ignored() {
        let long = format!("\x1b[?{}2004h", "1;".repeat(100));
        assert!(!run(&[&long]));
    }
}
