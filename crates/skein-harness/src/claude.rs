//! Claude Code's on-disk session store.
//!
//! Layout, verified against real `~/.claude/projects` directories on
//! macOS and Windows:
//!
//! ```text
//! ~/.claude/projects/<encoded-cwd>/<session-id>.jsonl
//! ~/.claude/projects/<encoded-cwd>/<session-id>/subagents/agent-<agent-id>.jsonl
//! ~/.claude/projects/<encoded-cwd>/<session-id>/subagents/agent-<agent-id>.meta.json
//! ```
//!
//! The transcript is append-only JSONL, one row per event. Rows we
//! type here:
//!
//! - `assistant` — one API response (or one streamed chunk of it).
//!   Carries `message.model`, `message.usage` (input / cache write /
//!   cache read / output / thinking tokens) and `requestId`. The same
//!   `message.id` can appear on several rows of one streamed turn;
//!   see [`AssistantRow::dedupe_key`].
//! - `user` — a typed prompt or a tool result. When the result is
//!   the `Agent` tool launching a subagent, `toolUseResult.agentId`
//!   names the subagent transcript it will write to.
//! - `cost-state` — Claude's own cumulative roll-up for the session
//!   (since 2.1.246): total USD, per-model tokens and cost, durations,
//!   lines changed. Rewritten a handful of times per session; the
//!   newest row wins, they must never be summed.
//!
//! Subagent transcripts use the same row shapes with
//! `isSidechain: true` and an `agentId`. The main session's rows
//! never carry `isSidechain: true`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::time::parse_iso8601_ms;

// ── paths ─────────────────────────────────────────────────────────

/// Claude's project-directory encoding: every path separator and the
/// Windows drive colon become `-`. `C:\git\skein` → `C--git-skein`,
/// `/Users/foo/bar` → `-Users-foo-bar`, `D:\` → `D--`. The encoding is
/// lossy (`/foo-bar/baz` and `/foo/bar/baz` collide), which is why
/// [`session_exists`] scans rather than recomputes.
pub fn encode_cwd(cwd: &str) -> String {
    cwd.replace(['/', '\\', ':'], "-")
}

/// `<home>/.claude/projects`.
pub fn projects_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}

/// `<home>/.claude/projects/<encoded-cwd>/<session-id>.jsonl`.
pub fn session_jsonl_path(home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    projects_dir(home)
        .join(encode_cwd(cwd))
        .join(format!("{session_id}.jsonl"))
}

/// The directory a session's subagent transcripts live in:
/// `<project-dir>/<session-id>/subagents`, next to the session's own
/// `.jsonl`. May not exist — a session that never delegated has none.
pub fn subagents_dir(session_jsonl: &Path) -> Option<PathBuf> {
    let stem = session_jsonl.file_stem()?;
    Some(session_jsonl.with_file_name(stem).join("subagents"))
}

/// Does any project directory hold `<session-id>.jsonl`? Walks every
/// project dir instead of recomputing the encoded cwd, so it survives
/// the encoding's collisions and any future change to it. Tens of
/// dirs on a typical machine — sub-millisecond.
pub fn session_exists(home: &Path, session_id: &str) -> bool {
    let filename = format!("{session_id}.jsonl");
    let Ok(entries) = fs::read_dir(projects_dir(home)) else {
        return false;
    };
    entries
        .flatten()
        .map(|e| e.path())
        .any(|p| p.is_dir() && p.join(&filename).exists())
}

/// One session transcript on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    /// The `<encoded-cwd>` directory the transcript sits in.
    pub project_dir: PathBuf,
    /// The session uuid — the file stem.
    pub session_id: String,
    /// Full path to the `.jsonl`.
    pub path: PathBuf,
}

impl SessionFile {
    /// Every `agent-*.jsonl` under this session's `subagents` dir,
    /// sorted by name. Empty when the session never delegated.
    pub fn subagent_files(&self) -> Vec<PathBuf> {
        let Some(dir) = subagents_dir(&self.path) else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "jsonl")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("agent-"))
            })
            .collect();
        files.sort();
        files
    }
}

