//! The compact, serialisable form of a [`GridDump`] that the replay gate pins
//! in `fixtures/NAME.engine.json` (#793, slice T-1) and a JavaScript test
//! compares with xterm.js. The format is documented in `fixtures/README.md`.
//!
//! A per-cell object would make the files megabytes, so a row is a list of
//! runs (consecutive cells with one style and one link) and the styles live
//! in one table per file.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Value, json};

use super::dump::{CellDump, ColorDump, CursorDump, GridDump, RowDump};
use super::{AlacrittyEngine, Modes};

/// One entry of a file's style table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[allow(clippy::struct_excessive_bools)] // independent SGR flags
pub struct StyleEntry {
    pub fg: Value,
    pub bg: Value,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub underline: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
}

fn color(c: ColorDump) -> Value {
    match c {
        ColorDump::Default => json!("default"),
        ColorDump::Palette { index } => json!({ "p": index }),
        ColorDump::Rgb { r, g, b } => json!({ "rgb": format!("{r:02x}{g:02x}{b:02x}") }),
    }
}

fn style_of(c: &CellDump) -> StyleEntry {
    StyleEntry {
        fg: color(c.fg),
        bg: color(c.bg),
        bold: c.bold,
        italic: c.italic,
        dim: c.dim,
        underline: c.underline,
        inverse: c.inverse,
        invisible: c.invisible,
        strikethrough: c.strikethrough,
    }
}

/// The styles of one fixture, in order of first use. Id 0 is always the
/// default style.
#[derive(Debug)]
pub struct StyleTable {
    pub styles: Vec<StyleEntry>,
    ids: HashMap<String, usize>,
}

impl StyleTable {
    pub fn new() -> Self {
        let mut t = Self {
            styles: Vec::new(),
            ids: HashMap::new(),
        };
        let default = StyleEntry {
            fg: color(ColorDump::Default),
            bg: color(ColorDump::Default),
            bold: false,
            italic: false,
            dim: false,
            underline: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
        };
        t.intern(default);
        t
    }

    fn intern(&mut self, s: StyleEntry) -> usize {
        let key = serde_json::to_string(&s).expect("a style serialises");
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = self.styles.len();
        self.ids.insert(key, id);
        self.styles.push(s);
        id
    }
}

/// Consecutive cells with one style and one link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// First column, zero-based.
    pub col: usize,
    /// One past the last column. A wide character occupies two columns.
    pub end: usize,
    pub text: String,
    pub style: usize,
    pub link: Option<String>,
}

/// Serialised as `[col, end, style, text]`, with the link URI as a fifth
/// element only when the run has one (the files stay small).
impl Serialize for Run {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(if self.link.is_some() { 5 } else { 4 }))?;
        seq.serialize_element(&self.col)?;
        seq.serialize_element(&self.end)?;
        seq.serialize_element(&self.style)?;
        seq.serialize_element(&self.text)?;
        if let Some(l) = &self.link {
            seq.serialize_element(l)?;
        }
        seq.end()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompactRow {
    pub wrapped: bool,
    pub runs: Vec<Run>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactCut {
    pub name: String,
    pub after_event: usize,
    pub rows: u16,
    pub cols: u16,
    pub alt_screen: bool,
    pub cursor: CursorDump,
    pub modes: Modes,
    pub history_rows_entered: u64,
    pub rebuilds: u32,
    pub history: Vec<CompactRow>,
    pub screen: Vec<CompactRow>,
}

fn is_blank_default(c: &CellDump, default_style: &StyleEntry) -> bool {
    c.text == " " && c.link.is_none() && style_of(c) == *default_style
}

fn compact_row(row: &RowDump, table: &mut StyleTable) -> CompactRow {
    let default_style = table.styles[0].clone();
    let mut end = row.cells.len();
    while end > 0
        && row.cells[end - 1].width != 0
        && is_blank_default(&row.cells[end - 1], &default_style)
    {
        end -= 1;
    }
    let mut runs: Vec<Run> = Vec::new();
    for (col, cell) in row.cells[..end].iter().enumerate() {
        // The spacer half of a wide character joins the run of its wide half
        // whatever its own style says.
        if cell.width == 0
            && let Some(run) = runs.last_mut()
        {
            run.end = col + 1;
            continue;
        }
        let style = table.intern(style_of(cell));
        match runs.last_mut() {
            Some(run) if run.style == style && run.link == cell.link => {
                run.text.push_str(&cell.text);
                run.end = col + 1;
            }
            _ => runs.push(Run {
                col,
                end: col + 1,
                text: cell.text.clone(),
                style,
                link: cell.link.clone(),
            }),
        }
    }
    CompactRow {
        wrapped: row.wrapped,
        runs,
    }
}

/// The compact picture of `engine` as it stands after `after_event`.
pub fn compact(
    engine: &AlacrittyEngine,
    table: &mut StyleTable,
    name: &str,
    after_event: usize,
) -> CompactCut {
    let d: GridDump = engine.dump();
    CompactCut {
        name: name.to_owned(),
        after_event,
        rows: d.rows,
        cols: d.cols,
        alt_screen: d.alt_screen,
        cursor: d.cursor,
        modes: d.modes,
        history_rows_entered: engine.history_rows_entered(),
        rebuilds: engine.rebuilds(),
        history: d.history.iter().map(|r| compact_row(r, table)).collect(),
        screen: d.screen.iter().map(|r| compact_row(r, table)).collect(),
    }
}
