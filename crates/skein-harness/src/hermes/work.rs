//! Outstanding work a Hermes session has on disk right now (#628):
//! background terminal processes and background `delegate_task` children.
//! Pure and read-only; the caller owns the connection (see
//! [`super::open`]) and supplies process liveness.
//!
//! Evidence: Hermes v0.21.7 on Windows, a real home recorded 2026-10-21.
//!
//! # Background processes — `<home>/processes.json`
//!
//! A JSON ARRAY of entries (Hermes' `process_registry_checkpoint.py`),
//! rewritten atomically; an entry is dropped when its process finishes.
//! `session_id` is the PROCESS id (`proc_…`), not a Hermes session;
//! `parent_session_id` is the Hermes session that started it. The
//! registry is process-global, so one profile's file can carry another
//! session's processes: always filter by `parent_session_id`. Absent,
//! BOM-prefixed, mid-write or garbage text reads as "none".
//!
//! Only a process that will WAKE the session counts as work (#628). In
//! v0.21.7 that is `notify_on_complete`; a default (notify off) process,
//! e.g. a dev server, never wakes the agent, so holding the turn for it
//! would be a lie. (`watch_patterns` also wakes, but only on a match that
//! may never come, so it is not counted.) The `processes.json` field
//! CANNOT tell them apart: `process_registry._track_started` checkpoints
//! at spawn, and `terminal_tool_background.py:197` sets
//! `notify_on_complete` only afterwards, with no rewrite until the next
//! registry event; the field is `false`/absent for a process that does
//! wake (observed). The reliable record is the `terminal` tool RESULT in
//! `state.db`'s `messages` (role `tool`): JSON with `"session_id":
//! "proc_…"` and `"notify_on_complete": true` exactly when notify was
//! kept (it is `false`/absent for default, and for a delegated child,
//! whose notice never reaches the parent). Recorded: default-notify run
//! → result without the key; `notify:true` run → `true`, and the parent
//! was woken by a `[IMPORTANT: Background process proc_… completed`
//! user message. [`notify_process_ids`] reads it; a missing table or
//! column means no notify information, so nothing counts.
//!
//! # Background delegations — `async_delegations` + `sessions`
//!
//! A delegation row is `running` until its owner writes a result. An
//! owner that died first is rewritten to `unknown` only by a later
//! Hermes run, so a `running` row whose `owner_pid` is dead is an
//! orphan and reads as ENDED (the liveness check is injected).
//!
//! There is NO precise row→child link: the row's `event_json`,
//! `result_json` and `task_json` carry the goal and a transcript path
//! keyed by `deleg_…`, never the child's session id (the hook-side
//! `child_session_id` of `subagent_start`/`stop` is the child's
//! `sessions.id`, verified against both). The child is a `sessions` row
//! with `parent_session_id` = the main session and `ended_at` NULL
//! while it runs. Its `source` is `tui`, NOT `subagent` (that is only
//! the hook's `platform`), but its title starts `Subagent: ` (goal,
//! truncated) in both recorded cases. So the rule is: when at least one
//! `running` row of the session has a live owner, its live children are
//! the unfinished sessions with that parent, that title (or `source =
//! 'subagent'`, for other versions) and `started_at` no earlier than the
//! earliest live row's `dispatched_at` (a child starts ~0.1-0.3 s after
//! it), which keeps a crashed older child that never got an `ended_at`
//! out. A compression child also has a parent but no such title.
//! `owner_pid` alone can be a recycled pid; `owner_started_at` could
//! tighten that and is not used yet.
//!
//! [`live_delegation_count`] counts the same rows without the child link,
//! for the "no live delegation left" edge that releases hook-started
//! subagents whose stop hook was lost.
//!
//! There is no `pid_alive` here: the crate forbids `unsafe` and has no
//! process dependency, so the app supplies it.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::Result;

/// A background terminal process of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct HermesProcess {
    /// Hermes' process id, `proc_…`.
    pub id: String,
    pub pid: Option<u32>,
    pub command: Option<String>,
    /// Epoch seconds, as Hermes writes it.
    pub started_at: Option<f64>,
}

#[derive(Deserialize)]
struct RawProcess {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    started_at: Option<f64>,
    #[serde(default)]
    parent_session_id: Option<String>,
}

/// The entries of `processes.json` text started by `session_id`. Any
/// unparseable text (or shape) is empty; unknown fields and entries
/// that fail to parse individually are ignored.
pub fn processes_for_session(json_text: &str, session_id: &str) -> Vec<HermesProcess> {
    let text = json_text.strip_prefix('\u{feff}').unwrap_or(json_text);
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|e| serde_json::from_value::<RawProcess>(e).ok())
        .filter(|p| p.parent_session_id.as_deref() == Some(session_id))
        .filter_map(|p| {
            Some(HermesProcess {
                id: p.session_id?,
                pid: p.pid,
                command: p.command,
                started_at: p.started_at,
            })
        })
        .collect()
}

/// [`processes_for_session`] over the file; absent or unreadable is empty.
pub fn read_processes(path: &Path, session_id: &str) -> Vec<HermesProcess> {
    std::fs::read_to_string(path)
        .map(|t| processes_for_session(&t, session_id))
        .unwrap_or_default()
}

/// Ids (`proc_…`) of the processes `session_id` started with
/// `notify_on_complete`, from the `terminal` tool results in `messages`.
/// Rows that are not JSON objects, lack a string `session_id` or have
/// anything but `notify_on_complete: true` are skipped.
pub fn notify_process_ids(conn: &Connection, session_id: &str) -> Result<HashSet<String>> {
    Ok(notify_process_ids_after(conn, session_id, 0)?.0)
}

