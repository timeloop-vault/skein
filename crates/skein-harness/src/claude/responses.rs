//! One row per API response, with its final usage.
//!
//! Claude Code writes one `assistant` row per streamed chunk of a
//! response, all sharing [`AssistantRow::dedupe_key`]. Every chunk
//! repeats the input and cache fields; `output_tokens` depends on the
//! version: older Claude Code writes a growing count (6, 6, 185), newer
//! repeats the final one. Keeping the first chunk therefore undercounts
//! output, and summing chunks overcounts everything.

use std::collections::HashSet;
use std::io;
use std::path::Path;

use super::{AssistantRow, Row, Usage, rows};

impl Usage {
    /// Field-wise maximum. Every field of a streamed chunk is either
    /// repeated or growing, so the max is the response's final value.
    pub fn max_with(&mut self, other: &Usage) {
        self.input = self.input.max(other.input);
        self.cache_creation = self.cache_creation.max(other.cache_creation);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.output = self.output.max(other.output);
        self.thinking = self.thinking.max(other.thinking);
        self.cache_creation_1h = self.cache_creation_1h.max(other.cache_creation_1h);
        self.cache_creation_5m = self.cache_creation_5m.max(other.cache_creation_5m);
    }
}

/// Incremental fold of assistant chunks into whole API responses.
///
/// THE rule for every consumer (whole-file summary, live tail, history
/// rebuild), so they cannot disagree. Measured on real transcripts: the
/// chunks of one response are written back to back, and EVERY chunk
/// carries the response's `stop_reason` (`tool_use, tool_use, tool_use`),
/// so a stop reason cannot mark the last chunk. The row after a
/// response's last chunk is always another row: `system`/`turn_duration`
/// after `end_turn`, the `user` tool result after `tool_use`, or end of
/// file (subagent transcripts).
///
/// So a response is the run of adjacent assistant rows with one
/// [`AssistantRow::dedupe_key`]; any other row closes it. Merge: identity
/// fields come from the first chunk (a `None` is filled from later
/// ones), `stop_reason` is the last `Some`, `timestamp_ms` the latest,
/// `usage` the field-wise max. A keyless row is its own response. Keys
/// already emitted are remembered, and a later straggler with one is
/// dropped, so a response is counted once, as the old global dedupe did.
#[derive(Debug, Default)]
pub struct ResponseFold {
    pending: Option<(Option<String>, AssistantRow)>,
    emitted: HashSet<String>,
}

impl ResponseFold {
    /// Feed the next row. Returns the response this row closed, if any
    /// (at most one).
    pub fn push(&mut self, row: &Row) -> Option<AssistantRow> {
        let Row::Assistant(a) = row else {
            return self.finish();
        };
        let key = a.dedupe_key();
        if let (Some(k), Some((Some(pk), pending))) = (&key, self.pending.as_mut())
            && k == pk
        {
            merge(pending, a);
            return None;
        }
        let closed = self.finish();
        if let Some(k) = &key
            && self.emitted.contains(k)
        {
            return closed;
        }
        self.pending = Some((key, a.clone()));
        closed
    }

    /// Close and return the pending response (end of input, subagent
    /// exit). The fold stays usable and keeps its emitted keys.
    pub fn finish(&mut self) -> Option<AssistantRow> {
        let (key, row) = self.pending.take()?;
        if let Some(k) = key {
            self.emitted.insert(k);
        }
        Some(row)
    }
}

fn merge(into: &mut AssistantRow, c: &AssistantRow) {
    fn fill(a: &mut Option<String>, b: Option<&String>) {
        if a.is_none() {
            a.clone_from(&b.cloned());
        }
    }
    fill(&mut into.session_id, c.session_id.as_ref());
    fill(&mut into.uuid, c.uuid.as_ref());
    fill(&mut into.parent_uuid, c.parent_uuid.as_ref());
    fill(&mut into.agent_id, c.agent_id.as_ref());
    fill(&mut into.request_id, c.request_id.as_ref());
    fill(&mut into.message_id, c.message_id.as_ref());
    fill(&mut into.model, c.model.as_ref());
    fill(&mut into.cwd, c.cwd.as_ref());
    into.is_sidechain |= c.is_sidechain;
    if c.stop_reason.is_some() {
        into.stop_reason.clone_from(&c.stop_reason);
    }
    into.timestamp_ms = into.timestamp_ms.max(c.timestamp_ms);
    into.usage.max_with(&c.usage);
}

/// Fold already-read rows into one [`AssistantRow`] per API response.
pub fn fold_responses<I: IntoIterator<Item = Row>>(rows: I) -> Vec<AssistantRow> {
    let mut fold = ResponseFold::default();
    let mut out: Vec<AssistantRow> = rows.into_iter().filter_map(|r| fold.push(&r)).collect();
    out.extend(fold.finish());
    out
}

/// One row per API response in a transcript, with its final usage.
pub fn responses(path: &Path) -> io::Result<impl Iterator<Item = AssistantRow>> {
    let mut fold = ResponseFold::default();
    let mut inner = rows(path)?;
    let mut done = false;
    Ok(std::iter::from_fn(move || {
        if done {
            return None;
        }
        for row in inner.by_ref() {
            if let Some(r) = fold.push(&row) {
                return Some(r);
            }
        }
        done = true;
        fold.finish()
    }))
}

#[cfg(test)]
mod tests;
