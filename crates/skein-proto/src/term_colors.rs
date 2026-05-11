//! Map `alacritty_terminal`'s color enum to floem `Color`. Uses the
//! standard VS Code dark+ palette for named ANSI colors, and the
//! xterm 256-color formula for indexed values.

use alacritty_terminal::vte::ansi::{Color as AlacColor, NamedColor, Rgb};
use floem::peniko::Color;

pub const DEFAULT_FG: Color = Color::rgb8(0xd4, 0xd4, 0xd4);
pub const DEFAULT_BG: Color = Color::rgb8(0x0e, 0x0e, 0x12);
pub const DEFAULT_CURSOR: Color = Color::rgb8(0xff, 0xc0, 0x4d);

// VS Code dark+ ANSI palette, picked because it's the closest thing
// to "default expectations" most users have.
const PALETTE_16: [Color; 16] = [
    Color::rgb8(0x00, 0x00, 0x00), // 0  Black
    Color::rgb8(0xcd, 0x31, 0x31), // 1  Red
    Color::rgb8(0x0d, 0xbc, 0x79), // 2  Green
    Color::rgb8(0xe5, 0xe5, 0x10), // 3  Yellow
    Color::rgb8(0x24, 0x72, 0xc8), // 4  Blue
    Color::rgb8(0xbc, 0x3f, 0xbc), // 5  Magenta
    Color::rgb8(0x11, 0xa8, 0xcd), // 6  Cyan
    Color::rgb8(0xe5, 0xe5, 0xe5), // 7  White
    Color::rgb8(0x66, 0x66, 0x66), // 8  BrightBlack
    Color::rgb8(0xf1, 0x4c, 0x4c), // 9  BrightRed
    Color::rgb8(0x23, 0xd1, 0x8b), // 10 BrightGreen
    Color::rgb8(0xf5, 0xf5, 0x43), // 11 BrightYellow
    Color::rgb8(0x3b, 0x8e, 0xea), // 12 BrightBlue
    Color::rgb8(0xd6, 0x70, 0xd6), // 13 BrightMagenta
    Color::rgb8(0x29, 0xb8, 0xdb), // 14 BrightCyan
    Color::rgb8(0xff, 0xff, 0xff), // 15 BrightWhite
];

/// xterm 256-color formula. Index 0-15 is the 16-color palette,
/// 16-231 is a 6×6×6 RGB cube (step values 0, 95, 135, 175, 215, 255),
/// 232-255 is a 24-step grayscale ramp.
fn indexed_color(index: u8) -> Color {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    if (index as usize) < PALETTE_16.len() {
        return PALETTE_16[index as usize];
    }
    if index >= 232 {
        // Grayscale ramp: 8 + 10*(index - 232), saturating at 238.
        let lvl = 8u16 + 10u16 * u16::from(index - 232);
        #[allow(clippy::cast_possible_truncation)]
        let l = lvl.min(255) as u8;
        return Color::rgb8(l, l, l);
    }
    // 6×6×6 cube.
    let i = index - 16;
    let r = i / 36;
    let g = (i / 6) % 6;
    let b = i % 6;
    Color::rgb8(STEPS[r as usize], STEPS[g as usize], STEPS[b as usize])
}

fn rgb_to_color(rgb: Rgb) -> Color {
    Color::rgb8(rgb.r, rgb.g, rgb.b)
}

/// Convert an alacritty cell color into a floem `Color`. Three
/// kinds: named (16 ANSI + foreground/background/cursor + dim
/// variants), indexed (xterm 256), or spec (truecolor).
pub fn term_color(color: AlacColor) -> Color {
    match color {
        AlacColor::Named(named) => named_color(named),
        AlacColor::Spec(rgb) => rgb_to_color(rgb),
        AlacColor::Indexed(idx) => indexed_color(idx),
    }
}

fn named_color(color: NamedColor) -> Color {
    use NamedColor::*;
    match color {
        Black | DimBlack => PALETTE_16[0],
        Red | DimRed => PALETTE_16[1],
        Green | DimGreen => PALETTE_16[2],
        Yellow | DimYellow => PALETTE_16[3],
        Blue | DimBlue => PALETTE_16[4],
        Magenta | DimMagenta => PALETTE_16[5],
        Cyan | DimCyan => PALETTE_16[6],
        White | DimWhite => PALETTE_16[7],
        BrightBlack => PALETTE_16[8],
        BrightRed => PALETTE_16[9],
        BrightGreen => PALETTE_16[10],
        BrightYellow => PALETTE_16[11],
        BrightBlue => PALETTE_16[12],
        BrightMagenta => PALETTE_16[13],
        BrightCyan => PALETTE_16[14],
        BrightWhite => PALETTE_16[15],
        Foreground | DimForeground | BrightForeground => DEFAULT_FG,
        Background => DEFAULT_BG,
        Cursor => DEFAULT_CURSOR,
    }
}