/// [`notify_process_ids`] over only the `tool` rows with `messages.id`
/// greater than `after_id` (the integer primary key), so a poller reads
/// each row once. Returns the ids found in that range and the new cursor:
/// the highest row id seen, matching or not, or `None` when there were no
/// rows (or the table/columns are missing) and the cursor stays put.
pub fn notify_process_ids_after(
    conn: &Connection,
    session_id: &str,
    after_id: i64,
) -> Result<(HashSet<String>, Option<i64>)> {
    if !has_columns(conn, "messages", &["id", "session_id", "role", "content"])? {
        return Ok((HashSet::new(), None));
    }
    let mut stmt = conn.prepare(
        "SELECT id, CASE WHEN content LIKE '%notify_on_complete%' THEN content END \
         FROM messages WHERE session_id = ?1 AND role = 'tool' AND id > ?2",
    )?;
    let rows = stmt.query_map(params![session_id, after_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let mut out = HashSet::new();
    let mut cursor: Option<i64> = None;
    for row in rows {
        let (id, content) = row?;
        cursor = Some(cursor.map_or(id, |c| c.max(id)));
        let Some(Ok(Value::Object(o))) = content.map(|c| serde_json::from_str::<Value>(&c)) else {
            continue;
        };
        if o.get("notify_on_complete") == Some(&Value::Bool(true))
            && let Some(Value::String(id)) = o.get("session_id")
        {
            out.insert(id.clone());
        }
    }
    Ok((out, cursor))
}

/// A background delegation's live child session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesDelegation {
    /// `sessions.id` of the child — what the hooks call `child_session_id`.
    pub child_session_id: String,
    /// The goal when known (exact with one live delegation, otherwise the
    /// child's truncated title).
    pub description: Option<String>,
}

/// Whether `table` exists and has every one of `columns`. Old Hermes
/// versions lack `async_delegations` or some of its columns.
fn has_columns(conn: &Connection, table: &str, columns: &[&str]) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let have: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(columns.iter().all(|c| have.iter().any(|h| h == c)))
}

fn goal_of(task_json: Option<&str>) -> Option<String> {
    let v: Value = serde_json::from_str(task_json?).ok()?;
    v.get("goal")?.as_str().map(str::to_string)
}

/// How many of `session_id`'s `running` delegation rows have a live owner.
/// Unlike [`running_delegations`] this does not depend on the child-linking
/// heuristic, so it is the signal for "all background delegations are
/// done" (a hook-started subagent whose `subagent_stop` was lost can be
/// released when this falls to 0). A missing table or column is 0.
pub fn live_delegation_count(
    conn: &Connection,
    session_id: &str,
    pid_alive: impl Fn(u32) -> bool,
) -> Result<usize> {
    if !has_columns(
        conn,
        "async_delegations",
        &["parent_session_id", "origin_session", "state", "owner_pid"],
    )? {
        return Ok(0);
    }
    let mut stmt = conn.prepare(
        "SELECT owner_pid FROM async_delegations \
         WHERE state = 'running' AND (parent_session_id = ?1 OR origin_session = ?1)",
    )?;
    let pids: Vec<Option<i64>> = stmt
        .query_map(params![session_id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(pids
        .into_iter()
        .filter(|pid| {
            pid.and_then(|p| u32::try_from(p).ok())
                .is_some_and(&pid_alive)
        })
        .count())
}

/// The live children of `session_id`'s background delegations; see the
/// module docs for the linking rule. A missing table or column is empty.
pub fn running_delegations(
    conn: &Connection,
    session_id: &str,
    pid_alive: impl Fn(u32) -> bool,
) -> Result<Vec<HermesDelegation>> {
    if !has_columns(
        conn,
        "async_delegations",
        &[
            "parent_session_id",
            "origin_session",
            "state",
            "dispatched_at",
            "owner_pid",
            "task_json",
        ],
    )? || !has_columns(
        conn,
        "sessions",
        &[
            "id",
            "parent_session_id",
            "ended_at",
            "started_at",
            "title",
            "source",
        ],
    )? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT dispatched_at, owner_pid, task_json FROM async_delegations \
         WHERE state = 'running' AND (parent_session_id = ?1 OR origin_session = ?1)",
    )?;
    let rows: Vec<(f64, Option<i64>, Option<String>)> = stmt
        .query_map(params![session_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let live: Vec<_> = rows
        .into_iter()
        .filter(|(_, pid, _)| {
            pid.and_then(|p| u32::try_from(p).ok())
                .is_some_and(&pid_alive)
        })
        .collect();
    let Some(earliest) = live.iter().map(|r| r.0).reduce(f64::min) else {
        return Ok(Vec::new());
    };
    let sole_goal = match live.as_slice() {
        [only] => goal_of(only.2.as_deref()),
        _ => None,
    };

    let mut stmt = conn.prepare(
        "SELECT id, title FROM sessions \
         WHERE parent_session_id = ?1 AND ended_at IS NULL AND started_at >= ?2 \
         AND (source = 'subagent' OR title LIKE 'Subagent:%') ORDER BY started_at, id",
    )?;
    // The child starts after the dispatch; the second of slack is for
    // clock rounding, not a window to widen.
    let children = stmt.query_map(params![session_id, earliest - 1.0], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let mut out = Vec::new();
    for child in children {
        let (child_session_id, title) = child?;
        let description = sole_goal.clone().or_else(|| {
            title.map(|t| t.strip_prefix("Subagent:").unwrap_or(&t).trim().to_string())
        });
        out.push(HermesDelegation {
            child_session_id,
            description,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
