//! Custom Floem `View` rendering an [`alacritty_terminal::Term`] grid.
//! Cell rendering pattern lifted from `lapce/lapce-app/src/terminal/
//! view.rs`, simplified for the prototype: monochrome glyphs (no
//! per-cell color theming), no IDE integration, no hyperlink
//! detection, no run/debug overlay.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as TermPoint, Side},
    selection::{Selection, SelectionType},
    term::cell::Flags,
};
use floem::{
    Clipboard, View, ViewId,
    context::{ComputeLayoutCx, EventCx, LayoutCx, PaintCx, UpdateCx},
    event::{Event, EventPropagation},
    keyboard::{Key, KeyEvent, Modifiers, NamedKey},
    peniko::{
        Color,
        kurbo::{Point, Rect, Size, Stroke},
    },
    pointer::PointerButton,
    taffy::prelude::NodeId,
    text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight},
};
use floem_renderer::Renderer;
use parking_lot::RwLock;

use crate::term::RawTerm;
use crate::term_colors::{DEFAULT_BG, DEFAULT_CURSOR, term_color};

const FONT_FAMILY: &str = "monospace";
const FONT_SIZE: f32 = 13.0;

pub type ResizeCallback = Box<dyn Fn(u16, u16)>;
pub type InputCallback = Box<dyn Fn(&[u8])>;

pub struct TerminalView {
    id: ViewId,
    raw: Arc<RwLock<RawTerm>>,
    on_resize: ResizeCallback,
    on_input: InputCallback,
    size: Size,
    cell_size: Size,
    focused: Arc<AtomicBool>,
    /// True between primary-button down and up — used to disambiguate
    /// `PointerMove` (drag-extends-selection vs hover).
    selecting: bool,
}

pub fn terminal_view(
    raw: Arc<RwLock<RawTerm>>,
    on_resize: impl Fn(u16, u16) + 'static,
    on_input: impl Fn(&[u8]) + 'static,
) -> TerminalView {
    let id = ViewId::new();
    TerminalView {
        id,
        raw,
        on_resize: Box::new(on_resize),
        on_input: Box::new(on_input),
        size: Size::ZERO,
        cell_size: Size::ZERO,
        focused: Arc::new(AtomicBool::new(false)),
        selecting: false,
    }
}

fn measure_cell() -> Size {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(FONT_FAMILY).collect();
    let attrs = Attrs::new().family(&family).font_size(FONT_SIZE);
    let mut layout = TextLayout::new();
    layout.set_text("W", AttrsList::new(attrs));
    layout.size()
}

fn grid_dims(area: Size, cell: Size) -> (u16, u16) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cols = (area.width / cell.width).floor().max(1.0) as u16;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rows = (area.height / cell.height).floor().max(1.0) as u16;
    (cols, rows)
}

impl TerminalView {
    pub fn view_id(&self) -> ViewId {
        self.id
    }

    /// Hand out a clone of the focused-state flag so the host can wire
    /// it up via `.on_event(EventListener::FocusGained, ...)`.
    pub fn focus_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.focused)
    }

    /// Pixel position → alacritty grid point. Accounts for the
    /// current scroll offset so a click on a scrolled-up line lands
    /// on the right history row.
    fn pixel_to_grid(&self, pos: Point) -> TermPoint {
        let cell = self.cell_size;
        let raw = self.raw.read();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let col = (pos.x / cell.width).max(0.0) as usize;
        #[allow(clippy::cast_possible_truncation)]
        let screen_line = (pos.y / cell.height) as i32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let line_no = screen_line - raw.term.grid().display_offset() as i32;
        TermPoint::new(Line(line_no), Column(col))
    }
}

impl View for TerminalView {
    fn id(&self) -> ViewId {
        self.id
    }

