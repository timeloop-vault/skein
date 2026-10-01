//! Claude Code background tasks (#444, slice S1 of #439): what a
//! transcript records when the agent starts a `Bash` / `PowerShell`
//! command in the background or arms a `Monitor`, and how it ends.
//! Every row shape below is quoted in `docs/background-task-recon.md`
//! §Q1; the live-set formula is §Q2. Pure parsers plus one small state
//! machine, [`BackgroundTasks`], to be shared by the live tail and the
//! attach scan, the way [`super::SubagentLifecycle`] is. Nothing here
//! touches Tauri or the frontend.
//!
//! The shapes belong to Claude Code, not Skein, so every field is read
//! defensively from raw `serde_json::Value` rows and anything
//! unrecognised is skipped rather than failing.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How much of an `.output` file's end is read to find its trailer. The
/// trailer (`[exited with code N]` / `[killed]`) is a short last line;
/// the files themselves can be large.
const TRAILER_TAIL_BYTES: u64 = 4096;

/// The prefix of a Monitor expiry event (recon §Q1, "Terminal:
/// Monitor"). An expired Monitor has NO `<status>` tag, so this string,
/// which Claude Code may reword, is the only thing that identifies it.
const MONITOR_EXPIRY_PREFIX: &str = "[Monitor expired after";

/// Which tool started the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    Bash,
    #[serde(rename = "powershell")]
    PowerShell,
    Monitor,
}

impl BackgroundKind {
    fn from_tool(name: &str) -> Option<Self> {
        match name {
            "Bash" => Some(Self::Bash),
            "PowerShell" => Some(Self::PowerShell),
            "Monitor" => Some(Self::Monitor),
            _ => None,
        }
    }
}

/// How a task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Completed,
    Failed,
    Killed,
    /// `<status>stopped</status>`, which Claude Code writes itself for
    /// every task a previous session left behind (recon §Q1,
    /// "Session-restart synthetic").
    Stopped,
    /// A Monitor passed its deadline; it is told from an event only by
    /// the event text.
    Expired,
    /// The agent called `TaskStop`, which writes no notification.
    TaskStopped,
    /// The subagent that launched the task exited. The adapter ends its
    /// tasks this way; it can't know whether they finished.
    SubagentEnded,
    /// A `<status>` value this parser doesn't know. Terminal on purpose:
    /// "unknown, not running" beats a leaked running entry.
    Unknown,
}

/// A terminal outcome with whatever the notification said about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub status: OutcomeStatus,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
}

impl Outcome {
    #[must_use]
    pub fn new(status: OutcomeStatus) -> Self {
        Self {
            status,
            exit_code: None,
            summary: None,
        }
    }
}

/// A background task, as its start rows describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub task_id: String,
    /// The start's `tool_use` id; a terminal notification repeats it
    /// (Monitor notices don't).
    pub tool_use_id: String,
    /// Serialized as `task_kind`: the adapter's event enum is tagged
    /// with `kind`, and a newtype variant flattens this struct into it.
    #[serde(rename = "task_kind")]
    pub kind: BackgroundKind,
    /// Moved to the background by its timeout rather than asked to
    /// (`toolUseResult.timedOutAfterMs` present).
    pub auto_backgrounded: bool,
    pub description: Option<String>,
    pub command: Option<String>,
    /// The `.output` path the start printed, when it printed one.
    pub output_file: Option<String>,
    pub timeout_ms: Option<u64>,
    pub persistent: bool,
    /// The start row's `timestamp`, epoch ms.
    pub started_ms: Option<i64>,
}

// ── <task-notification> ───────────────────────────────────────────

/// The `<task-notification>` envelope (recon §Q1). Agent tasks (`a…`
/// ids) share it and add a `<note>`, which is ignored here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskNotification {
    pub task_id: String,
    pub tool_use_id: Option<String>,
    pub output_file: Option<String>,
    /// The raw `<status>` text.
    pub status: Option<String>,
    pub summary: Option<String>,
    pub event: Option<String>,
}

/// What a notification means for the task it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationClass {
    Terminal(Outcome),
    /// A Monitor output line: not terminal.
    MonitorEvent(String),
    /// Neither a status nor an event.
    Ignored,
}

/// The text between `<tag>` and the next `</tag>`, trimmed, `None` when
/// absent or empty.
fn tag_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&format!("</{tag}>"))?;
    let inner = text[start..end].trim();
    (!inner.is_empty()).then_some(inner)
}

