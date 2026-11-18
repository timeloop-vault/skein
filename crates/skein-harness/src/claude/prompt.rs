//! Telling a real user prompt from the other `user` rows (#723).
//!
//! Claude Code's `last-prompt` row is written once per API request, so a feed
//! built from it repeats the prompt for every request of a turn. The `user`
//! row is written once, when the prompt is submitted; this module decides
//! which `user` rows are that.

use serde_json::Value;

use super::{is_queue_transcript_only, is_sidechain};

/// Longest prompt text kept, in chars. Mirrors Claude's own `lastPrompt`.
const MAX_CHARS: usize = 200;

/// The text of a `user` row if it is a real prompt, else `None`.
///
/// `last-prompt` is per request (#723), so the prompt is read from the `user`
/// row instead. A row is a prompt when `origin.kind == "human"`, or when it
/// has no `origin` at all and its text does not start with `<` (cross-session
/// messages carry no `origin` and count; `<command-name>`,
/// `<local-command-stdout>`, `<bash-input>` and friends all start with `<`).
/// Any other `origin.kind` (`task-notification`, `peer`, ...) is not a prompt.
/// Never a prompt either: sidechain rows, `isMeta`, `queueTranscriptOnly` and
/// `isCompactSummary` rows, rows with a `toolUseResult` or a `tool_result`
/// block, and empty text.
///
/// The text is a string content as-is, or the `text` blocks of a list joined
/// with `\n`, cut to 200 chars with a trailing `…`.
#[must_use]
pub fn user_prompt_text(row: &Value) -> Option<String> {
    if row.get("type").and_then(Value::as_str) != Some("user")
        || is_sidechain(row)
        || is_queue_transcript_only(row)
        || flag(row, "isMeta")
        || flag(row, "isCompactSummary")
        || row.get("toolUseResult").is_some()
    {
        return None;
    }
    let content = row.pointer("/message/content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    match row.get("origin") {
        Some(origin) => {
            if origin.get("kind").and_then(Value::as_str) != Some("human") {
                return None;
            }
        }
        None => {
            if text.trim_start().starts_with('<') {
                return None;
            }
        }
    }
    Some(truncate(&text))
}

fn flag(row: &Value, key: &str) -> bool {
    row.get(key).and_then(Value::as_bool) == Some(true)
}

fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests;
