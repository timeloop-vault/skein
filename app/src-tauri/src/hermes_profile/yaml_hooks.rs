//! Byte-exact edits of a Hermes `config.yaml` (#625): add, detect and
//! remove Skein's shell hooks (`hooks:` + `hooks_auto_accept:`).
//!
//! Same contract as [`super::yaml`]: text is spliced, never round-tripped
//! through a YAML parser, and anything that cannot be placed with
//! certainty is refused. Skein's text is fenced by marker comments that
//! differ from the MCP block's, so neither detection nor removal can
//! confuse the two:
//!
//! ```text
//! # skein-hooks:begin — added by Skein for harness status; inert outside Skein
//! hooks_auto_accept: true
//! hooks:
//!   on_session_start:
//!     - command: '"C:\Program Files\Skein\hook.cmd" status'
//!       timeout: 10
//! # skein-hooks:end
//! ```
//!
//! Unlike the MCP entry, our hooks are REWRITTEN when they differ: every
//! marked region is stripped first and the current spec inserted afresh.
//! Unmarked user entries are never touched, even ones naming our command.
//!
//! Command quoting: Hermes splits the command with `split_command_line`
//! (`hermes_cli/_subprocess_compat.py`): `shlex.split(posix=False)` on
//! Windows (backslashes stay literal, quotes are kept in the token and one
//! matching pair is then stripped) and plain `shlex.split` elsewhere.
//! A double-quoted program path therefore survives spaces on both; see
//! [`quote_arg`]. The whole command line is then written as a YAML
//! single-quoted scalar, where backslashes are literal too.

use super::yaml::{
    Doc, EditRefused, Value, check_single_document, find_top_key, indent_of, is_blank, is_comment,
    key_rest, quoted_key, refuse, root_is_block_mapping, unit_for,
};

pub(crate) type YamlError = EditRefused;

pub(crate) const HOOK_EVENTS: [&str; 8] = [
    "on_session_start",
    "pre_llm_call",
    "pre_tool_call",
    "on_session_end",
    "pre_approval_request",
    "post_approval_response",
    "subagent_start",
    "subagent_stop",
];

const BEGIN: &str =
    "# skein-hooks:begin \u{2014} added by Skein for harness status; inert outside Skein";
const END: &str = "# skein-hooks:end";
const KEY: &str = "hooks";
const AUTO: &str = "hooks_auto_accept";

/// What every event runs. `prefix` is the already-quoted front of a
/// command line as Hermes will split it (build it with [`quote_arg`]);
/// each event's command is `prefix + " " + event`, so the script learns
/// the event from `argv[1]`. `timeout_secs` is 1..=300.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookSpec {
    pub prefix: String,
    pub timeout_secs: u32,
}

impl HookSpec {
    /// The whole command line registered for `event`.
    pub(crate) fn command_for(&self, event: &str) -> String {
        format!("{} {event}", self.prefix)
    }
}

/// One argument of a command line that both `shlex.split` and Hermes'
/// Windows `posix=False` variant read back as `s`: bare when it needs no
/// quoting, else double-quoted. `None` when no quoting can carry it (an
/// embedded `"`, a control character, or a trailing backslash that would
/// escape the closing quote under POSIX rules).
pub(crate) fn quote_arg(s: &str) -> Option<String> {
    if s.is_empty() || s.contains('"') || s.chars().any(char::is_control) || s.ends_with('\\') {
        return None;
    }
    if s.contains(|c: char| c.is_whitespace() || "'\\$`#&;|<>()".contains(c)) {
        Some(format!("\"{s}\""))
    } else {
        Some(s.to_string())
    }
}

/// `s` as a YAML single-quoted scalar (`'` doubled, nothing else escapes).
fn yaml_scalar(s: &str) -> Option<String> {
    if s.chars().any(|c| c.is_control() && c != '\t') {
        return None;
    }
    Some(format!("'{}'", s.replace('\'', "''")))
}