/// Parses the envelope. `None` unless the text starts with
/// `<task-notification>` and names a `<task-id>`. Tags may come in any
/// order and the summary may span lines.
#[must_use]
pub fn parse_task_notification(text: &str) -> Option<TaskNotification> {
    if !text.trim_start().starts_with("<task-notification>") {
        return None;
    }
    let own = |tag| tag_text(text, tag).map(str::to_owned);
    Some(TaskNotification {
        task_id: own("task-id")?,
        tool_use_id: own("tool-use-id"),
        output_file: own("output-file"),
        status: own("status"),
        summary: own("summary"),
        event: own("event"),
    })
}

/// An exit code from summary forms like `completed (exit code 0)`,
/// `failed with exit code 2` and `ended without producing output
/// (exit 1)`. The LAST match wins: a summary that quotes a raw command
/// may itself mention an exit code.
fn exit_code_in(summary: &str) -> Option<i32> {
    for marker in ["exit code ", "(exit "] {
        if let Some(at) = summary.rfind(marker) {
            let rest = &summary[at + marker.len()..];
            let end = rest
                .char_indices()
                .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && c == '-')))
                .map_or(rest.len(), |(i, _)| i);
            if let Ok(code) = rest[..end].parse() {
                return Some(code);
            }
        }
    }
    None
}

/// Classifies a notification. A `<status>` decides (an unrecognised one
/// is `Unknown`, terminal); with none, a `[Monitor expired after …`
/// event is `Expired` and any other event is a non-terminal Monitor
/// event.
#[must_use]
pub fn classify_notification(n: &TaskNotification) -> NotificationClass {
    if let Some(status) = n.status.as_deref() {
        let status = match status.trim() {
            "completed" => OutcomeStatus::Completed,
            "failed" => OutcomeStatus::Failed,
            "killed" => OutcomeStatus::Killed,
            "stopped" => OutcomeStatus::Stopped,
            _ => OutcomeStatus::Unknown,
        };
        return NotificationClass::Terminal(Outcome {
            status,
            exit_code: n.summary.as_deref().and_then(exit_code_in),
            summary: n.summary.clone(),
        });
    }
    match n.event.as_deref() {
        Some(event) if event.trim_start().starts_with(MONITOR_EXPIRY_PREFIX) => {
            NotificationClass::Terminal(Outcome {
                status: OutcomeStatus::Expired,
                exit_code: None,
                summary: n.summary.clone(),
            })
        }
        Some(event) => NotificationClass::MonitorEvent(event.to_owned()),
        None => NotificationClass::Ignored,
    }
}

/// The notification text a row carries, and whether the row is the
/// enqueue. Three carriers hold the same text (recon §Q1, "How one
/// notification is carried"): a `queue-operation` `enqueue` row's
/// `content`; a `user` row whose `message.content` is the raw string (or
/// text blocks); an `attachment` row of type `queued_command`, in
/// `attachment.prompt`.
fn notification_carrier(row: &Value) -> Option<(&str, bool)> {
    let is_notification = |s: &str| s.trim_start().starts_with("<task-notification>");
    match row.get("type").and_then(Value::as_str)? {
        "queue-operation" => {
            if row.get("operation").and_then(Value::as_str) != Some("enqueue") {
                return None;
            }
            let text = row.get("content").and_then(Value::as_str)?;
            is_notification(text).then_some((text, true))
        }
        "user" => {
            let content = row.get("message")?.get("content")?;
            let text = match content {
                Value::String(s) => Some(s.as_str()),
                Value::Array(blocks) => blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .find_map(|b| b.get("text").and_then(Value::as_str)),
                _ => None,
            }?;
            is_notification(text).then_some((text, false))
        }
        "attachment" => {
            let att = row.get("attachment")?;
            if att.get("type").and_then(Value::as_str) != Some("queued_command") {
                return None;
            }
            let text = att.get("prompt").and_then(Value::as_str)?;
            is_notification(text).then_some((text, false))
        }
        _ => None,
    }
}

/// The `<task-notification>` text a row carries, from any of the three
/// carriers; `None` for every other row.
#[must_use]
pub fn task_notification_text(row: &Value) -> Option<&str> {
    notification_carrier(row).map(|(text, _)| text)
}

// ── .output trailer ───────────────────────────────────────────────

