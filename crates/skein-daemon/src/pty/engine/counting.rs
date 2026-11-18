//! A `Handler` that forwards every call to the `Term` and watches its
//! history (#793).
//!
//! `alacritty_terminal` reports no scroll event, and once history is full its
//! size stops changing, so rows entering history cannot be counted from
//! outside. This wrapper counts them where they happen. The `Term` is given
//! `HISTORY_ROWS + OVERFLOW_ROWS` of room, and after every handler call
//! (one escape sequence or one character) the history is cut back to
//! `HISTORY_ROWS`, oldest rows first. One call can scroll at most one screen
//! height, which fits in `u16`, so the overflow room is never exhausted
//! within a call and the growth of the history size across a call is exactly
//! the number of rows that entered it.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::cursor_icon::CursorIcon;
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb,
    ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};

use super::HISTORY_ROWS;
use super::alacritty::Listener;

/// Room above [`HISTORY_ROWS`] the `Term` is configured with, so a single
/// handler call can never overflow it (a screen is at most `u16::MAX` rows).
pub(super) const OVERFLOW_ROWS: usize = 70_000;

/// Cuts the active grid's history back to [`HISTORY_ROWS`]. Dropping the
/// oldest rows is O(1) in the grid's storage.
pub(super) fn trim_history(term: &mut Term<Listener>) {
    if term.history_size() > HISTORY_ROWS {
        let grid = term.grid_mut();
        grid.update_history(HISTORY_ROWS);
        grid.update_history(HISTORY_ROWS + OVERFLOW_ROWS);
    }
}

pub(super) struct Counting<'a> {
    pub(super) term: &'a mut Term<Listener>,
    pub(super) entered: &'a mut u64,
}

impl Counting<'_> {
    fn before(&self) -> (usize, bool) {
        (
            self.term.history_size(),
            self.term.mode().contains(TermMode::ALT_SCREEN),
        )
    }

    fn after(&mut self, (before, alt_before): (usize, bool)) {
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
        let after = self.term.history_size();
        trim_history(self.term);
        // Switching screens swaps which grid `history_size` describes (the
        // alternate grid keeps none), which is not a scroll.
        if alt != alt_before {
            return;
        }
        // A call that scrolls never also shrinks the history; one that
        // shrinks it (`ESC[3J`, a reset) never scrolls.
        if after > before {
            *self.entered += (after - before) as u64;
        }
    }
}

/// Forwards each listed `Handler` method to the `Term`, watching history, and
/// records the names in [`FORWARDED`].
///
/// `Handler` gives every method a default that does nothing, so a method
/// missing from this list is silently dropped, not a compile error. THE LIST
/// MUST BE RE-DIFFED against `vte::ansi::Handler` on any `vte` or
/// `alacritty_terminal` bump. It was checked against vte 0.15.0
/// (`alacritty_terminal` 0.26.0): 71 of 71 methods. `tests_forward.rs` pins
/// that version and count and drives every method through the wrapper.
macro_rules! forward {
    ($(fn $name:ident(&mut self $(, $arg:ident: $ty:ty)*);)*) => {
        impl Handler for Counting<'_> {
            $(
                fn $name(&mut self $(, $arg: $ty)*) {
                    let state = self.before();
                    Handler::$name(&mut *self.term $(, $arg)*);
                    self.after(state);
                }
            )*
        }

        /// The names of the forwarded methods.
        #[cfg(test)]
        pub(super) const FORWARDED: &[&str] = &[$(stringify!($name)),*];
    };
}

forward! {
        fn set_title(&mut self, t: Option<String>);
        fn set_cursor_style(&mut self, s: Option<CursorStyle>);
        fn set_cursor_shape(&mut self, s: CursorShape);
        fn input(&mut self, c: char);
        fn goto(&mut self, line: i32, col: usize);
        fn goto_line(&mut self, line: i32);
        fn goto_col(&mut self, col: usize);
        fn insert_blank(&mut self, n: usize);
        fn move_up(&mut self, n: usize);
        fn move_down(&mut self, n: usize);
        fn identify_terminal(&mut self, intermediate: Option<char>);
        fn device_status(&mut self, n: usize);
        fn move_forward(&mut self, col: usize);
        fn move_backward(&mut self, col: usize);
        fn move_down_and_cr(&mut self, row: usize);
        fn move_up_and_cr(&mut self, row: usize);
        fn put_tab(&mut self, count: u16);
        fn backspace(&mut self);
        fn carriage_return(&mut self);
        fn linefeed(&mut self);
        fn bell(&mut self);
        fn substitute(&mut self);
        fn newline(&mut self);
        fn set_horizontal_tabstop(&mut self);
        fn scroll_up(&mut self, n: usize);
        fn scroll_down(&mut self, n: usize);
        fn insert_blank_lines(&mut self, n: usize);
        fn delete_lines(&mut self, n: usize);
        fn erase_chars(&mut self, n: usize);
        fn delete_chars(&mut self, n: usize);
        fn move_backward_tabs(&mut self, count: u16);
        fn move_forward_tabs(&mut self, count: u16);
        fn save_cursor_position(&mut self);
        fn restore_cursor_position(&mut self);
        fn clear_line(&mut self, mode: LineClearMode);
        fn clear_screen(&mut self, mode: ClearMode);
        fn clear_tabs(&mut self, mode: TabulationClearMode);
        fn set_tabs(&mut self, interval: u16);
        fn reset_state(&mut self);
        fn reverse_index(&mut self);
        fn terminal_attribute(&mut self, attr: Attr);
        fn set_mode(&mut self, mode: Mode);
        fn unset_mode(&mut self, mode: Mode);
        fn report_mode(&mut self, mode: Mode);
        fn set_private_mode(&mut self, mode: PrivateMode);
        fn unset_private_mode(&mut self, mode: PrivateMode);
        fn report_private_mode(&mut self, mode: PrivateMode);
        fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>);
        fn set_keypad_application_mode(&mut self);
        fn unset_keypad_application_mode(&mut self);
        fn set_active_charset(&mut self, index: CharsetIndex);
        fn configure_charset(&mut self, index: CharsetIndex, set: StandardCharset);
        fn set_color(&mut self, index: usize, rgb: Rgb);
        fn dynamic_color_sequence(&mut self, prefix: String, index: usize, terminator: &str);
        fn reset_color(&mut self, index: usize);
        fn clipboard_store(&mut self, kind: u8, data: &[u8]);
        fn clipboard_load(&mut self, kind: u8, terminator: &str);
        fn decaln(&mut self);
        fn push_title(&mut self);
        fn pop_title(&mut self);
        fn text_area_size_pixels(&mut self);
        fn text_area_size_chars(&mut self);
        fn set_hyperlink(&mut self, link: Option<Hyperlink>);
        fn set_mouse_cursor_icon(&mut self, icon: CursorIcon);
        fn report_keyboard_mode(&mut self);
        fn push_keyboard_mode(&mut self, mode: KeyboardModes);
        fn pop_keyboard_modes(&mut self, to_pop: u16);
        fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior);
        fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys);
        fn report_modify_other_keys(&mut self);
        fn set_scp(&mut self, path: ScpCharPath, update: ScpUpdateMode);
}
