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
//! - `agent-setting` — the agent definition the session runs as
//!   (`claude --agent <name>` or the `agent` setting). Absent for a
//!   plain session.
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

pub mod background;
pub mod cli_version;
pub mod local_command;

// ── paths ─────────────────────────────────────────────────────────

/// Longest encoded name Claude uses as-is; longer ones are cut here and
/// suffixed with a hash of the full path.
const MAX_ENCODED_LEN: usize = 200;

/// Claude's project-directory encoding, ported from Claude Code's own
/// (2.1.270):
///
/// ```js
/// let e = t.replace(/[^a-zA-Z0-9]/g, "-");
/// if (e.length <= 200) return e;
/// return `${e.slice(0, 200)}-${Math.abs(hash(t)).toString(36)}`;
/// ```
///
/// **Every** character outside `[A-Za-z0-9]` becomes `-`, not only
/// separators: `D:\gaming_pc_d\git` → `D--gaming-pc-d-git` (#259). The
/// regex runs over UTF-16 code units, so a character outside the BMP
/// becomes two dashes. The encoding is lossy (`/foo-bar/baz` and
/// `/foo/bar/baz` collide), which is why [`session_exists`] scans
/// rather than recomputes.
pub fn encode_cwd(cwd: &str) -> String {
    let mut out = String::with_capacity(cwd.len());
    for c in cwd.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if out.len() <= MAX_ENCODED_LEN {
        return out;
    }
    // All ASCII by now, so byte length is the UTF-16 length JS measures.
    out.truncate(MAX_ENCODED_LEN);
    format!("{out}-{}", base36(js_string_hash(cwd).unsigned_abs()))
}

/// The `(h << 5) - h + charCode | 0` string hash Claude suffixes long
/// names with: Java's `String.hashCode`, over UTF-16 code units.
fn js_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, unit| {
        h.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

/// `Number.prototype.toString(36)` for a non-negative integer.
fn base36(mut n: u32) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(n % 36, 36).expect("remainder is below the radix"));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
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

/// The `<session-id>.jsonl` in whichever project directory holds it.
/// Walks every project dir instead of recomputing the encoded cwd, so
/// it survives the encoding's collisions and any future change to it.
/// Tens of dirs on a typical machine — sub-millisecond.
pub fn find_session_jsonl(home: &Path, session_id: &str) -> Option<PathBuf> {
    let filename = format!("{session_id}.jsonl");
    fs::read_dir(projects_dir(home))
        .ok()?
        .flatten()
        .map(|e| e.path().join(&filename))
        .find(|p| p.is_file())
}

/// Does any project directory hold `<session-id>.jsonl`? See
/// [`find_session_jsonl`].
pub fn session_exists(home: &Path, session_id: &str) -> bool {
    find_session_jsonl(home, session_id).is_some()
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

// ── subagents ────────────────────────────────────────────────────

/// The `<id>` in `agent-<id>.jsonl`. `None` for anything else — no
/// `agent-` prefix, a non-`.jsonl` extension (including the
/// `.meta.json` sidecar itself), or an empty id.
pub fn subagent_id_from_path(path: &Path) -> Option<String> {
    if path.extension().is_none_or(|x| x != "jsonl") {
        return None;
    }
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    let id = stem.strip_prefix("agent-")?;
    if id.is_empty() {
        return None;
    }
    Some(id.to_owned())
}

/// The sidecar next to a subagent transcript: `agent-<id>.jsonl` ->
/// `agent-<id>.meta.json`. Built explicitly from the stem rather than
/// `with_extension("meta.json")` — that only does the right thing
/// here because the stem itself has no dot.
pub fn subagent_meta_path(jsonl: &Path) -> PathBuf {
    let stem = jsonl.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    jsonl.with_file_name(format!("{stem}.meta.json"))
}

/// The `agent-<id>.meta.json` sidecar Claude Code writes next to a
/// subagent's transcript. A real one, verbatim (2.1.263):
///
/// ```json
/// {"agentType":"explore","description":"Map Claude JSONL tailer","toolUseId":"toolu_01NQaF9vThE1Z2brYyXv4anm","spawnDepth":1,"requestShape":"background","requestNonInteractive":true}
/// ```
///
/// Every field is optional and unknown keys are ignored ON PURPOSE —
/// this shape belongs to Claude Code, not Skein, and a future field
/// must not break parsing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentMeta {
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub tool_use_id: Option<String>,
    pub spawn_depth: Option<u32>,
    pub request_shape: Option<String>,
    pub request_non_interactive: Option<bool>,
}

/// Reads and parses the sidecar for a subagent transcript. `None` on
/// a missing file, an I/O error, or JSON that doesn't parse — never
/// an error, never a panic.
pub fn read_subagent_meta(jsonl: &Path) -> Option<SubagentMeta> {
    let text = fs::read_to_string(subagent_meta_path(jsonl)).ok()?;
    serde_json::from_str(&text).ok()
}

// ── row helpers on raw JSON ───────────────────────────────────────