/// What the end of a task's `.output` file says (recon §Q1, "On disk").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTrailer {
    /// `[exited with code N]`.
    Exited(i32),
    /// `[killed]`: `TaskStop` and a Monitor expiry both leave it.
    Killed,
    /// The file is readable but has no trailer: still running, or
    /// crashed (unverified).
    None,
    /// The file is missing or unreadable, so nothing is known.
    Unreadable,
}

/// Parses one line as a trailer; anything else is [`OutputTrailer::None`].
#[must_use]
pub fn parse_output_trailer(last_line: &str) -> OutputTrailer {
    let line = last_line.trim();
    if line == "[killed]" {
        return OutputTrailer::Killed;
    }
    line.strip_prefix("[exited with code ")
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|code| code.parse().ok())
        .map_or(OutputTrailer::None, OutputTrailer::Exited)
}

/// Reads the last non-empty line of the file, from at most the final
/// [`TRAILER_TAIL_BYTES`] bytes, and parses it.
#[must_use]
pub fn read_output_trailer(path: &Path) -> OutputTrailer {
    let Ok(mut file) = File::open(path) else {
        return OutputTrailer::Unreadable;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return OutputTrailer::Unreadable;
    };
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TRAILER_TAIL_BYTES)))
        .is_err()
    {
        return OutputTrailer::Unreadable;
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return OutputTrailer::Unreadable;
    }
    String::from_utf8_lossy(&tail)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map_or(OutputTrailer::None, parse_output_trailer)
}

// ── the state machine ─────────────────────────────────────────────

/// A change in the set of background tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "transition", rename_all = "snake_case")]
pub enum BackgroundTransition {
    Started(BackgroundTask),
    Ended {
        task: BackgroundTask,
        outcome: Outcome,
    },
    /// One Monitor output line. Only the `enqueue` copy of a
    /// notification yields one, so a line isn't reported twice when its
    /// delivery row follows; a Monitor owned by a subagent therefore
    /// yields none (its enqueue is in the main file).
    MonitorEvent {
        task_id: String,
        line: String,
    },
}