fn item_lines(ind: &str, spec: &HookSpec, event: &str) -> Result<Vec<String>, YamlError> {
    let Some(scalar) = yaml_scalar(&spec.command_for(event)) else {
        return refuse("the hook command contains a control character");
    };
    Ok(vec![
        format!("{ind}- command: {scalar}"),
        format!("{ind}  timeout: {}", spec.timeout_secs),
    ])
}

fn validate(spec: &HookSpec) -> Result<(), YamlError> {
    if is_blank(&spec.prefix) {
        return refuse("the hook command is empty");
    }
    if !(1..=300).contains(&spec.timeout_secs) {
        return refuse("the hook timeout must be 1..=300 seconds");
    }
    HOOK_EVENTS
        .iter()
        .try_for_each(|ev| item_lines("", spec, ev).map(|_| ()))
}

fn event_block(
    ci: &str,
    unit: &str,
    event: &str,
    spec: &HookSpec,
) -> Result<Vec<String>, YamlError> {
    let mut lines = vec![format!("{ci}{event}:")];
    lines.extend(item_lines(&format!("{ci}{unit}"), spec, event)?);
    Ok(lines)
}

/// The whole block written when the file has no `hooks:` at all.
fn full_block(spec: &HookSpec, auto_accept: bool) -> Result<Vec<String>, YamlError> {
    let mut lines = vec![BEGIN.to_string()];
    if auto_accept {
        lines.push(format!("{AUTO}: true"));
    }
    lines.push(format!("{KEY}:"));
    for ev in HOOK_EVENTS {
        lines.extend(event_block("  ", "  ", ev, spec)?);
    }
    lines.push(END.to_string());
    Ok(lines)
}

/// The lines a consent dialog shows: what is added to a config that has
/// no `hooks:` yet (empty when the spec is invalid).
pub(crate) fn hooks_preview(spec: &HookSpec, auto_accept: bool) -> Vec<String> {
    if validate(spec).is_err() {
        return Vec::new();
    }
    full_block(spec, auto_accept).unwrap_or_default()
}

fn has_top_key(doc: &Doc<'_>, key: &str) -> bool {
    (0..doc.lines.len()).any(|i| {
        let c = doc.content(i);
        key_rest(c, key).is_some() || quoted_key(c, key)
    })
}

/// The top-level `hooks_auto_accept` value, when it is a plain boolean.
pub(crate) fn auto_accept_value(text: &str) -> Option<bool> {
    let doc = Doc::new(text);
    (0..doc.lines.len()).find_map(|i| {
        let rest = key_rest(doc.content(i), AUTO)?;
        let rest = rest.split(" #").next().unwrap_or("").trim();
        match rest.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    })
}

/// The file sets `hooks_auto_accept` to anything but a plain `true`: Hermes
/// would then ask `[y/N]` about our hooks in the PTY, so they are not
/// written into a profile like that.
pub(crate) fn auto_accept_blocks(text: &str) -> bool {
    has_top_key(&Doc::new(text), AUTO) && auto_accept_value(text) != Some(true)
}

/// Line ranges `(begin, end)` of every marked region, markers included.
/// A BEGIN with no END before the next BEGIN is an edited marker, not the
/// start of a region: it is left out (and reported by [`has_stray_marker`]),
/// so a later region's END can never swallow the user's text in between.
fn regions(doc: &Doc<'_>) -> Vec<(usize, usize)> {
    let n = doc.lines.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if doc.content(i).trim() == BEGIN {
            let stop = (i + 1..n).find(|&j| {
                let c = doc.content(j).trim();
                c == END || c == BEGIN
            });
            if let Some(e) = stop
                && doc.content(e).trim() == END
            {
                out.push((i, e));
                i = e;
            }
        }
        i += 1;
    }
    out
}

/// A BEGIN marker that opens no region (its END is gone or edited), or an
/// END with no BEGIN: Skein cannot tell where its text stops, so it refuses
/// to edit rather than guess.
fn has_stray_marker(doc: &Doc<'_>) -> bool {
    let paired: usize = regions(doc).len() * 2;
    let markers = (0..doc.lines.len())
        .filter(|&i| {
            let c = doc.content(i).trim();
            c == BEGIN || c == END
        })
        .count();
    markers != paired
}

