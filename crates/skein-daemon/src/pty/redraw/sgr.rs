//! Style and link encoding: SGR and OSC 8.
//!
//! The client's pen is tracked in [`Pen`] so a run of cells in one style
//! costs one sequence. Every SGR sequence starts from a reset (`0;`), so no
//! sequence depends on what the client had before it.

use std::io::Write;

use crate::pty::engine::dump::{CellDump, ColorDump};

/// The part of a cell that SGR carries.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent SGR flags
pub(super) struct Style {
    fg: ColorDump,
    bg: ColorDump,
    bold: bool,
    italic: bool,
    dim: bool,
    underline: bool,
    inverse: bool,
    invisible: bool,
    strikethrough: bool,
}

impl Style {
    pub(super) fn default_style() -> Self {
        Self::of(&CellDump::blank())
    }

    pub(super) fn of(c: &CellDump) -> Self {
        Self {
            fg: c.fg,
            bg: c.bg,
            bold: c.bold,
            italic: c.italic,
            dim: c.dim,
            underline: c.underline,
            inverse: c.inverse,
            invisible: c.invisible,
            strikethrough: c.strikethrough,
        }
    }
}

/// `ESC [ 0 ; ... m`, or `ESC [ 0 m` for the default style.
fn write_sgr(out: &mut Vec<u8>, s: &Style) {
    out.extend_from_slice(b"\x1b[0");
    for (on, code) in [
        (s.bold, 1),
        (s.dim, 2),
        (s.italic, 3),
        (s.underline, 4),
        (s.inverse, 7),
        (s.invisible, 8),
        (s.strikethrough, 9),
    ] {
        if on {
            let _ = write!(out, ";{code}");
        }
    }
    write_color(out, s.fg, 30, 90, 38);
    write_color(out, s.bg, 40, 100, 48);
    out.push(b'm');
}

/// `base` is the first of the eight normal colours, `bright` of the eight
/// bright ones, `ext` the extended-colour introducer (38 or 48).
fn write_color(out: &mut Vec<u8>, c: ColorDump, base: u8, bright: u8, ext: u8) {
    let _ = match c {
        ColorDump::Default => return,
        ColorDump::Palette { index } if index < 8 => write!(out, ";{}", base + index),
        ColorDump::Palette { index } if index < 16 => write!(out, ";{}", bright + index - 8),
        ColorDump::Palette { index } => write!(out, ";{ext};5;{index}"),
        ColorDump::Rgb { r, g, b } => write!(out, ";{ext};2;{r};{g};{b}"),
    };
}

/// The link a cell may carry into an OSC 8: a URI with a control character
/// in it would end the sequence early, so such a link is dropped.
pub(super) fn link_of(c: &CellDump) -> Option<&str> {
    c.link
        .as_deref()
        .filter(|u| !u.is_empty() && !u.chars().any(char::is_control))
}

/// What the client's pen is set to: the SGR state and the open link.
pub(super) struct Pen {
    style: Style,
    link: Option<String>,
}

impl Pen {
    /// A pen at the reset state. A frame opens with a reset, so this is what
    /// the client's pen is at the start of every frame.
    pub(super) fn new() -> Self {
        Self {
            style: Style::default_style(),
            link: None,
        }
    }

    pub(super) fn set_style(&mut self, out: &mut Vec<u8>, want: &Style) {
        if self.style != *want {
            write_sgr(out, want);
            self.style = want.clone();
        }
    }

    /// Opens, switches or closes the OSC 8 link. The dump has no link id, so
    /// none is sent.
    pub(super) fn set_link(&mut self, out: &mut Vec<u8>, want: Option<&str>) {
        if self.link.as_deref() == want {
            return;
        }
        match want {
            Some(uri) => {
                out.extend_from_slice(b"\x1b]8;;");
                out.extend_from_slice(uri.as_bytes());
                out.extend_from_slice(b"\x1b\\");
            }
            None => out.extend_from_slice(b"\x1b]8;;\x1b\\"),
        }
        self.link = want.map(str::to_owned);
    }

    /// Ends the frame's pen state: default style, no open link.
    pub(super) fn reset(&mut self, out: &mut Vec<u8>) {
        self.set_link(out, None);
        self.set_style(out, &Style::default_style());
    }
}
