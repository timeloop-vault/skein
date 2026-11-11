//! One transcript row to events: initial-state derivation and the per-row parser.
#[cfg(test)]
use super::background_tasks::BackgroundState;
#[cfg(test)]
use super::persist::scan_history;

use super::ClaudeEvent;
use skein_harness::claude::background;
use skein_harness::claude::local_command::LocalCommandTracker;

/// Scan a JSONL session log (entire content) and return the event
/// that represents the current phase. Used on adapter attach to
/// handle `--resume`: a session that ended with `end_turn` already
/// in the file needs to start in `waiting`, not `running`. Without
/// this, every Claude harness shows green on Skein restart until
/// the user types something — Claude doesn't write any new row on
/// resume, so the watcher never has a transition to observe.
///
/// Returns:
/// - `Some(AwaitingPrompt)` if the last assistant row had a terminal
///   `stop_reason` (`end_turn` / `stop_sequence` / `max_tokens`), or
///   the turn was last ended by an interrupt or a `turn_duration` row
///   (#260, see [`ends_turn_without_stop_reason`]).
/// - `Some(AssistantTurn)` if the last assistant row was non-terminal
///   (`tool_use`) — session ended mid-turn, `claude --resume` will
///   pick it up; treat as running so the dot doesn't immediately
///   flip blue (the waiting indicator).
/// - `Some(UserPrompt)` / `Some(ToolUseResult)` if the last event
///   was a user/tool row — Claude was in the middle of consuming
///   input; running.
/// - `None` if there's nothing meaningful to derive state from
///   (empty file, only metadata rows). The first PTY chunk that
///   arrives will flip to running via the normal path.
///
/// Production code no longer calls this directly — `attach_at` uses
/// `scan_history`, which drives the same per-row logic
/// (`apply_initial_state_row`) from one walk shared with the action
/// extractor (#171e) instead of two. Kept as a `#[cfg(test)]` helper,
/// delegating to `scan_history` (with no `ActionPersistence`, so the
/// action side of its walk is a no-op) rather than re-implementing the
/// line walk, so the standalone "feed it a whole log, check the
/// derived phase" tests in `parse_tests.rs` stay simple to write.
#[cfg(test)]
pub(super) fn determine_initial_state(content: &str) -> Option<ClaudeEvent> {
    scan_history(
        content,
        None,
        &mut BackgroundState::default(),
        &mut LocalCommandTracker::default(),
    )
    .0
}

/// One row's worth of `determine_initial_state`'s logic, factored out
/// so `scan_history` can drive it from the same parsed `Value` its
/// action extractor already walked, instead of `determine_initial_state`
/// and the action scan each re-parsing every line (#171e). Mutates
/// `last` in place — same "last relevant row wins" rule the doc comment
/// on `determine_initial_state` describes.
pub(super) fn apply_initial_state_row(
    value: &serde_json::Value,
    last: &mut Option<ClaudeEvent>,
    local_command: &mut LocalCommandTracker,
) {
    if skein_harness::claude::is_sidechain(value) {
        return;
    }
    let Some(ty) = value.get("type").and_then(serde_json::Value::as_str) else {
        return;
    };
    // #440: same skip as the live parser.
    if skein_harness::claude::is_queue_transcript_only(value) {
        return;
    }
    // #463: a local slash command's own rows are not a turn start; see
    // the `local_command` module doc.
    if local_command.observe(value) {
        return;
    }
    // #260: same rule as the live parser, or an interrupted session
    // resumes into running on every restart.
    if ends_turn_without_stop_reason(ty, value) {
        *last = Some(ClaudeEvent::AwaitingPrompt);
        return;
    }
    match ty {
        "assistant" => {
            let stop_reason = value
                .get("message")
                .and_then(|m| m.get("stop_reason"))
                .and_then(serde_json::Value::as_str);
            if matches!(
                stop_reason,
                Some("end_turn" | "stop_sequence" | "max_tokens")
            ) {
                *last = Some(ClaudeEvent::AwaitingPrompt);
            } else {
                *last = Some(ClaudeEvent::AssistantTurn);
            }
        }
        "user" => {
            *last = if value.get("toolUseResult").is_some() {
                Some(ClaudeEvent::ToolUseResult)
            } else {
                Some(ClaudeEvent::UserPrompt {
                    task_notification: is_task_notification(value),
                })
            };
        }
        // Metadata rows don't shift phase; skip.
        _ => {}
    }
}

/// Is this `user` row a `<task-notification>` Claude Code wrote itself
/// rather than a prompt the user typed (#445)? By the row's text, via the
/// tracker's own carrier test, not by `origin.kind` /`promptSource`: the
/// text is what the tracker acts on, so the two can't disagree.
pub(super) fn is_task_notification(value: &serde_json::Value) -> bool {
    background::task_notification_text(value).is_some()
}

/// Text Claude writes as a plain `user` row when the user stops a turn:
/// `[Request interrupted by user]` mid-stream, `… for tool use]` during
/// a tool call. Prefix-matched so both, and any later suffix, count.
pub(super) const INTERRUPT_PREFIX: &str = "[Request interrupted by user";