/// `isSidechain == true` — the row belongs to a subagent transcript.
pub fn is_sidechain(row: &Value) -> bool {
    row.get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A `user` row flagged `queueTranscriptOnly` (#440): Claude Code writes
/// a `<task-notification>` this way right after an end of turn, to record
/// a queued message. It starts no turn, so it is never a prompt.
#[must_use]
pub fn is_queue_transcript_only(row: &Value) -> bool {
    row.get("type").and_then(Value::as_str) == Some("user")
        && row.get("queueTranscriptOnly").and_then(Value::as_bool) == Some(true)
}

/// The terminal stop-reason set, shared by `subagent_row_is_terminal`
/// (raw `Value`, needed because the subagent tailer never deserializes
/// a full `AssistantRow`) and `AssistantRow::is_terminal` (the parsed
/// struct) — one place spelling out `end_turn` / `stop_sequence` /
/// `max_tokens` so a future stop reason can't land in one and not the
/// other.
fn stop_reason_is_terminal(stop_reason: Option<&str>) -> bool {
    matches!(
        stop_reason,
        Some("end_turn" | "stop_sequence" | "max_tokens")
    )
}

/// True for an `assistant` row whose `stop_reason` is terminal
/// (`end_turn` / `stop_sequence` / `max_tokens` — mirrors the set
/// `harness_events_claude.rs` treats as ending a turn). A subagent has
/// no user to await, so the end of its turn is its exit: this is how
/// a finished subagent transcript is told from a live one.
pub fn subagent_row_is_terminal(row: &Value) -> bool {
    if row.get("type").and_then(Value::as_str) != Some("assistant") {
        return false;
    }
    let stop_reason = row
        .get("message")
        .and_then(|m| m.get("stop_reason"))
        .and_then(Value::as_str);
    stop_reason_is_terminal(stop_reason)
}

/// A change in whether a subagent transcript reads as finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentTransition {
    /// The subagent just exited.
    Finished,
    /// A finished subagent was resumed (a new prompt arrived).
    Reopened,
}

/// Per-subagent lifecycle, fed its transcript rows in order.
///
/// Older Claude Code versions end a subagent with a terminal
/// `stop_reason`. Newer ones (2.1.285) end it by calling the
/// `SubagentHandback` tool: an assistant row holding that `tool_use`
/// with a null `stop_reason`, then a `user` row with its `tool_result`,
/// and nothing after. Versions 2.1.271-284 hand back the same way but
/// then add a trailing `end_turn` text row.
///
/// The exit is the tool RESULT, not the `tool_use`: before the result the
/// report has not been delivered, and the result row is where Claude
/// itself marks `toolEndsTurn: true` (seen on no other row). A tail that
/// starts mid-file may never have seen the `tool_use`, so `toolEndsTurn`
/// alone is enough; the remembered handback ids cover a result that
/// lacks the flag.
///
/// A finished subagent can be resumed by a `user` row with no
/// `tool_result` (a `SendMessage` delivery), except a
/// `queueTranscriptOnly` row, which starts no turn. `isMeta` rows DO
/// reopen: on real transcripts 1525 of 1526 post-exit ones were followed
/// by assistant work. Assistant rows do NOT
/// reopen: the 2.1.27x trailing thinking/`end_turn` rows follow the
/// handback and must not flip it back, and non-message rows such as a
/// trailing `attachment` are ignored for the same reason.
#[derive(Debug, Clone, Default)]
pub struct SubagentLifecycle {
    finished: bool,
    handback_ids: HashSet<String>,
}

impl SubagentLifecycle {
    /// Whether the rows observed so far leave the subagent finished.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Feed the next transcript row; returns the transition it caused,
    /// if any.
    pub fn observe(&mut self, row: &Value) -> Option<SubagentTransition> {
        let content = row
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array);
        match row.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                for block in content.into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use")
                        && block.get("name").and_then(Value::as_str) == Some("SubagentHandback")
                        && let Some(id) = block.get("id").and_then(Value::as_str)
                    {
                        self.handback_ids.insert(id.to_owned());
                    }
                }
                if subagent_row_is_terminal(row) {
                    return self.set_finished();
                }
                None
            }
            Some("user") => {
                let mut has_result = false;
                let mut handed_back = false;
                for block in content.into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        has_result = true;
                        if block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| self.handback_ids.contains(id))
                        {
                            handed_back = true;
                        }
                    }
                }
                if has_result {
                    let ends_turn = row.get("toolEndsTurn").and_then(Value::as_bool) == Some(true);
                    if ends_turn || handed_back {
                        return self.set_finished();
                    }
                    None
                } else if self.finished && !is_queue_transcript_only(row) {
                    self.finished = false;
                    Some(SubagentTransition::Reopened)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn set_finished(&mut self) -> Option<SubagentTransition> {
        if self.finished {
            None
        } else {
            self.finished = true;
            Some(SubagentTransition::Finished)
        }
    }
}

/// The attach-time scan: run a fresh [`SubagentLifecycle`] over every
/// complete, parseable, non-empty JSON line and report whether the
/// transcript ends finished.
#[must_use]
pub fn subagent_transcript_is_finished(content: &str) -> bool {
    let mut lifecycle = SubagentLifecycle::default();
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(row) = serde_json::from_str::<Value>(line) {
            lifecycle.observe(&row);
        }
    }
    lifecycle.is_finished()
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
        stop_reason_is_terminal(self.stop_reason.as_deref())
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
    /// An `agent-setting` row: the name of the agent definition this
    /// session runs as (`claude --agent <name>` or the `agent` setting).
    /// Written once near the start of the transcript; absent when the
    /// session runs as plain Claude Code.
    AgentSetting(String),
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
            Row::CostState(_) | Row::AgentSetting(_) => None,
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
        "agent-setting" => Row::AgentSetting(owned_str_at(v, "agentSetting").unwrap_or_default()),
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
    /// The agent definition the session ran as, from the newest
    /// `agent-setting` row. `None` for plain Claude Code sessions.
    pub agent_setting: Option<String>,
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
            Row::AgentSetting(name) => out.agent_setting = Some(name),
            Row::Other { .. } => {}
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unreadable_literal, clippy::float_cmp)]
mod tests;