/// A `tool_use` seen but not yet answered.
#[derive(Debug, Clone)]
enum Pending {
    Start {
        kind: BackgroundKind,
        description: Option<String>,
        command: Option<String>,
        timeout_ms: Option<u64>,
        persistent: bool,
        started_ms: Option<i64>,
    },
    Stop {
        task_id: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct Entry {
    task: BackgroundTask,
    ended: bool,
}

/// Per-session background-task state, fed transcript rows in order.
///
/// A start is a `tool_use` (`Bash`/`PowerShell`/`Monitor`) joined to its
/// non-error `tool_result` whose row-level `toolUseResult` carries
/// `backgroundTaskId` (shell) or `taskId` (Monitor). It ends on a
/// terminal notification, a `TaskStop` result, or an explicit
/// [`end`](Self::end). Each id starts at most once and ends at most once:
/// every notification appears as an enqueue AND as a delivery, and the
/// second is ignored. Ids never seen to start (agent tasks, or another
/// transcript's) are ignored.
///
/// There is no "running" row anywhere (recon §Q2), so see
/// [`outstanding`](Self::outstanding) for how the live set is derived.
#[derive(Debug, Clone, Default)]
pub struct BackgroundTasks {
    pending: HashMap<String, Pending>,
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    /// Directory (and its separator, since the path may be a Windows
    /// one read on another OS) of any `.output` path seen.
    output_dir: Option<(String, char)>,
    /// Terminals (a notification, a `TaskStop` result) that arrived
    /// before their id's start: one adapter feeds the main file and the
    /// subagent files, and a subagent's start can be processed after the
    /// main file's enqueue of its terminal. Applied when the start lands.
    /// Agent `a…` ids sit here harmlessly. First terminal wins.
    early_terminals: HashMap<String, Outcome>,
}

impl BackgroundTasks {
    /// The attach-time scan: a fresh state over every parseable,
    /// non-empty JSON line.
    #[must_use]
    pub fn scan(content: &str) -> Self {
        let mut tasks = Self::default();
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(row) = serde_json::from_str::<Value>(line) {
                tasks.observe(&row);
            }
        }
        tasks
    }

    /// Feed the next transcript row; returns the transitions it caused.
    pub fn observe(&mut self, row: &Value) -> Vec<BackgroundTransition> {
        let mut out = Vec::new();
        if let Some((text, enqueue)) = notification_carrier(row) {
            self.observe_notification(text, enqueue, &mut out);
            return out;
        }
        match row.get("type").and_then(Value::as_str) {
            Some("assistant") => self.observe_assistant(row),
            Some("user") => self.observe_result(row, &mut out),
            _ => {}
        }
        out
    }

    fn observe_assistant(&mut self, row: &Value) {
        let started_ms = super::timestamp_ms(row);
        for block in content_blocks(row) {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let (Some(name), Some(id)) = (
                block.get("name").and_then(Value::as_str),
                block.get("id").and_then(Value::as_str),
            ) else {
                continue;
            };
            let input = block.get("input");
            let field = |k: &str| input.and_then(|i| i.get(k));
            let text = |k: &str| field(k).and_then(Value::as_str).map(str::to_owned);
            if name == "TaskStop" {
                self.pending.insert(
                    id.to_owned(),
                    Pending::Stop {
                        task_id: text("task_id"),
                    },
                );
            } else if let Some(kind) = BackgroundKind::from_tool(name) {
                let timeout = if kind == BackgroundKind::Monitor {
                    "timeout_ms"
                } else {
                    "timeout"
                };
                self.pending.insert(
                    id.to_owned(),
                    Pending::Start {
                        kind,
                        description: text("description"),
                        command: text("command"),
                        timeout_ms: field(timeout).and_then(Value::as_u64),
                        persistent: field("persistent")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        started_ms,
                    },
                );
            }
        }
    }

    fn observe_result(&mut self, row: &Value, out: &mut Vec<BackgroundTransition>) {
        let result = row.get("toolUseResult");
        for block in content_blocks(row) {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(pending) = self.pending.remove(use_id) else {
                continue;
            };
            if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            match pending {
                Pending::Start {
                    kind,
                    description,
                    command,
                    timeout_ms,
                    persistent,
                    started_ms,
                } => {
                    let id_key = if kind == BackgroundKind::Monitor {
                        "taskId"
                    } else {
                        "backgroundTaskId"
                    };
                    let Some(task_id) = result.and_then(|r| r.get(id_key)).and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let r = |k: &str| result.and_then(|r| r.get(k));
                    let output_file = output_path_in(&result_text(block));
                    if let Some(path) = &output_file {
                        self.note_output_dir(path);
                    }
                    let task = BackgroundTask {
                        task_id: task_id.to_owned(),
                        tool_use_id: use_id.to_owned(),
                        kind,
                        auto_backgrounded: r("timedOutAfterMs").is_some(),
                        description,
                        command,
                        output_file,
                        timeout_ms: r("timeoutMs")
                            .or_else(|| r("timedOutAfterMs"))
                            .and_then(Value::as_u64)
                            .or(timeout_ms),
                        persistent: r("persistent")
                            .and_then(Value::as_bool)
                            .unwrap_or(persistent),
                        started_ms,
                    };
                    if self.index.contains_key(task_id) {
                        continue;
                    }
                    self.index.insert(task_id.to_owned(), self.entries.len());
                    self.entries.push(Entry {
                        task: task.clone(),
                        ended: false,
                    });
                    out.push(BackgroundTransition::Started(task));
                    if let Some(outcome) = self.early_terminals.remove(task_id) {
                        out.extend(self.end(task_id, outcome));
                    }
                }
                Pending::Stop { task_id } => {
                    let id = result
                        .and_then(|r| r.get("task_id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or(task_id);
                    if let Some(id) = id {
                        let outcome = Outcome::new(OutcomeStatus::TaskStopped);
                        if self.index.contains_key(&id) {
                            out.extend(self.end(&id, outcome));
                        } else {
                            self.early_terminals.entry(id).or_insert(outcome);
                        }
                    }
                }
            }
        }
    }

    fn observe_notification(
        &mut self,
        text: &str,
        enqueue: bool,
        out: &mut Vec<BackgroundTransition>,
    ) {
        let Some(n) = parse_task_notification(text) else {
            return;
        };
        if let Some(path) = &n.output_file {
            self.note_output_dir(path);
        }
        let Some(&at) = self.index.get(&n.task_id) else {
            if let NotificationClass::Terminal(outcome) = classify_notification(&n) {
                self.early_terminals.entry(n.task_id).or_insert(outcome);
            }
            return;
        };
        if self.entries[at].ended {
            return;
        }
        if self.entries[at].task.output_file.is_none() {
            self.entries[at].task.output_file.clone_from(&n.output_file);
        }
        match classify_notification(&n) {
            NotificationClass::Terminal(outcome) => out.extend(self.end(&n.task_id, outcome)),
            NotificationClass::MonitorEvent(line) if enqueue => {
                out.push(BackgroundTransition::MonitorEvent {
                    task_id: n.task_id,
                    line,
                });
            }
            _ => {}
        }
    }

    fn note_output_dir(&mut self, path: &str) {
        if let Some(at) = path.rfind(['/', '\\']) {
            let sep = if path.as_bytes()[at] == b'\\' {
                '\\'
            } else {
                '/'
            };
            self.output_dir = Some((path[..at].to_owned(), sep));
        }
    }

    /// Ends a task from outside (the adapter drops a subagent's tasks
    /// when that subagent ends). `None` for an unknown or already ended
    /// id.
    pub fn end(&mut self, task_id: &str, outcome: Outcome) -> Option<BackgroundTransition> {
        let entry = self.entries.get_mut(*self.index.get(task_id)?)?;
        if entry.ended {
            return None;
        }
        entry.ended = true;
        Some(BackgroundTransition::Ended {
            task: entry.task.clone(),
            outcome,
        })
    }

    /// Ends every non-persistent Monitor with
    /// `started_ms + timeout_ms + grace_ms <= now_ms` as `Expired`, no
    /// summary. The adapter sweeps with a grace so the real expiry
    /// notice, which says more, normally wins.
    pub fn expire_overdue(&mut self, now_ms: i64, grace_ms: i64) -> Vec<BackgroundTransition> {
        let due: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !e.ended)
            .filter(|e| monitor_past_deadline(&e.task, now_ms.saturating_sub(grace_ms) + 1))
            .map(|e| e.task.task_id.clone())
            .collect();
        due.iter()
            .filter_map(|id| self.end(id, Outcome::new(OutcomeStatus::Expired)))
            .collect()
    }

    /// Every task that started and has not ended, in start order.
    /// Excludes a non-persistent Monitor past `started_ms + timeout_ms`:
    /// a Monitor sometimes passes its deadline with no notice at all
    /// (2 of 3 unexplained Monitor orphans, recon §Q2); a persistent one
    /// has no deadline. `now_ms` is the caller's clock.
    #[must_use]
    pub fn outstanding(&self, now_ms: i64) -> Vec<&BackgroundTask> {
        self.entries
            .iter()
            .filter(|e| !e.ended)
            .map(|e| &e.task)
            .filter(|t| !monitor_past_deadline(t, now_ms))
            .collect()
    }

    /// The task, ended or not.
    #[must_use]
    pub fn task(&self, task_id: &str) -> Option<&BackgroundTask> {
        self.index.get(task_id).map(|&i| &self.entries[i].task)
    }

    /// Where the task's `.output` file is: the path its start printed,
    /// else `<dir>/<task_id>.output` with `<dir>` taken from any
    /// `.output` path seen in this session. Never reconstructed from the
    /// OS temp dir (recon §Q1: it differs per OS). `None` when neither
    /// is known.
    #[must_use]
    pub fn output_path(&self, task_id: &str) -> Option<PathBuf> {
        if let Some(path) = self.task(task_id).and_then(|t| t.output_file.as_deref()) {
            return Some(PathBuf::from(path));
        }
        let (dir, sep) = self.output_dir.as_ref()?;
        Some(PathBuf::from(format!("{dir}{sep}{task_id}.output")))
    }

    /// Ids of tasks the state has seen start (ended or not).
    #[must_use]
    pub fn known_ids(&self) -> HashSet<&str> {
        self.index.keys().map(String::as_str).collect()
    }
}

fn monitor_past_deadline(task: &BackgroundTask, now_ms: i64) -> bool {
    if task.kind != BackgroundKind::Monitor || task.persistent {
        return false;
    }
    match (task.started_ms, task.timeout_ms) {
        (Some(start), Some(timeout)) => {
            now_ms > start.saturating_add(i64::try_from(timeout).unwrap_or(i64::MAX))
        }
        _ => false,
    }
}

fn content_blocks(row: &Value) -> impl Iterator<Item = &Value> {
    row.get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// A `tool_result` block's text: its string content, or its text blocks
/// joined.
fn result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The path in "Output is being written to: <path>." when present. The
/// path runs to its `.output` suffix, so a dot or space inside a
/// directory name doesn't cut it short.
fn output_path_in(text: &str) -> Option<String> {
    const MARK: &str = "Output is being written to: ";
    let start = text.find(MARK)? + MARK.len();
    let len = text[start..].find(".output")? + ".output".len();
    Some(text[start..start + len].to_owned())
}

#[cfg(test)]
mod tests;
