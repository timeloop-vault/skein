//! Byte-exact edits of a Hermes `config.yaml` (#627): add, detect and
//! remove Skein's `mcp_servers.skein` entry.
//!
//! Hermes refuses to start on a malformed config, so this is not a YAML
//! round-trip: it splices text and touches nothing outside the block it
//! inserts. Anything it cannot place with certainty is refused.
//!
//! The inserted block is fenced by marker comments so removal is exact:
//!
//! ```text
//! mcp_servers:
//!   # skein:begin — added by Skein for its review tools; inert outside Skein
//!   skein:
//!     url: ${SKEIN_REVIEW_URL}
//!     ...
//!   # skein:end
//! ```
//!
//! When the file has no `mcp_servers:` at all, Skein creates it and the
//! begin marker goes at COLUMN 0, above the `mcp_servers:` line (and the
//! end marker at column 0 too). That is the distinct variant: removal
//! then takes the `mcp_servers:` line with the block, unless the user
//! has since added children below the end marker (then only the entry
//! goes and the key stays).

use std::fmt;

pub(crate) const BEGIN: &str =
    "# skein:begin \u{2014} added by Skein for its review tools; inert outside Skein";
const BEGIN_PREFIX: &str = "# skein:begin";
const END: &str = "# skein:end";
const KEY: &str = "mcp_servers";
const BOM: &str = "\u{feff}";

/// Why an edit was not attempted; the reason is meant for a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditRefused(pub String);

impl fmt::Display for EditRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(super) fn refuse<T>(why: &str) -> Result<T, EditRefused> {
    Err(EditRefused(why.to_string()))
}

pub(super) struct Line {
    pub(super) start: usize,
    /// End of the content, before any `\r\n` / `\n`.
    pub(super) content_end: usize,
    /// End including the terminator.
    pub(super) end: usize,
}

pub(super) struct Doc<'a> {
    pub(super) text: &'a str,
    pub(super) lines: Vec<Line>,
    pub(super) eol: &'static str,
}

impl<'a> Doc<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        let mut start = if text.starts_with(BOM) { BOM.len() } else { 0 };
        let mut lines = Vec::new();
        while start < text.len() {
            let end = text[start..]
                .find('\n')
                .map_or(text.len(), |i| start + i + 1);
            let mut content_end = end;
            if text[..end].ends_with('\n') {
                content_end -= 1;
                if content_end > start && text[..content_end].ends_with('\r') {
                    content_end -= 1;
                }
            }
            lines.push(Line {
                start,
                content_end,
                end,
            });
            start = end;
        }
        let eol = match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
            _ => "\n",
        };
        Doc { text, lines, eol }
    }

    pub(super) fn content(&self, i: usize) -> &'a str {
        &self.text[self.lines[i].start..self.lines[i].content_end]
    }

    pub(super) fn has_terminator(&self, i: usize) -> bool {
        self.lines[i].end > self.lines[i].content_end
    }
}

pub(super) fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

pub(super) fn is_comment(s: &str) -> bool {
    s.trim_start().starts_with('#')
}

pub(super) fn indent_of(s: &str) -> &str {
    &s[..s.len() - s.trim_start_matches([' ', '\t']).len()]
}

/// `key:` at the start of `s` (already unindented), followed by end or
/// whitespace. Returns the rest after the colon.
pub(super) fn key_rest<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let after = s.strip_prefix(key)?.trim_start_matches([' ', '\t']);
    let rest = after.strip_prefix(':')?;
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some(rest)
}

pub(super) fn quoted_key(s: &str, key: &str) -> bool {
    ['"', '\''].iter().any(|q| {
        s.strip_prefix(*q)
            .and_then(|r| r.strip_prefix(key))
            .and_then(|r| r.strip_prefix(*q))
            .is_some_and(|r| r.trim_start().starts_with(':'))
    })
}

/// A line whose value is a block-scalar header (`|`, `>`, `|-`, `>+`,
/// `|2`, ...), optionally followed by a comment. The lines below it are
/// text, so a `#` there is not a comment and an edit near it is unsafe.
fn opens_block_scalar(line: &str) -> bool {
    let code = match line.find(" #") {
        Some(i) => &line[..i],
        None => line,
    };
    let Some(token) = code.split_whitespace().next_back() else {
        return false;
    };
    let mut chars = token.chars();
    matches!(chars.next(), Some('|' | '>')) && chars.all(|c| matches!(c, '+' | '-' | '0'..='9'))
}

