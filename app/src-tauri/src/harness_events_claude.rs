//! Claude Code event-stream adapter — epic #50 L2c-1.
//!
//! Claude writes every session event to a JSONL log at
//! `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`. Skein
//! pre-allocates `sessionId` via the `--session-id <uuid>` flag
//! (chapter 5), so we know the path without any snapshot-and-diff.
//!
//! This module tails that file and emits a small `ClaudeEvent` enum
//! the frontend translates into harness-activity phase transitions.
//! The authoritative "Claude is awaiting user input" signal is
//! `message.stop_reason` on an `assistant` row — `end_turn`,
//! `stop_sequence`, or `max_tokens` mean Claude is done and the next
//! event will be a user prompt. `tool_use` means there's more to
//! come. Reading this off the JSONL is shorter and sharper than the
//! L2b pattern-matching strategy and survives Claude TUI text
//! changes.
//!
//! Lifecycle: `ClaudeEventsManager::attach` starts watching for the
//! given (harnessId, sessionId, cwd). `detach` (or dropping the
//! manager) stops the watcher. The adapter is purely additive — if it
//! fails to attach, the harness falls back to the L2a idle heuristic
//! and nothing user-visible breaks.
//!
//! # Where things are
//!
//! | File | The question it answers |
//! |---|---|
//! | `paths.rs` | Where a session transcript lives on disk and which directories the tail must watch for it. |
//! | `adapter.rs` | Per-harness tail state: the adapter, its dead-tail bookkeeping and the registry that holds them. |
//! | `manager.rs` | `ClaudeEventsManager`, the public handle the Tauri commands drive: attach, detach, reattach. |
//! | `attach.rs` | Building an adapter from scratch and installing it, for a fresh attach and for a reattach. |
//! | `supervise.rs` | Dead-tail detection and automatic reattach, run from a background supervisor thread. |
//! | `resync.rs` | Noticing that a transcript reappeared or shrank under the tail, and recovering from it. |
//! | `tick.rs` | The main-transcript tail tick: read new bytes, parse rows, dispatch events, heartbeat. |
//! | `subagents.rs` | The subagent sidecar tail tick and the per-row subagent lifecycle helpers. |
//! | `background_tasks.rs` | Background-task state fed from every transcript row, and its reconciliation into events. |
//! | `persist.rs` | History scans and the batched writes of extracted actions into `harness_actions`. |
//! | `parse.rs` | One transcript row to events: initial-state derivation and the per-row parser. |

use serde::Serialize;
use skein_harness::claude::background::{BackgroundKind, OutcomeStatus};

mod adapter;
mod attach;
mod background_tasks;
mod manager;
mod parse;
mod paths;
mod persist;
mod resync;
mod subagents;
mod supervise;
mod tick;

pub use manager::{ClaudeEventsManager, ReattachOutcome};

