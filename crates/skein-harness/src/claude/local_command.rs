//! Telling a local slash command's own rows from rows that start a turn (#463).
//!
//! A *local* command (`/context`, `/model`, `/login`, `/skills`, `/clear`, …)
//! runs inside Claude Code and never reaches the model. On 2.1.270+ it writes a
//! `system` row `subtype: "local_command"` carrying the command name, a second
//! one carrying `<local-command-stdout>` (plus `commandRun`), and, from
//! 2.1.284, a `user` row with `isMeta: true` and a `promptId` whose content is
//! the PLAIN-TEXT output. No assistant row follows (0 of 39 bursts in a survey
//! of 695 main transcripts, 2.1.246–2.1.286). `<local-command-caveat>` isMeta
//! rows were never followed by one either, and builds up to 2.1.270 also wrote
//! user rows starting `<local-command-stdout>`.
//!
//! # Why position-based
//!
//! The 2.1.284+ output row is untagged plain text, so no content test can find
//! it. What identifies it is where it sits: after a `local_command` system row
//! and before anything that belongs to a real turn. [`LocalCommandTracker`]
//! therefore arms on the `local_command` row and treats following `isMeta`
//! user rows as that command's output until a real row disarms it.
//!
//! # Why not "every isMeta row"
//!
//! Peer and subagent-handback messages are `isMeta` too (`origin.kind ==
//! "peer"`, `promptSource: "system"`, `turnOrigin: "peer"`) and start about
//! 3,000 real turns, often right after `system/turn_duration`. `turnCompanion`
//! rows (`Base directory for this skill…`, `[Image: …]`) and compaction
//! `Continue from where…` rows sit in the middle of a turn. Compaction rows
//! (`isCompactSummary`, `This session is being continued…`, `Continue from
//! where…`) are `isMeta` but start a real turn, and `/compact` is itself a
//! command, so they never count as output even when armed. A custom command or
//! user-typed skill is a plain non-meta user row with `origin.kind: "human"`,
//! then an `isMeta` companion body, then assistant rows; it stays a turn start.
//!
//! # Why the error direction is safe
//!
//! Skipping a real prompt costs only until the first assistant row, which reads
//! as running anyway. A false prompt leaves the harness `running` forever and
//! holds its mail. So every ambiguous case resolves to "not a local command".
//!
//! # Known limit
//!
//! Builds before 2.1.270 recorded a local command as a non-meta user row
//! `<command-name>/model</command-name>…`, indistinguishable by shape from a
//! custom command's invocation row. Those are left alone.
//!
//! # Scope
//!
//! Main transcript only. Subagent sidecars keep #440's isMeta-reopens rule in
//! `SubagentLifecycle`.

use serde_json::Value;

/// Stateful classifier for the main transcript; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct LocalCommandTracker {
    armed: bool,
}

impl LocalCommandTracker {
    /// Whether a `local_command` row has been seen with nothing real since.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Feed every main-chain row in order. Returns true when the row is a
    /// local command's own output and must not be read as a turn start.
    pub fn observe(&mut self, row: &Value) -> bool {
        match row.get("type").and_then(Value::as_str) {
            Some("system") => {
                if row.get("subtype").and_then(Value::as_str) == Some("local_command") {
                    self.armed = true;
                    return true;
                }
                false
            }
            Some("user") => self.observe_user(row),
            Some("assistant") => {
                self.armed = false;
                false
            }
            _ => false,
        }
    }

    fn observe_user(&mut self, row: &Value) -> bool {
        if row.get("toolUseResult").is_some() || has_tool_result(row) {
            self.armed = false;
            return false;
        }
        if first_text(row).is_some_and(|t| {
            t.starts_with("<local-command-stdout>") || t.starts_with("<local-command-caveat>")
        }) {
            return true;
        }
        let meta = row.get("isMeta").and_then(Value::as_bool) == Some(true);
        let peer = row
            .pointer("/origin/kind")
            .and_then(Value::as_str)
            .is_some_and(|k| k == "peer");
        if self.armed && meta && !peer && !is_compaction(row) {
            return true;
        }
        self.armed = false;
        false
    }
}

/// A compaction summary or continuation row: `isMeta`, yet it starts a real
/// turn, and `/compact` is itself a command so it can follow a `local_command`.
fn is_compaction(row: &Value) -> bool {
    row.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        || first_text(row).is_some_and(|t| {
            t.starts_with("This session is being continued") || t.starts_with("Continue from where")
        })
}

fn content_blocks(row: &Value) -> Option<&Vec<Value>> {
    row.pointer("/message/content").and_then(Value::as_array)
}

fn has_tool_result(row: &Value) -> bool {
    content_blocks(row).is_some_and(|b| {
        b.iter()
            .any(|x| x.get("type").and_then(Value::as_str) == Some("tool_result"))
    })
}