/// Whether any non-comment line in `from..to` opens a block scalar.
pub(super) fn block_scalar_between(doc: &Doc<'_>, from: usize, to: usize) -> bool {
    (from..to.min(doc.lines.len()))
        .map(|i| doc.content(i))
        .any(|c| !is_comment(c) && opens_block_scalar(c))
}

/// Whether the document root is a block mapping, judged from its first
/// content line (after blanks, comments and a bare `---`).
pub(super) fn root_is_block_mapping(doc: &Doc<'_>) -> bool {
    let first = (0..doc.lines.len())
        .map(|i| doc.content(i))
        .find(|c| !is_blank(c) && !is_comment(c) && c.trim_end() != "---");
    let Some(c) = first else {
        return true;
    };
    let c = c.trim_start();
    !(c.starts_with(['{', '[']) || c == "-" || c.starts_with("- ") || c.starts_with("-	"))
}

/// Whole-file check: more than one YAML document makes any edit unsafe.
pub(super) fn check_single_document(doc: &Doc<'_>) -> Result<(), EditRefused> {
    let mut seen_content = false;
    let mut markers = 0;
    for i in 0..doc.lines.len() {
        let c = doc.content(i);
        if c == "---" || c.starts_with("--- ") || c.starts_with("---\t") {
            markers += 1;
            if seen_content || markers > 1 {
                return refuse("the file has several YAML documents");
            }
        } else if c == "..." || c.starts_with("... ") {
            return refuse("the file has several YAML documents");
        } else if !is_blank(c) && !is_comment(c) {
            seen_content = true;
        }
    }
    Ok(())
}

pub(super) enum Value {
    Empty,
    Null,
}

/// The top-level `mcp_servers:` mapping.
pub(super) struct Top {
    pub(super) line: usize,
    pub(super) value: Value,
    /// Indentation of the first child, when there is one.
    pub(super) child_indent: Option<String>,
    /// Last indented content line of the mapping.
    pub(super) last_content: Option<usize>,
    /// Lines that are direct children (`indent == child_indent`).
    pub(super) child_lines: Vec<usize>,
    /// A line in the block opens a block scalar.
    pub(super) block_scalar: bool,
}

fn find_top(doc: &Doc<'_>) -> Result<Option<Top>, EditRefused> {
    find_top_key(doc, KEY)
}

/// [`find_top`] for any top-level key (#625 reuses it for `hooks`).
pub(super) fn find_top_key(doc: &Doc<'_>, key: &str) -> Result<Option<Top>, EditRefused> {
    let mut found = None;
    for i in 0..doc.lines.len() {
        let c = doc.content(i);
        if quoted_key(c, key) {
            return refuse(&format!("{key} is written as a quoted key"));
        }
        if let Some(rest) = key_rest(c, key) {
            found = Some((i, rest));
            break;
        }
    }
    let Some((line, rest)) = found else {
        return Ok(None);
    };
    let rest = rest.trim();
    let value = if rest.is_empty() || rest.starts_with('#') {
        Value::Empty
    } else if rest.starts_with(['&', '*', '!']) {
        return refuse(&format!("{key} uses a YAML anchor, alias or tag"));
    } else if rest.starts_with(['{', '[']) {
        return refuse(&format!(
            "{key} is written in flow style ({{...}} or [...])"
        ));
    } else if rest == "~" || rest == "null" || rest.starts_with("null ") || rest.starts_with("~ ") {
        Value::Null
    } else {
        return refuse(&format!("{key} has a value Skein does not recognise"));
    };

    let mut child_indent: Option<String> = None;
    let mut last_content = None;
    let mut child_lines = Vec::new();
    for j in line + 1..doc.lines.len() {
        let c = doc.content(j);
        if is_blank(c) {
            continue;
        }
        let ind = indent_of(c);
        if ind.is_empty() {
            if is_comment(c) {
                continue;
            }
            break;
        }
        if ind.contains('\t') {
            return refuse(&format!("the {key} block is indented with tabs"));
        }
        if is_comment(c) {
            continue;
        }
        let body = &c[ind.len()..];
        if body.starts_with("<<") {
            return refuse(&format!("{key} uses a merge key"));
        }
        if child_indent.is_none() {
            if body.starts_with('-') {
                return refuse(&format!("{key} holds a list, not a mapping"));
            }
            child_indent = Some(ind.to_string());
        }
        if child_indent.as_deref() == Some(ind) {
            child_lines.push(j);
        }
        last_content = Some(j);
    }
    // A column-0 `- item` straight under an empty `mcp_servers:` is a list.
    if child_indent.is_none() {
        let next = (line + 1..doc.lines.len())
            .map(|j| doc.content(j))
            .find(|c| !is_blank(c) && !is_comment(c));
        if next.is_some_and(|c| c.starts_with('-') && !c.starts_with("---")) {
            return refuse(&format!("{key} holds a list, not a mapping"));
        }
    }
    let block_scalar =
        block_scalar_between(doc, line + 1, last_content.map_or(line + 1, |l| l + 1));
    Ok(Some(Top {
        line,
        value,
        child_indent,
        last_content,
        child_lines,
        block_scalar,
    }))
}

