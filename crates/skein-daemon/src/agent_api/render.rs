//! Rendering helpers for the review verbs: a hunk or file as unified-diff
//! text, and today's lines around a comment. Split out of `verbs.rs` (#455).

use std::fmt::Write as _;

use skein_review::{FileState, Hunk, LineKind};

use crate::review::abs_path;

/// How many lines of today's file to show either side of a comment.
const CONTEXT_RADIUS: usize = 6;

/// The hunk whose new-side range covers `line`.
pub(super) fn covering_hunk(hunks: &[Hunk], line: usize) -> Option<&Hunk> {
    hunks
        .iter()
        .find(|h| line >= h.new_start && line < h.new_start + h.new_lines)
}

/// `line_start..=line_end` in the file on disk, with a few lines either
/// side and 1-based numbers, so the agent sees what is there now and
/// not only what was there when the comment was written.
pub(super) fn read_around(cwd: &str, path: &str, start: usize, end: usize) -> Option<String> {
    let FileState::Text(text) = skein_review::read_state(&abs_path(cwd, path)) else {
        return None;
    };
    let lines: Vec<&str> = text.lines().collect();
    let from = start.saturating_sub(CONTEXT_RADIUS).max(1);
    let to = (end + CONTEXT_RADIUS).min(lines.len());
    if from > to || from > lines.len() {
        return None;
    }
    let mut out = String::new();
    for (offset, line) in lines[from - 1..to].iter().enumerate() {
        let n = from + offset;
        let marker = if n >= start && n <= end { '>' } else { ' ' };
        let _ = writeln!(out, "{marker} {n:>5} | {line}");
    }
    Some(out)
}

/// One file as a unified diff, the way `git diff` would print it.
///
/// Text, not the [`Hunk`] tree the pane renders: an agent reads a patch
/// natively and would have to reconstruct one from the JSON anyway.
pub(super) fn render_file(path: &str, blocked: Option<&'static str>, hunks: &[Hunk]) -> String {
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    if let Some(reason) = blocked {
        let _ = writeln!(out, "(no line diff: {reason})");
        return out;
    }
    if hunks.is_empty() {
        out.push_str("(no changes in this scope)\n");
        return out;
    }
    for h in hunks {
        out.push_str(&render_hunk(h));
    }
    out
}

pub(super) fn render_hunk(h: &Hunk) -> String {
    let mut out = format!("{}\n", h.header);
    for l in &h.lines {
        let sign = match l.kind {
            LineKind::Add => '+',
            LineKind::Delete => '-',
            LineKind::Context => ' ',
        };
        out.push(sign);
        out.push_str(&l.content);
        out.push('\n');
    }
    out
}