/// Every session transcript under `<home>/.claude/projects`, in no
/// particular order. Subagent transcripts are not included — reach
/// them via [`SessionFile::subagent_files`].
pub fn list_sessions(home: &Path) -> Vec<SessionFile> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir(home)) else {
        return out;
    };
    for project in projects.flatten() {
        let project_dir = project.path();
        if !project_dir.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&project_dir) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if !path.is_file() || path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            out.push(SessionFile {
                project_dir: project_dir.clone(),
                session_id: stem.to_owned(),
                path: path.clone(),
            });
        }
    }
    out
}

// ── row helpers on raw JSON ───────────────────────────────────────

/// `isSidechain == true` — the row belongs to a subagent transcript.
pub fn is_sidechain(row: &Value) -> bool {
    row.get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The row's `timestamp` (ISO 8601) as epoch ms. `None` when the row
/// has none — `cost-state`, `last-prompt`, `ai-title`,
/// `permission-mode` and `bridge-session` rows never do.
pub fn timestamp_ms(row: &Value) -> Option<i64> {
    row.get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_iso8601_ms)
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn owned_str_at(v: &Value, key: &str) -> Option<String> {
    str_at(v, key).map(str::to_owned)
}

fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

// ── typed rows ────────────────────────────────────────────────────

/// Token counts from an assistant row's `message.usage`. All zero
/// when the row has no usage block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Uncached input tokens.
    pub input: u64,
    /// Tokens written to the prompt cache this call.
    pub cache_creation: u64,
    /// Tokens served from the prompt cache.
    pub cache_read: u64,
    /// Output tokens, thinking included.
    pub output: u64,
    /// The thinking share of `output` (`output_tokens_details`).
    pub thinking: u64,
    /// `cache_creation` split by cache TTL, when Claude reports it.
    pub cache_creation_1h: u64,
    pub cache_creation_5m: u64,
}

impl Usage {
    /// Read `message.usage`. Missing fields read as zero.
    pub fn from_value(usage: &Value) -> Self {
        let cache = usage.get("cache_creation");
        Self {
            input: u64_at(usage, "input_tokens"),
            cache_creation: u64_at(usage, "cache_creation_input_tokens"),
            cache_read: u64_at(usage, "cache_read_input_tokens"),
            output: u64_at(usage, "output_tokens"),
            thinking: usage
                .get("output_tokens_details")
                .map_or(0, |d| u64_at(d, "thinking_tokens")),
            cache_creation_1h: cache.map_or(0, |c| u64_at(c, "ephemeral_1h_input_tokens")),
            cache_creation_5m: cache.map_or(0, |c| u64_at(c, "ephemeral_5m_input_tokens")),
        }
    }

    /// Everything the model read this call: input + cache write +
    /// cache read.
    pub fn total_input(&self) -> u64 {
        self.input + self.cache_creation + self.cache_read
    }

    /// Field-wise sum.
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
        self.output += other.output;
        self.thinking += other.thinking;
        self.cache_creation_1h += other.cache_creation_1h;
        self.cache_creation_5m += other.cache_creation_5m;
    }
}

/// One `assistant` row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantRow {
    pub session_id: Option<String>,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub is_sidechain: bool,
    /// Set on subagent transcripts only.
    pub agent_id: Option<String>,
    pub request_id: Option<String>,
    /// `message.id` — shared by every streamed chunk of one response.
    pub message_id: Option<String>,
    pub model: Option<String>,
    /// `end_turn` / `tool_use` / `stop_sequence` / `max_tokens`, or
    /// `None` on a mid-stream chunk.
    pub stop_reason: Option<String>,
    pub usage: Usage,
    pub cwd: Option<String>,
}

impl AssistantRow {
    /// Terminal stop reasons — Claude has finished the turn and is
    /// waiting for the user. `tool_use` and `None` mean more rows
    /// follow.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.stop_reason.as_deref(),
            Some("end_turn" | "stop_sequence" | "max_tokens")
        )
    }

    /// Key for collapsing the streamed chunks of one response, which
    /// each repeat the response's usage. Sum usage over distinct keys,
    /// not over rows, or cached tokens are counted several times.
    /// `None` when the row identifies neither message nor request.
    pub fn dedupe_key(&self) -> Option<String> {
        match (&self.message_id, &self.request_id) {
            (Some(m), Some(r)) => Some(format!("{m}:{r}")),
            (Some(m), None) => Some(m.clone()),
            (None, Some(r)) => Some(r.clone()),
            (None, None) => None,
        }
    }
}