fn has_skein_child(doc: &Doc<'_>, top: &Top) -> bool {
    top.child_lines.iter().any(|&j| {
        let body = doc.content(j).trim_start();
        key_rest(body, "skein").is_some() || quoted_key(body, "skein")
    })
}

/// Whether a top-level (column 0) `mcp_servers:` mapping has a direct
/// child key `skein` (marked by Skein or not). Files that cannot be
/// read as such a mapping report `false`.
pub(crate) fn skein_entry_present(text: &str) -> bool {
    let doc = Doc::new(text);
    matches!(find_top(&doc), Ok(Some(top)) if has_skein_child(&doc, &top))
}

struct Plan {
    /// Byte offset the text goes at.
    at: usize,
    /// The preceding line has no terminator: add one first.
    prefix_eol: bool,
    lines: Vec<String>,
    /// Inserting after an unterminated last line: the block ends the
    /// same way, so the file still has no final newline.
    no_final_eol: bool,
}

impl Plan {
    fn text(&self, eol: &str) -> String {
        let mut s = String::new();
        if self.prefix_eol {
            s.push_str(eol);
        }
        s.push_str(&self.lines.join(eol));
        if !self.no_final_eol {
            s.push_str(eol);
        }
        s
    }
}

/// The five lines of the `skein:` entry itself, `child` deep.
fn entry_body(child: &str, unit: &str) -> Vec<String> {
    let deep = format!("{child}{unit}");
    let inner = format!("{deep}{unit}");
    vec![
        format!("{child}skein:"),
        format!("{deep}url: ${{SKEIN_REVIEW_URL}}"),
        format!("{deep}headers:"),
        format!("{inner}Authorization: Bearer ${{SKEIN_REVIEW_TOKEN}}"),
        format!("{inner}X-Skein-Harness: ${{SKEIN_HARNESS_ID}}"),
    ]
}

/// Marker-wrapped entry at `child` indentation.
fn entry_lines(child: &str, unit: &str) -> Vec<String> {
    let mut lines = vec![format!("{child}{BEGIN}")];
    lines.extend(entry_body(child, unit));
    lines.push(format!("{child}{END}"));
    lines
}

pub(super) fn unit_for(child: &str) -> &str {
    if child.len() == 4 { child } else { "  " }
}