/// Semantic events emitted to the frontend. The translator in
/// `harnessEvents.ts` maps these to phase calls. We deliberately keep
/// this slightly *richer* than what the state machine needs today —
/// L7 (cross-harness activity feed) will want `ToolUseStart` /
/// `ToolUseResult` for "h1b just used the Edit tool" lines.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaudeEvent {
    /// Start of a new assistant turn. We coalesce the many `assistant`
    /// rows that make up a single turn (one per streamed chunk) into a
    /// single start event — the policy layer doesn't need to see every
    /// chunk to know Claude is `running`.
    AssistantTurn,
    /// Claude initiated a tool call. Carries the tool name so the
    /// activity feed (L7) can display it.
    ToolUseStart { name: String },
    /// Tool finished and the result was appended to the session.
    ToolUseResult,
    /// User-authored message arrived (typed prompt, not a tool result).
    ///
    /// `task_notification` (#445) is true when the row is a
    /// `<task-notification>` Claude Code wrote itself (a background
    /// task's end, a Monitor line), not something the user typed — read
    /// with `skein_harness::claude::background::task_notification_text`,
    /// the same test the background tracker uses. The phase is the same
    /// either way (still `running`); the flag is for the consumer that
    /// needs to tell the two apart (#446).
    UserPrompt { task_notification: bool },
    /// Claude finished its turn and is awaiting the next user prompt.
    /// This is the "waiting on input" signal.
    AwaitingPrompt,
    /// User attached a file or pasted content.
    Attachment,
    /// Session log was deleted or otherwise vanished — fall back to
    /// L2a heuristics.
    SessionEnd,
    /// A subagent transcript began (or resumed): a new
    /// `agent-<id>.jsonl` appeared under the session's `subagents`
    /// dir, or one that had already finished got more rows after its
    /// terminal row (a follow-up delegation to the same id, which
    /// flips it live again). `agent_type`/`description` come from the
    /// `agent-<id>.meta.json` sidecar when it exists and parses;
    /// `None` otherwise. `initial` is `true` only for the batch seeded
    /// from disk at attach time (`attach_at`'s `initial_subagent_starts`)
    /// — a transcript that already existed, with no terminal
    /// `stop_reason`, belongs to a Claude process that died with the
    /// PTY it ran in (PTYs die with Skein; `claude_events_attach` runs
    /// once per spawn), so it can never still be running. `false` is
    /// everything a live watcher tick discovers afterwards, which can
    /// be. #277 needs the distinction to avoid deferring a harness's
    /// "done" notification for the ceiling's whole duration after
    /// every restart on account of subagents that are already dead.
    SubagentStart {
        agent_id: String,
        agent_type: Option<String>,
        description: Option<String>,
        initial: bool,
    },
    /// A tool call inside a subagent's own turn just returned a
    /// result. This exists so a permission dialog a *subagent* opened
    /// can be cleared (epic #298 — today the badge stays stuck until
    /// the whole subagent finishes). It must never be read as a
    /// parent-harness phase change: it says nothing about the main
    /// session's own state.
    SubagentToolResult { agent_id: String },
    /// A subagent's transcript ended — an assistant row with a terminal
    /// `stop_reason`, or (#440) the `tool_result` of its `SubagentHandback`
    /// call (see `skein_harness::claude::SubagentLifecycle`). A subagent
    /// has no user to await, so the end of its turn IS its exit. The
    /// handback's result, not its `tool_use`, is the exit: only then is
    /// the report delivered, and Claude marks that row `toolEndsTurn`.
    SubagentEnd {
        agent_id: String,
        agent_type: Option<String>,
        description: Option<String>,
    },
    /// A background `Bash` / `PowerShell` command or `Monitor` started
    /// (#445, slice S2 of #439): its `tool_use` joined to a `tool_result`
    /// carrying the task id (`skein_harness::claude::background`).
    /// `agent_id` names the subagent that launched it, `None` for the
    /// main session. `initial` mirrors `SubagentStart.initial`: true
    /// only for the batch re-derived from disk at attach time, where a
    /// task still outstanding belongs to a process that died with the
    /// PTY it ran in; false for everything a live tick discovers.
    BackgroundStart {
        task_id: String,
        tool_use_id: String,
        task_kind: BackgroundKind,
        description: Option<String>,
        command: Option<String>,
        timeout_ms: Option<u64>,
        persistent: bool,
        auto_backgrounded: bool,
        agent_id: Option<String>,
        initial: bool,
    },
    /// A background task ended: a terminal notification, a `TaskStop`
    /// result, a Monitor deadline the adapter swept, or (status
    /// `subagent_ended`) the launching subagent exiting, in which case
    /// Skein can't know whether it finished.
    BackgroundEnd {
        task_id: String,
        task_kind: BackgroundKind,
        agent_id: Option<String>,
        status: OutcomeStatus,
        exit_code: Option<i32>,
    },
}

#[derive(Debug)]
pub struct ClaudeEventsError(pub String);

impl std::fmt::Display for ClaudeEventsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClaudeEventsError {}

impl ClaudeEventsError {
    fn from_err<E: std::fmt::Display>(e: E) -> Self {
        Self(e.to_string())
    }
}

#[cfg(test)]
mod background_resync_tests;
#[cfg(test)]
mod background_tests;
#[cfg(test)]
mod concurrent_tests;
#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod reattach_tests;
#[cfg(test)]
mod resync_tests;
#[cfg(test)]
mod subagent_tail_tests;
#[cfg(test)]
mod subagent_tests;
#[cfg(test)]
mod tail_tests;
#[cfg(test)]
mod tests;
