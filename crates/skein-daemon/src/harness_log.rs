//! The Tauri-free half of the `harness_events` / `harness_actions` log
//! commands: the context-size query and its wire shape.

use crate::db::{Database, LatestTurnCost};

/// A harness's current context size: what the model was handed on the
/// latest turn's last API response (#368). Not a cost, not a sum.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessContextSize {
    pub harness_id: String,
    pub tokens: u64,
    /// Epoch ms of the source row. 0 for opencode rows.
    pub timestamp_ms: i64,
}

/// Context tokens of one `turn_cost` row, by the harness kind that wrote
/// it. Only Claude (`usage`, the terminal response's own) and opencode v1
/// (`tokens`, per step) qualify; Hermes writes a cumulative delta and any
/// other kind is not a source. A row from before the kind column (#538)
/// is guessed from its payload shape.
pub(crate) fn context_tokens_of(row: &LatestTurnCost) -> Option<u64> {
    let payload: serde_json::Value = serde_json::from_str(&row.payload).ok()?;
    let claude = || {
        payload
            .get("usage")
            .and_then(skein_harness::claude::context_tokens)
    };
    let opencode = || {
        payload
            .get("tokens")
            .and_then(skein_harness::opencode::context_tokens)
    };
    match row.harness_kind.as_deref() {
        Some("claude") => claude(),
        Some("opencode") => opencode(),
        Some(_) => None,
        None => claude().or_else(opencode),
    }
}

/// # Errors
/// The database error, flattened to a string.
pub fn context_sizes(
    db: &Database,
    harness_ids: &[String],
) -> Result<Vec<HarnessContextSize>, String> {
    Ok(db
        .latest_turn_costs(harness_ids)?
        .into_iter()
        .filter_map(|row| {
            Some(HarnessContextSize {
                tokens: context_tokens_of(&row)?,
                harness_id: row.harness_id,
                timestamp_ms: row.timestamp_ms,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::fresh_db;

    fn put(db: &Database, h: &str, ts: i64, kind: Option<&str>, payload: &str) {
        db.record_harness_action(h, "r1", ts, "turn_cost", payload, None, kind)
            .unwrap();
    }

    #[test]
    fn sizes_use_newest_row_per_kind_and_skip_unusable_harnesses() {
        let (_dir, db) = fresh_db();
        put(
            &db,
            "c",
            5,
            Some("claude"),
            r#"{"usage":{"input_tokens":1}}"#,
        );
        put(
            &db,
            "c",
            9,
            Some("claude"),
            r#"{"usage":{"input_tokens":2,"cache_read_input_tokens":30,"cache_creation_input_tokens":4}}"#,
        );
        // opencode: timestamps all 0, so id order decides.
        put(&db, "o", 0, Some("opencode"), r#"{"tokens":{"input":900}}"#);
        put(
            &db,
            "o",
            0,
            Some("opencode"),
            r#"{"tokens":{"input":1,"cache":{"read":10,"write":5}}}"#,
        );
        put(
            &db,
            "h",
            7,
            Some("hermes"),
            r#"{"usage":{"input_tokens":50}}"#,
        );
        put(&db, "old", 3, None, r#"{"usage":{"input_tokens":8}}"#);
        db.record_harness_action("c", "r1", 99, "tool_call", "{}", None, Some("claude"))
            .unwrap();
        let ids: Vec<String> = ["c", "o", "h", "old", "none"].map(String::from).to_vec();
        let got = context_sizes(&db, &ids).unwrap();
        let sizes: Vec<_> = got
            .iter()
            .map(|s| (s.harness_id.as_str(), s.tokens, s.timestamp_ms))
            .collect();
        assert_eq!(sizes, vec![("c", 36, 9), ("o", 16, 0), ("old", 8, 3)]);
    }

    #[test]
    fn opencode_v2_turn_cost_payload_is_read() {
        let (_dir, db) = fresh_db();
        // The v2 extraction's own payload shape, newest by timestamp (#678).
        put(
            &db,
            "v2",
            1_795_006_725_929,
            Some("opencode"),
            r#"{"reason":"stop","tokens":{"input":100,"output":7,"reasoning":3,"cache":{"read":20,"write":5}},"cost":0.01}"#,
        );
        let got = context_sizes(&db, &["v2".to_owned()]).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tokens, 125);
        assert_eq!(got[0].timestamp_ms, 1_795_006_725_929);
    }
}