/// Whether every event carries Skein's hook for exactly `spec`.
pub(crate) fn hooks_present(text: &str, spec: &HookSpec) -> bool {
    let doc = Doc::new(text);
    let regs = regions(&doc);
    HOOK_EVENTS.iter().all(|ev| {
        let Ok(items) = item_lines("", spec, ev) else {
            return false;
        };
        regs.iter().any(|&(b, e)| {
            (b + 1..e).any(|k| {
                k + 1 < e
                    && doc.content(k).trim() == items[0].trim()
                    && doc.content(k + 1).trim() == items[1].trim()
            })
        })
    })
}

/// Remove every marked region, markers included. A region that opened
/// the top-level `hooks:` key keeps that line when the user has since
/// added children under it. Text without a region comes back unchanged.
pub(crate) fn remove_hooks(text: &str) -> String {
    let doc = Doc::new(text);
    let regs = regions(&doc);
    let Some(&(_, last_e)) = regs.last() else {
        return text.to_string();
    };
    let n = doc.lines.len();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut last_orphaned = false;
    for &(b, e) in &regs {
        let hooks_line = (b + 1..e).take(2).find(|&k| {
            key_rest(doc.content(k), KEY).is_some_and(|r| is_blank(r) || is_comment(r))
                && (k == b + 1 || doc.content(b + 1).trim_start().starts_with(AUTO))
        });
        let orphaned = indent_of(doc.content(b)).is_empty()
            && hooks_line.is_some()
            && (e + 1..n)
                .map(|j| doc.content(j))
                .find(|c| !is_blank(c) && !is_comment(c))
                .is_some_and(|c| !indent_of(c).is_empty());
        last_orphaned = orphaned;
        match hooks_line {
            Some(k) if orphaned => {
                ranges.push((doc.lines[b].start, doc.lines[k].start));
                ranges.push((doc.lines[k].end, doc.lines[e].end));
            }
            _ => ranges.push((doc.lines[b].start, doc.lines[e].end)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (s, end) in ranges {
        out.push_str(&text[pos..s]);
        pos = end;
    }
    out.push_str(&text[pos..]);
    // The block ended the file without a newline: the newline we put
    // before it goes too.
    if !doc.has_terminator(last_e) && !last_orphaned && out.ends_with('\n') {
        out.pop();
        if out.ends_with('\r') {
            out.pop();
        }
    }
    out
}

/// Where an existing event's sequence takes a new item.
struct SeqLoc {
    insert_at: usize,
    indent: String,
}

fn locate_event(
    doc: &Doc<'_>,
    top: &super::yaml::Top,
    ci: &str,
    unit: &str,
    ev: &str,
) -> Result<Option<SeqLoc>, YamlError> {
    let mut hit = None;
    for &j in &top.child_lines {
        let body = doc.content(j).trim_start();
        if quoted_key(body, ev) {
            return refuse(&format!("hooks.{ev} is written as a quoted key"));
        }
        if let Some(rest) = key_rest(body, ev) {
            hit = Some((j, rest.trim()));
            break;
        }
    }
    let Some((j, rest)) = hit else {
        return Ok(None);
    };
    if !(rest.is_empty() || rest.starts_with('#')) {
        return refuse(&if rest.starts_with(['{', '[']) {
            format!("hooks.{ev} is written in flow style ({{...}} or [...])")
        } else if rest.starts_with(['&', '*', '!']) {
            format!("hooks.{ev} uses a YAML anchor, alias or tag")
        } else {
            format!("hooks.{ev} is not a list of hooks")
        });
    }
    let mut seq: Option<(String, usize)> = None;
    for k in j + 1..=top.last_content.unwrap_or(j) {
        let c = doc.content(k);
        if is_blank(c) || is_comment(c) {
            continue;
        }
        let ind = indent_of(c);
        let body = &c[ind.len()..];
        let is_item = body == "-" || body.starts_with("- ") || body.starts_with("-\t");
        if ind.len() < ci.len() || (ind.len() == ci.len() && !is_item) {
            break;
        }
        match &mut seq {
            None if !is_item => return refuse(&format!("hooks.{ev} holds a mapping, not a list")),
            None => seq = Some((ind.to_string(), k)),
            Some((_, last)) => *last = k,
        }
    }
    Ok(Some(match seq {
        Some((indent, last)) => SeqLoc {
            insert_at: doc.lines[last].end,
            indent,
        },
        None => SeqLoc {
            insert_at: doc.lines[j].end,
            indent: format!("{ci}{unit}"),
        },
    }))
}

fn marked(ind: &str, body: Vec<String>) -> Vec<String> {
    let mut lines = vec![format!("{ind}{BEGIN}")];
    lines.extend(body);
    lines.push(format!("{ind}{END}"));
    lines
}

fn insert_all(base: &str, spec: &HookSpec, auto_accept: bool) -> Result<String, YamlError> {
    let probe = Doc::new(base);
    check_single_document(&probe)?;
    if has_stray_marker(&probe) {
        return refuse("a Skein hooks marker comment was edited; fix or remove it by hand");
    }
    if !root_is_block_mapping(&probe) {
        return refuse("the config's top level is not a block mapping");
    }
    // Work on a copy that ends in a newline so every insertion is
    // "after a terminated line"; the newline is dropped again at the end.
    let unterminated = probe.lines.last().is_some_and(|l| l.end == l.content_end);
    let eol = probe.eol;
    let work = if unterminated {
        format!("{base}{eol}")
    } else {
        base.to_string()
    };
    let doc = Doc::new(&work);
    let want_auto = auto_accept && !has_top_key(&doc, AUTO);

    // (offset, order, lines): equal offsets apply in `order`.
    let mut edits: Vec<(usize, u8, Vec<String>)> = Vec::new();
    match find_top_key(&doc, KEY)? {
        None => edits.push((work.len(), 0, full_block(spec, want_auto)?)),
        Some(top) => {
            if matches!(top.value, Value::Null) {
                return refuse("hooks is explicitly null; edit it by hand or empty the value");
            }
            if top.block_scalar {
                return refuse("the hooks block contains a block scalar (| or >)");
            }
            let ci = top.child_indent.clone().unwrap_or_else(|| "  ".to_string());
            let unit = unit_for(&ci).to_string();
            let mut body = Vec::new();
            for ev in HOOK_EVENTS {
                match locate_event(&doc, &top, &ci, &unit, ev)? {
                    Some(loc) => {
                        let item = marked(&loc.indent, item_lines(&loc.indent, spec, ev)?);
                        edits.push((loc.insert_at, 0, item));
                    }
                    None => body.extend(event_block(&ci, &unit, ev, spec)?),
                }
            }
            if !body.is_empty() {
                let after = top.last_content.unwrap_or(top.line);
                edits.push((doc.lines[after].end, 1, marked(&ci, body)));
            }
            if want_auto {
                let lines = vec![BEGIN.to_string(), format!("{AUTO}: true"), END.to_string()];
                edits.push((work.len(), 2, lines));
            }
        }
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(work.len() + 1200);
    let mut pos = 0;
    for (at, _, lines) in edits {
        out.push_str(&work[pos..at]);
        out.push_str(&lines.join(eol));
        out.push_str(eol);
        pos = at;
    }
    out.push_str(&work[pos..]);
    if unterminated {
        out.truncate(out.len() - eol.len());
    }
    Ok(out)
}

/// The text with Skein's hooks (re)written for `spec`. `Ok(None)`: they
/// are already there, byte for byte. `auto_accept` adds
/// `hooks_auto_accept: true` only when the file has no such key; an
/// existing one is left alone whatever its value ([`auto_accept_value`]).
pub(crate) fn plan_hooks(
    text: &str,
    spec: &HookSpec,
    auto_accept: bool,
) -> Result<Option<String>, YamlError> {
    validate(spec)?;
    let base = remove_hooks(text);
    let out = insert_all(&base, spec, auto_accept)?;
    Ok((out != text).then_some(out))
}
