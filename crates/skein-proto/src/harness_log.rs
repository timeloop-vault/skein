//! Rows of the `harness_events` and `harness_actions` logs, as they cross the wire.

use serde::{Deserialize, Serialize};

/// One row in the `harness_events` append-only log. Epic #50 L6.
///
/// Stored fields are intentionally minimal — phase strings come from
/// the TS `ActivityPhase` union (`spawning` / `running` / `idle` /
/// `waiting` / `exited`) but we don't enforce a check constraint
/// here; future phases would just become new string values. The
/// `source` field is free-form text for v1 (e.g. `"l2c1-claude"`,
/// `"l2a-idle"`, `"pty-exit"`), reserved for L7 attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessEvent {
    pub id: i64,
    pub harness_id: String,
    pub room_id: String,
    pub from_phase: String,
    pub to_phase: String,
    /// Epoch milliseconds.
    pub timestamp_ms: i64,
    pub has_user_input: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
}

/// One row in the `harness_actions` append-only log. Issue #80.
///
/// Sibling of [`HarnessEvent`] — that table tracks phase transitions
/// (a tight finite state machine); this one tracks everything else
/// (tool calls, plan changes, patches, …). They join naturally on
/// `(harness_id, timestamp_ms)` if a unified room timeline is ever
/// needed.
///
/// `kind` is a free-form string. The v1 vocabulary lives in
/// [`action_kind`] as constants — adapters write rows with those
/// values, consumers compare against them. Adding a new kind is zero
/// schema work: a new `pub const` here, populate it in the adapter.
///
/// `payload` is an opaque JSON string. Shape varies per kind; the
/// canonical shape per kind is documented in
/// `docs/live-context-design-brief.md` §3. The DB layer stores +
/// returns it verbatim — no parsing or validation here.
///
/// `source` carries the adapter event id that produced this row
/// (mirrors the L7a `source` column on `harness_events`). Reserved
/// for cross-room / cross-harness correlation in later issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessAction {
    pub id: i64,
    pub harness_id: String,
    pub room_id: String,
    /// Epoch milliseconds.
    pub timestamp_ms: i64,
    pub kind: String,
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    /// The `HarnessKind` string of the harness that wrote the row (#538),
    /// so a feed row keeps its chip after the harness is closed. `None`
    /// for rows written before the column existed.
    #[serde(default)]
    pub harness_kind: Option<String>,
}