/// Does this main-chain row end a turn that no terminal `stop_reason`
/// will end (#260)? Two shapes, verified against Claude Code 2.1.270:
///
/// - the interrupt row above. It is a `user` row with no
///   `toolUseResult`, so without this it reads as a fresh prompt and
///   the harness shows running until the next restart — and then again,
///   because the probe finds the same last row.
/// - `system` / `turn_duration`, written the moment any turn ends,
///   interrupted or not. A second signal, not a replacement for
///   `end_turn`: it is missing after some turns that did end cleanly.
pub(super) fn ends_turn_without_stop_reason(ty: &str, value: &serde_json::Value) -> bool {
    match ty {
        "user" => {
            if value.get("toolUseResult").is_some() {
                return false;
            }
            let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
                return false;
            };
            // The first text block, or the whole content when it is a
            // bare string — an interrupt row carries nothing else.
            let text = content.as_str().or_else(|| {
                content
                    .as_array()?
                    .iter()
                    .find_map(|b| b.get("text").and_then(serde_json::Value::as_str))
            });
            text.is_some_and(|t| t.starts_with(INTERRUPT_PREFIX))
        }
        "system" => {
            value.get("subtype").and_then(serde_json::Value::as_str) == Some("turn_duration")
        }
        _ => false,
    }
}

/// Parse one JSONL row into at most one `ClaudeEvent`. Returns `None`
/// for rows we don't surface (metadata, sub-agents, unknown types).
///
/// The authoritative "Claude is done, awaiting user" signal lives on
/// the `assistant` row as `message.stop_reason`. Observed values
/// across recent sessions:
///
/// - `"tool_use"` — Claude wants to call a tool; another assistant
///   row will follow once the tool result returns. Still running.
/// - `"end_turn"` — Claude finished its turn cleanly. Awaiting user.
/// - `"stop_sequence"` — hit a configured stop string. Awaiting user.
/// - `"max_tokens"` — hit the context limit mid-thought. Effectively
///   awaiting user (they need to /clear or continue manually).
///
/// `last-prompt` rows — despite the suggestive name — fire when
/// Claude *captures the user's new prompt* (the leaf uuid is for
/// retry/edit). They appear at the *start* of a Claude turn, not
/// the end. Treating them as "awaiting input" was the bug that
/// kept the dot green until the L2a 8 s idle timeout finally fired.
pub(super) fn parse_value(
    value: &serde_json::Value,
    in_assistant_turn: &mut bool,
    local_command: &mut LocalCommandTracker,
) -> Option<ClaudeEvent> {
    let ty = value.get("type")?.as_str()?;

    // Sub-agent rows carry isSidechain=true. The main session is
    // what reflects the user-facing harness state; sub-agents are
    // their own internal flow and shouldn't drive the dot.
    if skein_harness::claude::is_sidechain(value) {
        // Reset the turn flag if a sub-agent interrupts so the next
        // main-session assistant row starts a fresh turn.
        *in_assistant_turn = false;
        return None;
    }

    if skein_harness::claude::is_queue_transcript_only(value) {
        return None;
    }

    // #463: a local slash command's own rows are not a turn start, and
    // leave the turn flag alone; see the `local_command` module doc.
    if local_command.observe(value) {
        return None;
    }

    // #260: an interrupt or a turn_duration row ends the turn even
    // though no assistant row said so.
    if ends_turn_without_stop_reason(ty, value) {
        *in_assistant_turn = false;
        return Some(ClaudeEvent::AwaitingPrompt);
    }

    match ty {
        "assistant" => {
            let stop_reason = value
                .get("message")
                .and_then(|m| m.get("stop_reason"))
                .and_then(serde_json::Value::as_str);
            // Terminal stop reasons → turn is over → emit
            // AwaitingPrompt regardless of mid-turn coalescing
            // state. Non-terminal (tool_use, null, unknown) means
            // more rows will follow — emit AssistantTurn for the
            // first row of the turn, coalesce thereafter.
            let terminal = matches!(
                stop_reason,
                Some("end_turn" | "stop_sequence" | "max_tokens")
            );
            if terminal {
                *in_assistant_turn = false;
                return Some(ClaudeEvent::AwaitingPrompt);
            }

            // Non-terminal row. Look for a tool_use block — useful
            // for the L7 activity feed (which tool, which file).
            // Today the policy just needs "running"; tool_event
            // therefore only matters during a *coalesced* (mid-turn)
            // row, where AssistantTurn has already fired and we'd
            // otherwise emit nothing.
            let mut tool_event = None;
            if let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(serde_json::Value::as_array)
            {
                for block in content {
                    if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use")
                        && let Some(name) = block.get("name").and_then(serde_json::Value::as_str)
                    {
                        tool_event = Some(ClaudeEvent::ToolUseStart {
                            name: name.to_owned(),
                        });
                    }
                }
            }
            if *in_assistant_turn {
                // Mid-turn — AssistantTurn already fired for this
                // turn. Surface the tool call if there is one;
                // otherwise this row is redundant for the policy.
                tool_event
            } else {
                *in_assistant_turn = true;
                Some(ClaudeEvent::AssistantTurn)
            }
        }
        "user" => {
            *in_assistant_turn = false;
            // `user` rows with `toolUseResult` are tool replies; the
            // ones without are real user-typed prompts. The
            // distinction matters for the activity feed (L7) but
            // both keep the harness in `running` from the state
            // machine's perspective.
            if value.get("toolUseResult").is_some() {
                Some(ClaudeEvent::ToolUseResult)
            } else {
                Some(ClaudeEvent::UserPrompt {
                    task_notification: is_task_notification(value),
                })
            }
        }
        "attachment" => {
            // No turn-flag reset: an attachment doesn't end an
            // assistant turn (it's a user-side action between turns).
            Some(ClaudeEvent::Attachment)
        }
        // Everything else — `last-prompt` (user-prompt capture, see
        // doc above), `permission-mode`, `ai-title`, `pr-link`,
        // `system`, future row types — doesn't drive the activity
        // dot. Keep `in_assistant_turn` as-is so a metadata row in
        // the middle of a turn doesn't re-open the turn boundary.
        _ => None,
    }
}