/// The `Agent` tool's launch result on a `user` row — the link from
/// a session to the subagent transcript it spawned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnedAgent {
    pub agent_id: String,
    pub description: Option<String>,
    pub resolved_model: Option<String>,
}

/// One `user` row — a typed prompt or a tool result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRow {
    pub session_id: Option<String>,
    pub uuid: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    /// `toolUseResult` present — a tool reply rather than a prompt.
    pub is_tool_result: bool,
    /// Set when the tool result is a subagent launch.
    pub spawned_agent: Option<SpawnedAgent>,
}

/// Per-model entry of a `cost-state` row's `modelUsage`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
}

/// A `cost-state` row: Claude's own cumulative session totals.
/// Missing fields read as zero — older Claude versions wrote fewer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CostState {
    pub session_id: String,
    #[serde(rename = "totalCostUSD")]
    pub total_cost_usd: f64,
    #[serde(rename = "totalAPIDuration")]
    pub total_api_duration_ms: u64,
    #[serde(rename = "totalAPIDurationWithoutRetries")]
    pub total_api_duration_without_retries_ms: u64,
    #[serde(rename = "totalToolDuration")]
    pub total_tool_duration_ms: u64,
    #[serde(rename = "totalDuration")]
    pub total_duration_ms: u64,
    pub total_lines_added: u64,
    pub total_lines_removed: u64,
    #[serde(rename = "startTime")]
    pub start_time_ms: u64,
    pub model_usage: BTreeMap<String, ModelUsage>,
    pub has_unknown_model_cost: bool,
}

/// One transcript row, typed where it matters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Row {
    Assistant(AssistantRow),
    User(UserRow),
    CostState(CostState),
    /// Any other row type (`system`, `attachment`, `last-prompt`,
    /// `ai-title`, `file-history-snapshot`, …). Kept so callers can
    /// count or skip them without a second parse.
    Other {
        ty: String,
        timestamp_ms: Option<i64>,
    },
}

impl Row {
    /// The row's timestamp, when it has one.
    pub fn timestamp_ms(&self) -> Option<i64> {
        match self {
            Row::Assistant(r) => r.timestamp_ms,
            Row::User(r) => r.timestamp_ms,
            Row::CostState(_) => None,
            Row::Other { timestamp_ms, .. } => *timestamp_ms,
        }
    }
}

/// Type one already-parsed row. `None` when it has no `type`.
pub fn row_from_value(v: &Value) -> Option<Row> {
    let ty = str_at(v, "type")?;
    let row = match ty {
        "assistant" => {
            let message = v.get("message");
            Row::Assistant(AssistantRow {
                session_id: owned_str_at(v, "sessionId"),
                uuid: owned_str_at(v, "uuid"),
                parent_uuid: owned_str_at(v, "parentUuid"),
                timestamp_ms: timestamp_ms(v),
                is_sidechain: is_sidechain(v),
                agent_id: owned_str_at(v, "agentId"),
                request_id: owned_str_at(v, "requestId"),
                message_id: message.and_then(|m| owned_str_at(m, "id")),
                model: message.and_then(|m| owned_str_at(m, "model")),
                stop_reason: message.and_then(|m| owned_str_at(m, "stop_reason")),
                usage: message
                    .and_then(|m| m.get("usage"))
                    .map(Usage::from_value)
                    .unwrap_or_default(),
                cwd: owned_str_at(v, "cwd"),
            })
        }
        "user" => {
            let result = v.get("toolUseResult");
            let spawned_agent = result.and_then(|r| {
                Some(SpawnedAgent {
                    agent_id: owned_str_at(r, "agentId")?,
                    description: owned_str_at(r, "description"),
                    resolved_model: owned_str_at(r, "resolvedModel"),
                })
            });
            Row::User(UserRow {
                session_id: owned_str_at(v, "sessionId"),
                uuid: owned_str_at(v, "uuid"),
                timestamp_ms: timestamp_ms(v),
                is_sidechain: is_sidechain(v),
                agent_id: owned_str_at(v, "agentId"),
                is_tool_result: result.is_some(),
                spawned_agent,
            })
        }
        "cost-state" => Row::CostState(serde_json::from_value(v.clone()).unwrap_or_default()),
        other => Row::Other {
            ty: other.to_owned(),
            timestamp_ms: timestamp_ms(v),
        },
    };
    Some(row)
}

