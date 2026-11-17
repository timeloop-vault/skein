//! `api_usage` rows: one per API response, with its final usage (#341).
//!
//! The payload builder is shared by the main transcript's extractor and the
//! subagent transcripts, so both write the same shape.

use serde_json::{Value, json};
use skein_harness::claude::AssistantRow;

use super::ExtractedAction;
use crate::db::action_kind;

/// The `api_usage` action for one folded response. `agent` is
/// `(agent_id, agent_type)` for a subagent's response; it adds `agent_id` and
/// `agent_type` to the payload. `source` is `message.id`, else the dedupe
/// key; `None` when the response identifies neither (such a row is not
/// deduped by the unique index).
pub(crate) fn api_usage_action(
    row: &AssistantRow,
    agent: Option<(&str, Option<&str>)>,
) -> ExtractedAction {
    let mut payload = json!({
        "model": row.model,
        "stop_reason": row.stop_reason,
        "request_id": row.request_id,
        "message_id": row.message_id,
        "usage": row.usage,
    });
    if let (Some((id, ty)), Some(map)) = (agent, payload.as_object_mut()) {
        map.insert("agent_id".into(), Value::from(id));
        map.insert("agent_type".into(), ty.map_or(Value::Null, Value::from));
    }
    ExtractedAction {
        kind: action_kind::API_USAGE,
        timestamp_ms: row.timestamp_ms.unwrap_or(0),
        payload: payload.to_string(),
        source: row.message_id.clone().or_else(|| row.dedupe_key()),
    }
}