    fn event_before_children(&mut self, _cx: &mut EventCx, event: &Event) -> EventPropagation {
        match event {
            Event::PointerDown(e) => {
                self.id.request_focus();
                match e.button {
                    PointerButton::Primary => {
                        let point = self.pixel_to_grid(e.pos);
                        let mut raw = self.raw.write();
                        raw.term.selection =
                            Some(Selection::new(SelectionType::Simple, point, Side::Left));
                        self.selecting = true;
                    }
                    PointerButton::Secondary => {
                        copy_selection_to_clipboard(&self.raw);
                        self.raw.write().term.selection = None;
                    }
                    _ => {}
                }
                self.id.request_paint();
                EventPropagation::Stop
            }
            Event::PointerMove(e) => {
                if self.selecting {
                    let point = self.pixel_to_grid(e.pos);
                    let mut raw = self.raw.write();
                    if let Some(sel) = raw.term.selection.as_mut() {
                        sel.update(point, Side::Right);
                    }
                    drop(raw);
                    self.id.request_paint();
                }
                EventPropagation::Continue
            }
            Event::PointerUp(_) => {
                self.selecting = false;
                EventPropagation::Continue
            }
            Event::PointerWheel(e) => {
                let cell_h = self.cell_size.height.max(1.0);
                let mut raw = self.raw.write();
                // Follow Lapce's convention: positive wheel delta =
                // content moves down (user sees newer / lower lines).
                // Inverting here means we respect the user's
                // system-level natural-scroll preference, which winit
                // already pre-applies to delta.y.
                raw.scroll_delta -= e.delta.y;
                #[allow(clippy::cast_possible_truncation)]
                let lines = (raw.scroll_delta / cell_h) as i32;
                if lines != 0 {
                    #[allow(clippy::cast_precision_loss)]
                    let consumed = f64::from(lines) * cell_h;
                    raw.scroll_delta -= consumed;
                    raw.term.scroll_display(Scroll::Delta(lines));
                    drop(raw);
                    self.id.request_paint();
                }
                EventPropagation::Stop
            }
            Event::KeyDown(key) => {
                // Cmd+V (Mac) / Ctrl+Shift+V (other) — paste. Treat
                // the clipboard contents as a typed input stream.
                if is_paste_combo(key) {
                    if let Ok(text) = Clipboard::get_contents() {
                        self.raw.write().term.scroll_display(Scroll::Bottom);
                        (self.on_input)(text.as_bytes());
                        self.id.request_paint();
                    }
                    return EventPropagation::Stop;
                }
                // Cmd+C with an active selection — copy. Cmd+C with
                // no selection falls through so the normal "send ^C"
                // path on Ctrl+C still works (Mac's Cmd+C is a
                // clipboard convention, never a SIGINT).
                if is_copy_combo(key) {
                    let has_sel = self.raw.read().term.selection.is_some();
                    if has_sel {
                        copy_selection_to_clipboard(&self.raw);
                        return EventPropagation::Stop;
                    }
                }
                if let Some(bytes) = key_to_bytes(key) {
                    // Typing snaps the viewport back to the bottom —
                    // matches every terminal emulator's behavior.
                    self.raw.write().term.scroll_display(Scroll::Bottom);
                    (self.on_input)(&bytes);
                    self.id.request_paint();
                    EventPropagation::Stop
                } else {
                    EventPropagation::Continue
                }
            }
            _ => EventPropagation::Continue,
        }
    }

    fn update(&mut self, _cx: &mut UpdateCx, _state: Box<dyn std::any::Any>) {
        self.id.request_paint();
    }

    fn layout(&mut self, cx: &mut LayoutCx) -> NodeId {
        cx.layout_node(self.id, false, |_| Vec::new())
    }