/// Parse one JSONL line. `None` for blank lines, invalid JSON, or a
/// row without a `type`.
pub fn parse_row(line: &str) -> Option<Row> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(trimmed).ok()?;
    row_from_value(&v)
}

/// Iterate a transcript's rows. Lines that are not valid UTF-8, not
/// JSON, or have no `type` are skipped — a partially written last
/// line (Claude is mid-flush) is the common case, and one bad row
/// should not hide the rest.
pub fn rows(path: &Path) -> io::Result<impl Iterator<Item = Row>> {
    let reader = BufReader::new(fs::File::open(path)?);
    // Split on raw bytes rather than `lines()` so one non-UTF-8 line
    // is dropped on its own instead of ending the iteration.
    Ok(reader
        .split(b'\n')
        .map_while(Result::ok)
        .filter_map(|bytes| String::from_utf8(bytes).ok())
        .filter_map(|line| parse_row(&line)))
}

/// Summary of one transcript: usage per model with streamed chunks
/// collapsed, plus the newest `cost-state` row if any.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSummary {
    /// Distinct API responses per model (after dedupe).
    pub responses_by_model: BTreeMap<String, u64>,
    pub usage_by_model: BTreeMap<String, Usage>,
    pub cost_state: Option<CostState>,
    pub first_timestamp_ms: Option<i64>,
    pub last_timestamp_ms: Option<i64>,
    /// Subagents this transcript launched, in order of appearance.
    pub spawned_agents: Vec<SpawnedAgent>,
    pub cwd: Option<String>,
}