fn first_text(row: &Value) -> Option<&str> {
    let content = row.pointer("/message/content")?;
    if let Some(s) = content.as_str() {
        return Some(s);
    }
    content
        .as_array()?
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|b| b.get("text"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sys(subtype: &str) -> Value {
        json!({"type": "system", "subtype": subtype, "content": "x"})
    }
    fn user(text: &str, meta: bool) -> Value {
        json!({"type": "user", "isMeta": meta, "message": {"role": "user", "content": text}})
    }
    fn peer() -> Value {
        json!({"type": "user", "isMeta": true, "origin": {"kind": "peer"},
               "message": {"role": "user", "content": "Another session says hi"}})
    }
    fn assistant() -> Value {
        json!({"type": "assistant", "message": {"role": "assistant", "content": []}})
    }
    fn tool_result() -> Value {
        json!({"type": "user", "toolUseResult": {},
               "message": {"role": "user", "content": [{"type": "tool_result", "content": "ok"}]}})
    }

    #[test]
    fn context_sequence_is_all_output_then_prompt_is_a_turn() {
        let mut t = LocalCommandTracker::default();
        assert!(t.observe(&sys("local_command")));
        assert!(t.observe(&sys("local_command")));
        assert!(t.observe(&user("Context Usage\n  plain text", true)));
        assert!(!t.observe(&user("a real prompt", false)));
        assert!(!t.is_armed());
    }

    #[test]
    fn caveat_row_is_output() {
        let mut t = LocalCommandTracker::default();
        assert!(t.observe(&user(
            "<local-command-caveat>Caveat</local-command-caveat>",
            true
        )));
    }

    #[test]
    fn legacy_stdout_user_row_is_output() {
        let mut t = LocalCommandTracker::default();
        let row = json!({"type": "user", "message": {"role": "user",
            "content": [{"type": "text", "text": "<local-command-stdout>hi</local-command-stdout>"}]}});
        assert!(t.observe(&row));
        assert!(!t.is_armed());
    }

    #[test]
    fn peer_message_after_local_command_is_a_turn_start() {
        let mut t = LocalCommandTracker::default();
        t.observe(&sys("local_command"));
        t.observe(&sys("local_command"));
        assert!(!t.observe(&peer()));
        assert!(!t.is_armed());
    }

    #[test]
    fn custom_command_and_companion_are_turn_rows() {
        let mut t = LocalCommandTracker::default();
        let invoke = json!({"type": "user", "origin": {"kind": "human"}, "message": {"role": "user",
            "content": "<command-message>x</command-message><command-name>/x</command-name>"}});
        assert!(!t.observe(&invoke));
        assert!(!t.observe(&user("Base directory for this skill: x", true)));
        assert!(!t.observe(&assistant()));
    }

    #[test]
    fn skill_via_tool_result_then_companion_is_not_output() {
        let mut t = LocalCommandTracker::default();
        t.observe(&sys("local_command"));
        assert!(!t.observe(&tool_result()));
        assert!(!t.observe(&user("Base directory for this skill: x", true)));
    }

    #[test]
    fn meta_row_when_never_armed_is_not_output() {
        let mut t = LocalCommandTracker::default();
        assert!(!t.observe(&user("Continue from where you left off", true)));
    }

    #[test]
    fn unrelated_rows_keep_armed() {
        let mut t = LocalCommandTracker::default();
        t.observe(&sys("local_command"));
        assert!(!t.observe(&sys("away_summary")));
        assert!(!t.observe(&sys("turn_duration")));
        assert!(!t.observe(&json!({"type": "attachment"})));
        assert!(t.observe(&user("plain output", true)));
        assert!(t.is_armed());
    }

    #[test]
    fn compaction_meta_row_after_local_command_is_a_turn_start() {
        let mut t = LocalCommandTracker::default();
        t.observe(&sys("local_command"));
        t.observe(&user("Context Usage\n  plain text", true));
        let summary = json!({"type": "user", "isMeta": true, "isCompactSummary": true,
            "message": {"role": "user", "content": "Summary of the work so far"}});
        assert!(!t.observe(&summary));
        assert!(!t.is_armed());

        t.observe(&sys("local_command"));
        assert!(!t.observe(&user("Continue from where you left off.", true)));
        assert!(!t.is_armed());

        t.observe(&sys("local_command"));
        assert!(!t.observe(&user(
            "This session is being continued from a previous one.",
            true
        )));
        assert!(!t.is_armed());
    }

    #[test]
    fn assistant_disarms() {
        let mut t = LocalCommandTracker::default();
        t.observe(&sys("local_command"));
        assert!(!t.observe(&assistant()));
        assert!(!t.observe(&user("late meta", true)));
    }
}