    fn compute_layout(&mut self, _cx: &mut ComputeLayoutCx) -> Option<Rect> {
        let layout = self.id.get_layout().unwrap_or_default();
        let new_size = Size::new(f64::from(layout.size.width), f64::from(layout.size.height));
        if new_size.is_zero_area() {
            return None;
        }
        if (new_size.width - self.size.width).abs() > 0.5
            || (new_size.height - self.size.height).abs() > 0.5
        {
            self.size = new_size;
            self.cell_size = measure_cell();
            let (cols, rows) = grid_dims(new_size, self.cell_size);
            self.raw.write().resize(rows, cols);
            (self.on_resize)(rows, cols);
        }
        None
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        if self.cell_size.width <= 0.0 {
            self.cell_size = measure_cell();
        }
        let cell = self.cell_size;
        let is_focused = self.focused.load(Ordering::Relaxed);

        cx.fill(
            &Rect::from_origin_size(Point::ZERO, self.size),
            DEFAULT_BG,
            0.0,
        );

        let family: Vec<FamilyOwned> = FamilyOwned::parse_list(FONT_FAMILY).collect();
        let attrs = Attrs::new().family(&family).font_size(FONT_SIZE);

        let raw = self.raw.read();
        let term = &raw.term;
        let content = term.renderable_content();
        let cursor_point = content.cursor.point;
        #[allow(clippy::cast_precision_loss)]
        let display_offset = content.display_offset as f64;

        // Selection background — paint first so glyphs render on top.
        if let Some(sel) = content.selection.as_ref() {
            let sel_color = Color::rgb8(0x33, 0x55, 0x88);
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            let off = content.display_offset as i32;
            let start_line = sel.start.line.0 + off;
            let end_line = sel.end.line.0 + off;
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            let last_col = term.columns().saturating_sub(1) as i32;
            for line in start_line..=end_line {
                let left = if sel.is_block || line == start_line {
                    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
                    {
                        sel.start.column.0 as i32
                    }
                } else {
                    0
                };
                let right = if sel.is_block || line == end_line {
                    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
                    {
                        sel.end.column.0 as i32 + 1
                    }
                } else {
                    last_col + 1
                };
                if right <= left {
                    continue;
                }
                let x0 = f64::from(left) * cell.width;
                let x1 = f64::from(right) * cell.width;
                let y0 = f64::from(line) * cell.height;
                let y1 = y0 + cell.height;
                cx.fill(&Rect::new(x0, y0, x1, y1), sel_color, 0.0);
            }
        }

        for indexed in content.display_iter {
            let cell_data = indexed.cell;
            let point = indexed.point;
            let c = cell_data.c;

            #[allow(clippy::cast_precision_loss)]
            let x = point.column.0 as f64 * cell.width;
            let y = (f64::from(point.line.0) + display_offset) * cell.height;

            let inverse = cell_data.flags.contains(Flags::INVERSE);
            let mut fg = term_color(cell_data.fg);
            let mut bg = term_color(cell_data.bg);
            if inverse {
                std::mem::swap(&mut fg, &mut bg);
            }

            // Paint background only if it differs from the terminal
            // default — covers the common case where most cells use
            // the default and saves a fill per blank cell.
            if bg != DEFAULT_BG {
                cx.fill(&Rect::from_origin_size(Point::new(x, y), cell), bg, 0.0);
            }

            if c == ' ' || c == '\t' {
                continue;
            }

            let mut a = attrs.color(fg);
            if cell_data.flags.contains(Flags::BOLD) || cell_data.flags.contains(Flags::DIM_BOLD) {
                a = a.weight(Weight::BOLD);
            }

            let mut layout = TextLayout::new();
            layout.set_text(&c.to_string(), AttrsList::new(a));
            cx.draw_text(&layout, Point::new(x, y));
        }

        #[allow(clippy::cast_precision_loss)]
        let cx0 = cursor_point.column.0 as f64 * cell.width;
        let cy0 = (f64::from(cursor_point.line.0) + display_offset) * cell.height;
        let cursor_rect = Rect::from_origin_size(Point::new(cx0, cy0), cell);
        let cursor_color = DEFAULT_CURSOR;
        if is_focused {
            cx.fill(&cursor_rect, cursor_color, 0.0);
        } else {
            cx.stroke(&cursor_rect, cursor_color, &Stroke::new(1.0));
        }

        let _ = &self.on_input;
    }
}

fn is_paste_combo(key: &KeyEvent) -> bool {
    let Key::Character(c) = &key.key.logical_key else {
        return false;
    };
    if c.as_str() != "v" && c.as_str() != "V" {
        return false;
    }
    // macOS uses Cmd (Meta); other platforms use Ctrl+Shift.
    #[cfg(target_os = "macos")]
    {
        key.modifiers == Modifiers::META
    }
    #[cfg(not(target_os = "macos"))]
    {
        key.modifiers == Modifiers::CONTROL | Modifiers::SHIFT
    }
}