/// Fold rows into a [`TranscriptSummary`]. Usage is summed once per
/// [`AssistantRow::dedupe_key`]; rows without a key are counted
/// individually.
pub fn summarize<I: IntoIterator<Item = Row>>(rows: I) -> TranscriptSummary {
    let mut out = TranscriptSummary::default();
    let mut seen = HashSet::new();
    for row in rows {
        if let Some(ts) = row.timestamp_ms() {
            out.first_timestamp_ms = Some(out.first_timestamp_ms.map_or(ts, |f| f.min(ts)));
            out.last_timestamp_ms = Some(out.last_timestamp_ms.map_or(ts, |l| l.max(ts)));
        }
        match row {
            Row::Assistant(a) => {
                if out.cwd.is_none() {
                    out.cwd.clone_from(&a.cwd);
                }
                if let Some(key) = a.dedupe_key()
                    && !seen.insert(key)
                {
                    continue;
                }
                let model = a.model.unwrap_or_else(|| "unknown".to_owned());
                *out.responses_by_model.entry(model.clone()).or_default() += 1;
                out.usage_by_model.entry(model).or_default().add(&a.usage);
            }
            Row::User(u) => {
                if let Some(spawned) = u.spawned_agent {
                    out.spawned_agents.push(spawned);
                }
            }
            Row::CostState(c) => out.cost_state = Some(c),
            Row::Other { .. } => {}
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unreadable_literal, clippy::float_cmp)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn encode_cwd_matches_claude_scheme() {
        // Verified against real project dirs (chapter-5 recon §3, #145).
        assert_eq!(encode_cwd("/Users/foo/bar"), "-Users-foo-bar");
        assert_eq!(encode_cwd("/foo-bar/baz"), "-foo-bar-baz");
        assert_eq!(encode_cwd("C:\\git\\skein"), "C--git-skein");
        assert_eq!(encode_cwd("C:\\Users\\stefa"), "C--Users-stefa");
        assert_eq!(encode_cwd("D:\\"), "D--");
    }

    #[test]
    fn session_path_and_subagents_dir_sit_side_by_side() {
        let home = Path::new("/home/u");
        let p = session_jsonl_path(home, "/home/u/proj", "abc");
        assert_eq!(
            p,
            Path::new("/home/u/.claude/projects/-home-u-proj/abc.jsonl")
        );
        assert_eq!(
            subagents_dir(&p).unwrap(),
            Path::new("/home/u/.claude/projects/-home-u-proj/abc/subagents")
        );
    }

    #[test]
    fn session_exists_scans_every_project_dir() {
        let home = tempfile::TempDir::new().unwrap();
        let dir = projects_dir(home.path()).join("C--git-x");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("s1.jsonl"), "").unwrap();
        assert!(session_exists(home.path(), "s1"));
        assert!(!session_exists(home.path(), "s2"));
        assert!(!session_exists(Path::new("/definitely/not/here"), "s1"));
    }

    #[test]
    fn list_sessions_finds_transcripts_and_their_subagents() {
        let home = tempfile::TempDir::new().unwrap();
        let dir = projects_dir(home.path()).join("C--git-x");
        let sub = dir.join("s1").join("subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.join("s1.jsonl"), "").unwrap();
        fs::write(dir.join("notes.txt"), "").unwrap();
        fs::write(sub.join("agent-b.jsonl"), "").unwrap();
        fs::write(sub.join("agent-a.jsonl"), "").unwrap();
        fs::write(sub.join("agent-a.meta.json"), "{}").unwrap();

        let sessions = list_sessions(home.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "s1");
        let subs = sessions[0].subagent_files();
        let names: Vec<_> = subs
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["agent-a.jsonl", "agent-b.jsonl"]);
    }

    /// Shape taken from a real 2.1.263 assistant row.
    fn assistant_row() -> Value {
        json!({
            "parentUuid": "p1",
            "isSidechain": false,
            "message": {
                "model": "claude-fable-5-1",
                "id": "msg_01",
                "role": "assistant",
                "content": [],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 2,
                    "cache_creation_input_tokens": 14654,
                    "cache_read_input_tokens": 31919,
                    "output_tokens": 400,
                    "output_tokens_details": {"thinking_tokens": 135},
                    "cache_creation": {
                        "ephemeral_1h_input_tokens": 14654,
                        "ephemeral_5m_input_tokens": 0
                    }
                }
            },
            "requestId": "req_01",
            "type": "assistant",
            "uuid": "u1",
            "timestamp": "2026-09-08T11:16:21.559Z",
            "cwd": "C:\\git\\agents",
            "sessionId": "s1"
        })
    }

    #[test]
    fn assistant_row_is_typed_with_usage() {
        let Some(Row::Assistant(a)) = row_from_value(&assistant_row()) else {
            panic!("expected assistant row");
        };
        assert_eq!(a.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(a.request_id.as_deref(), Some("req_01"));
        assert_eq!(a.message_id.as_deref(), Some("msg_01"));
        assert!(!a.is_terminal());
        assert!(!a.is_sidechain);
        assert_eq!(a.usage.cache_read, 31919);
        assert_eq!(a.usage.cache_creation_1h, 14654);
        assert_eq!(a.usage.thinking, 135);
        assert_eq!(a.usage.total_input(), 2 + 14654 + 31919);
        assert_eq!(a.dedupe_key().as_deref(), Some("msg_01:req_01"));
        assert!(a.timestamp_ms.is_some());
    }

    #[test]
    fn user_row_carries_spawned_agent() {
        let v = json!({
            "type": "user",
            "isSidechain": false,
            "uuid": "u2",
            "timestamp": "2026-07-12T17:09:56.953Z",
            "message": {"role": "user", "content": []},
            "toolUseResult": {
                "isAsync": true,
                "status": "async_launched",
                "agentId": "a837d4fac8622afd9",
                "description": "Audit first editor open log",
                "resolvedModel": "claude-fable-5"
            }
        });
        let Some(Row::User(u)) = row_from_value(&v) else {
            panic!("expected user row");
        };
        assert!(u.is_tool_result);
        let spawned = u.spawned_agent.unwrap();
        assert_eq!(spawned.agent_id, "a837d4fac8622afd9");
        assert_eq!(spawned.resolved_model.as_deref(), Some("claude-fable-5"));
    }

    #[test]
    fn plain_prompt_is_not_a_tool_result() {
        let v = json!({"type": "user", "message": {"role": "user", "content": "hi"}});
        let Some(Row::User(u)) = row_from_value(&v) else {
            panic!("expected user row");
        };
        assert!(!u.is_tool_result);
        assert!(u.spawned_agent.is_none());
        assert_eq!(u.timestamp_ms, None);
    }

    /// Shape taken verbatim from a real 2.1.263 `cost-state` row.
    #[test]
    fn cost_state_row_is_typed() {
        let v = json!({
            "type": "cost-state",
            "sessionId": "9210e4bf",
            "totalCostUSD": 12.05013775,
            "totalAPIDuration": 181766,
            "totalAPIDurationWithoutRetries": 181728,
            "totalToolDuration": 1197212,
            "totalLinesAdded": 12,
            "totalLinesRemoved": 1,
            "totalDuration": 11859757,
            "startTime": 1788723920352u64,
            "modelUsage": {
                "claude-fable-5-1": {
                    "inputTokens": 838,
                    "outputTokens": 10945,
                    "thinkingTokens": 2722,
                    "cacheReadInputTokens": 5129711,
                    "cacheCreationInputTokens": 510604,
                    "webSearchRequests": 0,
                    "costUSD": 12.05013775
                }
            },
            "hasUnknownModelCost": false
        });
        let Some(Row::CostState(c)) = row_from_value(&v) else {
            panic!("expected cost-state row");
        };
        assert_eq!(c.session_id, "9210e4bf");
        assert!((c.total_cost_usd - 12.050_137_75).abs() < 1e-9);
        assert_eq!(c.total_duration_ms, 11_859_757);
        assert_eq!(c.total_lines_added, 12);
        let m = &c.model_usage["claude-fable-5-1"];
        assert_eq!(m.cache_read_input_tokens, 5_129_711);
        assert!((m.cost_usd - 12.050_137_75).abs() < 1e-9);
    }

    #[test]
    fn cost_state_tolerates_a_field_less_row() {
        let v = json!({"type": "cost-state", "sessionId": "s"});
        let Some(Row::CostState(c)) = row_from_value(&v) else {
            panic!("expected cost-state row");
        };
        assert_eq!(c.total_cost_usd, 0.0);
        assert!(c.model_usage.is_empty());
    }

    #[test]
    fn other_rows_keep_their_type() {
        let v = json!({"type": "file-history-snapshot", "timestamp": "2026-05-15T21:16:22Z"});
        assert!(matches!(
            row_from_value(&v),
            Some(Row::Other { ty, timestamp_ms: Some(_) }) if ty == "file-history-snapshot"
        ));
        assert!(row_from_value(&json!({"foo": 1})).is_none());
        assert!(parse_row("").is_none());
        assert!(parse_row("{not json").is_none());
    }

    #[test]
    fn summarize_dedupes_streamed_chunks_and_keeps_newest_cost_state() {
        let mut chunk2 = assistant_row();
        chunk2["uuid"] = json!("u1b");
        chunk2["message"]["stop_reason"] = json!("end_turn");
        let mut other = assistant_row();
        other["message"]["id"] = json!("msg_02");
        other["requestId"] = json!("req_02");
        other["message"]["model"] = json!("claude-haiku-4-5");
        let rows = vec![
            row_from_value(&assistant_row()).unwrap(),
            row_from_value(&chunk2).unwrap(),
            row_from_value(&other).unwrap(),
            row_from_value(&json!({"type": "cost-state", "sessionId": "s", "totalCostUSD": 1.0}))
                .unwrap(),
            row_from_value(&json!({"type": "cost-state", "sessionId": "s", "totalCostUSD": 2.5}))
                .unwrap(),
        ];
        let s = summarize(rows);
        assert_eq!(s.responses_by_model["claude-fable-5-1"], 1);
        assert_eq!(s.responses_by_model["claude-haiku-4-5"], 1);
        assert_eq!(s.usage_by_model["claude-fable-5-1"].cache_read, 31919);
        assert!((s.cost_state.unwrap().total_cost_usd - 2.5).abs() < 1e-9);
        assert_eq!(s.cwd.as_deref(), Some("C:\\git\\agents"));
    }

    #[test]
    fn rows_skips_bad_lines_and_reads_the_rest() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{}", assistant_row()).unwrap();
        writeln!(f).unwrap();
        writeln!(f, "{{\"type\":\"cost-state\",\"sessionId\":\"s\"}}").unwrap();
        f.write_all(b"\xff\xfe not utf8\n").unwrap();
        write!(f, "{{\"type\":\"assistant\",\"truncated").unwrap();
        drop(f);
        let got: Vec<Row> = rows(&path).unwrap().collect();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0], Row::Assistant(_)));
        assert!(matches!(got[1], Row::CostState(_)));
    }
}
