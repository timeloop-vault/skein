//! Row-timestamp ordering of one tick's events across the main transcript
//! and the subagent transcripts (#634).
//!
//! Each file read in a tick is its own [`Stream`]. Appending the streams
//! one after another made every main-file event precede every subagent
//! event whatever the rows' timestamps, so a subagent re-open stamped
//! before the main `end_turn` was delivered after it. [`merge_streams`]
//! interleaves them by row timestamp instead, without ever reordering
//! events within one stream.

use super::ClaudeEvent;

/// Sort key for an event whose stream has no parsable timestamp at all:
/// it sorts after every stamped event, which is where it sat before the
/// merge existed (main first, subagents after).
const UNSTAMPED: i64 = i64::MAX;

/// One file's events for a tick, in file order, plus the row boundaries.
#[derive(Default)]
pub(super) struct Stream {
    pub(super) events: Vec<ClaudeEvent>,
    /// `(events.len() after the row, the row's timestamp)`, one per row
    /// processed, in order.
    marks: Vec<(usize, Option<i64>)>,
}

impl Stream {
    /// Events with no row behind them: unstamped, so they sort last.
    pub(super) fn unstamped(events: Vec<ClaudeEvent>) -> Self {
        Self {
            events,
            marks: Vec::new(),
        }
    }

    /// Close the current row: every event pushed since the previous call
    /// belongs to a row stamped `ts` (`None` when it had no timestamp).
    pub(super) fn end_row(&mut self, ts: Option<i64>) {
        self.marks.push((self.events.len(), ts));
    }

    /// Per-event timestamps, parallel to `events`. An event whose row has
    /// no timestamp inherits the previous one in this stream; leading
    /// events with none (and events pushed before any row, such as a
    /// first-sighting `SubagentStart`) take the first known timestamp of
    /// the stream, so they stay in place at its front; a stream with no
    /// timestamp anywhere is [`UNSTAMPED`].
    fn stamps(&self) -> Vec<i64> {
        let mut raw: Vec<Option<i64>> = Vec::with_capacity(self.events.len());
        let mut marks = self.marks.iter().peekable();
        for i in 0..self.events.len() {
            while marks.next_if(|(end, _)| *end <= i).is_some() {}
            // Events past the last mark (pushed outside a row) fall
            // through to the forward fill below.
            raw.push(marks.peek().and_then(|(_, ts)| *ts));
        }
        let first_known = raw.iter().flatten().next().copied();
        let mut prev = first_known.unwrap_or(UNSTAMPED);
        raw.into_iter()
            .map(|t| {
                if let Some(t) = t {
                    prev = t;
                }
                prev
            })
            .collect()
    }
}

/// Stable k-way merge by row timestamp. Within a stream the order never
/// changes (even if its timestamps are not monotonic); across streams the
/// smallest head timestamp goes first, ties going to the earlier stream
/// (callers pass main first, then subagents in iteration order). Returns
/// each event with the timestamp it was sorted under.
pub(super) fn merge_streams(streams: Vec<Stream>) -> Vec<(i64, ClaudeEvent)> {
    let total = streams.iter().map(|s| s.events.len()).sum();
    let mut heads: Vec<std::iter::Peekable<std::vec::IntoIter<(i64, ClaudeEvent)>>> = streams
        .into_iter()
        .map(|s| {
            let stamps = s.stamps();
            stamps
                .into_iter()
                .zip(s.events)
                .collect::<Vec<_>>()
                .into_iter()
                .peekable()
        })
        .collect();
    let mut out = Vec::with_capacity(total);
    loop {
        let mut best: Option<(usize, i64)> = None;
        for (i, it) in heads.iter_mut().enumerate() {
            if let Some((ts, _)) = it.peek()
                && best.is_none_or(|(_, b)| *ts < b)
            {
                best = Some((i, *ts));
            }
        }
        let Some((i, _)) = best else { break };
        if let Some(item) = heads[i].next() {
            out.push(item);
        }
    }
    out
}

/// `kind@timestamp,…` for the tick's debug line, in emitted order.
pub(super) fn describe(stamped: &[(i64, ClaudeEvent)]) -> String {
    stamped
        .iter()
        .map(|(ts, e)| {
            let kind = serde_json::to_value(e)
                .ok()
                .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
                .unwrap_or_else(|| "?".to_owned());
            if *ts == UNSTAMPED {
                format!("{kind}@-")
            } else {
                format!("{kind}@{ts}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