fn plan(doc: &Doc<'_>) -> Result<Option<Plan>, EditRefused> {
    check_single_document(doc)?;
    let top = find_top(doc)?;
    let n = doc.lines.len();
    // Insert after line `i`: (offset, prefix_eol, no_final_eol).
    let after = |i: usize| -> (usize, bool, bool) {
        let unterminated = !doc.has_terminator(i);
        (doc.lines[i].end, unterminated, unterminated)
    };
    let Some(top) = top else {
        // (a) no mcp_servers: append a full block, markers at column 0.
        if !root_is_block_mapping(doc) {
            return refuse("the config's top level is not a block mapping");
        }
        let mut lines = vec![BEGIN.to_string(), format!("{KEY}:")];
        lines.extend(entry_body("  ", "  "));
        lines.push(END.to_string());
        let (at, prefix_eol, no_final_eol) = if n == 0 {
            (doc.text.len(), false, false)
        } else {
            after(n - 1)
        };
        return Ok(Some(Plan {
            at,
            prefix_eol,
            lines,
            no_final_eol,
        }));
    };
    if has_skein_child(doc, &top) {
        return Ok(None);
    }
    if top.block_scalar {
        return refuse("the mcp_servers block contains a block scalar (| or >)");
    }
    match (&top.value, &top.child_indent) {
        (Value::Null, _) => {
            refuse("mcp_servers is explicitly null; edit it by hand or empty the value")
        }
        (Value::Empty, None) => {
            let (at, prefix_eol, no_final_eol) = after(top.line);
            Ok(Some(Plan {
                at,
                prefix_eol,
                lines: entry_lines("  ", "  "),
                no_final_eol,
            }))
        }
        (Value::Empty, Some(child)) => {
            let (at, prefix_eol, no_final_eol) = after(top.last_content.unwrap_or(top.line));
            Ok(Some(Plan {
                at,
                prefix_eol,
                lines: entry_lines(child, unit_for(child)),
                no_final_eol,
            }))
        }
    }
}

/// The text with Skein's entry added. `Ok(None)`: a `skein` server is
/// already there, nothing to do.
pub(crate) fn insert_skein_entry(text: &str) -> Result<Option<String>, EditRefused> {
    let doc = Doc::new(text);
    Ok(plan(&doc)?.map(|p| {
        let mut out = String::with_capacity(text.len() + 300);
        out.push_str(&text[..p.at]);
        out.push_str(&p.text(doc.eol));
        out.push_str(&text[p.at..]);
        out
    }))
}

/// The exact lines [`insert_skein_entry`] would add, joined by the
/// file's line ending, without a final terminator. Empty when a
/// `skein` server is already present.
pub(crate) fn entry_preview(text: &str) -> Result<String, EditRefused> {
    let doc = Doc::new(text);
    Ok(plan(&doc)?
        .map(|p| p.lines.join(doc.eol))
        .unwrap_or_default())
}

/// Remove the marker-delimited block Skein wrote, markers included.
/// `None` when there is no such block (an unmarked `skein` entry is
/// never touched).
pub(crate) fn remove_skein_entry(text: &str) -> Option<String> {
    let doc = Doc::new(text);
    let n = doc.lines.len();
    let b = (0..n).find(|&i| doc.content(i).trim_start().starts_with(BEGIN_PREFIX))?;
    let e = (b + 1..n).find(|&i| doc.content(i).trim() == END)?;
    // Only a block that still reads exactly as Skein wrote it.
    if doc.content(b).trim() != BEGIN {
        return None;
    }
    let key_line = (0..b).find(|&i| key_rest(doc.content(i), KEY).is_some());
    if block_scalar_between(&doc, key_line.map_or(b, |k| k + 1), e) {
        return None;
    }
    let owns_key = indent_of(doc.content(b)).is_empty()
        && b + 2 <= e
        && key_rest(doc.content(b + 1), KEY).is_some_and(|r| is_blank(r) || is_comment(r));
    let orphaned = owns_key
        && (e + 1..n)
            .map(|j| doc.content(j))
            .find(|c| !is_blank(c) && !is_comment(c))
            .is_some_and(|c| !indent_of(c).is_empty());
    let mut start = doc.lines[b].start;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    if orphaned {
        // Keep `mcp_servers:` for the children added after the block.
        ranges.push((start, doc.lines[b].end));
        ranges.push((doc.lines[b + 2].start, doc.lines[e].end));
    } else {
        // The block ended the file without a newline: take the newline
        // we added before it too.
        if !doc.has_terminator(e) && b > 0 {
            start = doc.lines[b - 1].content_end;
        }
        ranges.push((start, doc.lines[e].end));
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (s, end) in ranges {
        out.push_str(&text[pos..s]);
        pos = end;
    }
    out.push_str(&text[pos..]);
    Some(out)
}