fn is_copy_combo(key: &KeyEvent) -> bool {
    let Key::Character(c) = &key.key.logical_key else {
        return false;
    };
    if c.as_str() != "c" && c.as_str() != "C" {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        key.modifiers == Modifiers::META
    }
    #[cfg(not(target_os = "macos"))]
    {
        key.modifiers == Modifiers::CONTROL | Modifiers::SHIFT
    }
}

fn copy_selection_to_clipboard(raw: &Arc<RwLock<RawTerm>>) {
    let raw = raw.read();
    let Some(sel) = raw.term.selection.as_ref() else {
        return;
    };
    let Some(range) = sel.to_range(&raw.term) else {
        return;
    };
    let content = raw.term.bounds_to_string(range.start, range.end);
    if !content.is_empty() {
        let _ = Clipboard::set_contents(content);
    }
}

/// Convert a Floem `KeyEvent` into the bytes a terminal expects to
/// receive. Lifted (and trimmed) from `lapce/lapce-app/src/terminal/
/// data.rs::resolve_key_event`. Covers the substrate the spike needs
/// — printable + ctrl-letter + arrows + enter / backspace / tab /
/// escape + Alt+letter as ESC-prefix. No keypad-cursor mode, no
/// home/end/pgup/pgdn modifier matrix.
fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    match &key.key.logical_key {
        Key::Character(c) => {
            if key.modifiers == Modifiers::CONTROL {
                let b = match c.as_str() {
                    "@" => 0x00,
                    "a" => 0x01,
                    "b" => 0x02,
                    "c" => 0x03,
                    "d" => 0x04,
                    "e" => 0x05,
                    "f" => 0x06,
                    "g" => 0x07,
                    "h" => 0x08,
                    "i" => 0x09,
                    "j" => 0x0a,
                    "k" => 0x0b,
                    "l" => 0x0c,
                    "m" => 0x0d,
                    "n" => 0x0e,
                    "o" => 0x0f,
                    "p" => 0x10,
                    "q" => 0x11,
                    "r" => 0x12,
                    "s" => 0x13,
                    "t" => 0x14,
                    "u" => 0x15,
                    "v" => 0x16,
                    "w" => 0x17,
                    "x" => 0x18,
                    "y" => 0x19,
                    "z" => 0x1a,
                    "[" => 0x1b,
                    "\\" => 0x1c,
                    "]" => 0x1d,
                    "^" => 0x1e,
                    "_" => 0x1f,
                    _ => return None,
                };
                Some(vec![b])
            } else if key.modifiers == Modifiers::ALT {
                // Alt+x → ESC + x
                let mut out = vec![0x1b];
                out.extend_from_slice(c.as_bytes());
                Some(out)
            } else if key.modifiers.is_empty() || key.modifiers == Modifiers::SHIFT {
                Some(c.as_bytes().to_vec())
            } else {
                None
            }
        }
        Key::Named(NamedKey::Backspace) => Some(if key.modifiers.control() {
            vec![0x08]
        } else if key.modifiers.alt() {
            vec![0x1b, 0x7f]
        } else {
            vec![0x7f]
        }),
        Key::Named(NamedKey::Space) => Some(b" ".to_vec()),
        Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
        Key::Named(NamedKey::Enter) => Some(if key.modifiers.shift() {
            // Shift+Enter — same trick claude uses: send a literal
            // newline distinct from carriage return so the TUI can
            // branch on it.
            b"\x1b\r".to_vec()
        } else {
            b"\r".to_vec()
        }),
        Key::Named(NamedKey::Escape) => Some(b"\x1b".to_vec()),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        Key::Named(NamedKey::Home) => Some(b"\x1bOH".to_vec()),
        Key::Named(NamedKey::End) => Some(b"\x1bOF".to_vec()),
        Key::Named(NamedKey::Delete) => Some(b"\x1b[3~".to_vec()),
        Key::Named(NamedKey::PageUp) => Some(b"\x1b[5~".to_vec()),
        Key::Named(NamedKey::PageDown) => Some(b"\x1b[6~".to_vec()),
        _ => None,
    }
}
