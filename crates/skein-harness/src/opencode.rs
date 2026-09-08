//! opencode's on-disk session store: the `SQLite` database at
//! `~/.local/share/opencode/opencode.db`.
//!
//! Tables we read (schema as of opencode 1.18):
//!
//! - `session` — one row per session. `parent_id` links a subagent
//!   session to the session that spawned it; `agent` is the agent
//!   name it ran as; `model` is a JSON blob `{id, providerID,
//!   variant}`; `cost` and the `tokens_*` columns are opencode's own
//!   per-session roll-ups. `time_archived` is set when the user
//!   archives it.
//! - `message` — `data` is JSON. Assistant messages carry `role`,
//!   `agent`, `modelID`, `providerID`, `cost`, `tokens {input, output,
//!   reasoning, cache {read, write}}`, `time {created, completed}` and
//!   `path.cwd`.
//! - `part` — tool calls, step-finish, patches; read by Skein's
//!   Live Context and not typed here.
//!
//! The database is always opened read-only so we never contend with
//! a running opencode for the writer lock.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// `<home>/.local/share/opencode/opencode.db` — the same on every OS
/// (opencode does not use the platform data dir on Windows).
pub fn db_path(home: &Path) -> PathBuf {
    home.join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db")
}

/// Open the database read-only. Fails if it does not exist — callers
/// that treat "never ran opencode" as empty should check the path
/// first.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

/// Token counts as opencode records them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    /// Field-wise sum.
    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// Everything the model read: input + cache write + cache read.
    pub fn total_input(&self) -> u64 {
        self.input + self.cache_write + self.cache_read
    }
}

/// One `session` row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// The session that spawned this one, for subagent sessions.
    pub parent_id: Option<String>,
    pub project_id: String,
    pub directory: String,
    pub title: String,
    /// Agent name (`orchestrator`, `build`, `explore`, …).
    pub agent: Option<String>,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub model_variant: Option<String>,
    /// opencode's own USD estimate for the session.
    pub cost: f64,
    pub tokens: Tokens,
    pub time_created: i64,
    pub time_updated: i64,
    pub time_archived: Option<i64>,
}

impl Session {
    /// `<provider>/<model>` as opencode config names it, or `None`
    /// when the row records no model.
    pub fn model_ref(&self) -> Option<String> {
        let id = self.model_id.as_deref()?;
        Some(match self.provider_id.as_deref() {
            Some(p) => format!("{p}/{id}"),
            None => id.to_owned(),
        })
    }
}

const SESSION_COLUMNS: &str = "id, parent_id, project_id, directory, title, agent, model, cost, \
     tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, \
     time_created, time_updated, time_archived";

#[allow(clippy::cast_sign_loss)]
fn u64_col(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<u64> {
    Ok(row.get::<_, i64>(idx)?.max(0) as u64)
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    session_from_offset_row(row, 0)
}

/// One session by id, or a unique id prefix. `None` when nothing
/// matches or the prefix is ambiguous.
pub fn session(conn: &Connection, id_or_prefix: &str) -> Result<Option<Session>> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1 OR id LIKE ?2 \
         ORDER BY (id = ?1) DESC, length(id) LIMIT 2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Session> = stmt
        .query_map(
            params![id_or_prefix, format!("{id_or_prefix}%")],
            session_from_row,
        )?
        .collect::<rusqlite::Result<_>>()?;
    let unique_or_exact =
        rows.len() == 1 || rows.first().is_some_and(|first| first.id == id_or_prefix);
    Ok(if unique_or_exact {
        rows.into_iter().next()
    } else {
        None
    })
}

/// Ids of every non-archived session whose `directory` is exactly
/// `dir`, newest first.
pub fn session_ids_for_directory(conn: &Connection, dir: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM session \
         WHERE directory = ?1 AND time_archived IS NULL \
         ORDER BY time_created DESC",
    )?;
    let ids = stmt
        .query_map([dir], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Does a non-archived session with this exact id exist?
pub fn session_exists(conn: &Connection, id: &str) -> Result<bool> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM session WHERE id = ?1 AND time_archived IS NULL LIMIT 1")?;
    Ok(stmt.exists([id])?)
}

/// Top-level sessions (no parent), newest first. `directory_like`
/// narrows to sessions whose directory contains the substring.
pub fn root_sessions(conn: &Connection, directory_like: Option<&str>) -> Result<Vec<Session>> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM session WHERE parent_id IS NULL \
         AND (?1 IS NULL OR directory LIKE ?2) ORDER BY time_created DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let like = directory_like.map(|d| format!("%{d}%"));
    let rows = stmt
        .query_map(params![directory_like, like], session_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// A session and every session it (transitively) spawned, root
/// first, then by depth and creation time. `depth` is 0 for the root.
pub fn session_tree(conn: &Connection, root_id: &str) -> Result<Vec<(usize, Session)>> {
    let sql = format!(
        "WITH RECURSIVE tree(id, depth) AS ( \
           SELECT id, 0 FROM session WHERE id = ?1 \
           UNION ALL \
           SELECT s.id, t.depth + 1 FROM session s JOIN tree t ON s.parent_id = t.id \
         ) \
         SELECT t.depth, {cols} FROM tree t JOIN session s ON s.id = t.id \
         ORDER BY t.depth, s.time_created",
        cols = SESSION_COLUMNS
            .split(", ")
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([root_id], |row| {
            let depth = usize::try_from(row.get::<_, i64>(0)?).unwrap_or(0);
            // Session columns start at index 1 here; shift by
            // re-reading through a column-offset view.
            let session = session_from_offset_row(row, 1)?;
            Ok((depth, session))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn session_from_offset_row(row: &rusqlite::Row<'_>, off: usize) -> rusqlite::Result<Session> {
    let model_json: Option<String> = row.get(off + 6)?;
    let model: Option<Value> = model_json.and_then(|s| serde_json::from_str(&s).ok());
    let model_str = |key: &str| {
        model
            .as_ref()
            .and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Ok(Session {
        id: row.get(off)?,
        parent_id: row.get(off + 1)?,
        project_id: row.get(off + 2)?,
        directory: row.get(off + 3)?,
        title: row.get(off + 4)?,
        agent: row.get(off + 5)?,
        model_id: model_str("id"),
        provider_id: model_str("providerID"),
        model_variant: model_str("variant"),
        cost: row.get::<_, Option<f64>>(off + 7)?.unwrap_or(0.0),
        tokens: Tokens {
            input: u64_col(row, off + 8)?,
            output: u64_col(row, off + 9)?,
            reasoning: u64_col(row, off + 10)?,
            cache_read: u64_col(row, off + 11)?,
            cache_write: u64_col(row, off + 12)?,
        },
        time_created: row.get(off + 13)?,
        time_updated: row.get(off + 14)?,
        time_archived: row.get(off + 15)?,
    })
}

/// One assistant message — the per-API-call record, with the model
/// and tokens opencode attributed to it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub id: String,
    pub session_id: String,
    /// Agent the message ran under (`agent`, falling back to the
    /// older `mode` field).
    pub agent: Option<String>,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    /// opencode's own USD estimate for this call.
    pub cost: f64,
    pub tokens: Tokens,
    /// `time.created` in the JSON, else the row's `time_created`.
    pub time_created: i64,
    pub time_completed: Option<i64>,
    pub cwd: Option<String>,
}

impl AssistantMessage {
    /// Build from a `message.data` JSON blob. `None` unless
    /// `role == "assistant"`.
    pub fn from_data(
        id: &str,
        session_id: &str,
        row_time_created: i64,
        data: &Value,
    ) -> Option<Self> {
        if data.get("role").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        let s = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
        let n = |v: Option<&Value>, key: &str| {
            v.and_then(|x| x.get(key))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        let tokens = data.get("tokens");
        let cache = tokens.and_then(|t| t.get("cache"));
        let time = data.get("time");
        Some(Self {
            id: id.to_owned(),
            session_id: session_id.to_owned(),
            agent: s(data, "agent").or_else(|| s(data, "mode")),
            model_id: s(data, "modelID"),
            provider_id: s(data, "providerID"),
            cost: data.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
            tokens: Tokens {
                input: n(tokens, "input"),
                output: n(tokens, "output"),
                reasoning: n(tokens, "reasoning"),
                cache_read: n(cache, "read"),
                cache_write: n(cache, "write"),
            },
            time_created: time
                .and_then(|t| t.get("created"))
                .and_then(Value::as_i64)
                .unwrap_or(row_time_created),
            time_completed: time
                .and_then(|t| t.get("completed"))
                .and_then(Value::as_i64),
            cwd: data.get("path").and_then(|p| s(p, "cwd")),
        })
    }
}

/// Assistant messages, oldest first. `session_id = None` reads the
/// whole database — every message row is parsed, so on a large db
/// prefer a time window via `since_ms`.
pub fn assistant_messages(
    conn: &Connection,
    session_id: Option<&str>,
    since_ms: Option<i64>,
) -> Result<Vec<AssistantMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, time_created, data FROM message \
         WHERE (?1 IS NULL OR session_id = ?1) \
           AND (?2 IS NULL OR time_created >= ?2) \
           AND data LIKE '%\"role\":\"assistant\"%' \
         ORDER BY time_created, id",
    )?;
    let rows = stmt.query_map(params![session_id, since_ms], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, sid, ts, data) = row?;
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if let Some(m) = AssistantMessage::from_data(&id, &sid, ts, &value) {
            out.push(m);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unreadable_literal, clippy::float_cmp)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal copy of the opencode schema — the columns we read,
    /// plus `project_id` NOT NULL as in the real thing.
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
                directory TEXT NOT NULL, title TEXT NOT NULL, agent TEXT, model TEXT,
                cost REAL DEFAULT 0 NOT NULL,
                tokens_input INTEGER DEFAULT 0 NOT NULL, tokens_output INTEGER DEFAULT 0 NOT NULL,
                tokens_reasoning INTEGER DEFAULT 0 NOT NULL,
                tokens_cache_read INTEGER DEFAULT 0 NOT NULL,
                tokens_cache_write INTEGER DEFAULT 0 NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER
             );
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );",
        )
        .unwrap();
        let mut ins = conn
            .prepare(
                "INSERT INTO session (id, project_id, parent_id, directory, title, agent, model, cost, \
                 tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, \
                 time_created, time_updated, time_archived) \
                 VALUES (?1, 'p', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13)",
            )
            .unwrap();
        ins.execute(params![
            "ses_root",
            None::<String>,
            "/w/proj",
            "root",
            "orchestrator",
            r#"{"id":"claude-opus-5","providerID":"github-copilot"}"#,
            1.91,
            52,
            14502,
            1_387_289,
            136_764,
            1_788_179_429_005i64,
            None::<i64>
        ])
        .unwrap();
        ins.execute(params![
            "ses_child",
            Some("ses_root"),
            "/w/proj",
            "Commit (@scribe subagent)",
            "scribe",
            r#"{"id":"claude-haiku-4.5","providerID":"github-copilot","variant":"default"}"#,
            0.093,
            91,
            2214,
            432_073,
            30_921,
            1_788_180_611_902i64,
            None::<i64>
        ])
        .unwrap();
        ins.execute(params![
            "ses_grand",
            Some("ses_child"),
            "/w/proj",
            "nested",
            "explore",
            None::<String>,
            0.0,
            0,
            0,
            0,
            0,
            1_788_180_700_000i64,
            None::<i64>
        ])
        .unwrap();
        ins.execute(params![
            "ses_old",
            None::<String>,
            "/w/other",
            "archived",
            "build",
            None::<String>,
            0.0,
            0,
            0,
            0,
            0,
            1_700_000_000_000i64,
            Some(1_700_000_001_000i64)
        ])
        .unwrap();
        drop(ins);

        let assistant = json!({
            "parentID": "msg_u", "role": "assistant", "mode": "orchestrator",
            "agent": "orchestrator",
            "path": {"cwd": "/w/proj", "root": "/w/proj"},
            "cost": 0.0447725,
            "tokens": {"total": 74166, "input": 2, "output": 229, "reasoning": 0,
                       "cache": {"write": 360, "read": 73575}},
            "modelID": "claude-opus-5", "providerID": "github-copilot",
            "time": {"created": 1_788_180_667_414i64, "completed": 1_788_180_672_150i64},
            "finish": "stop"
        });
        let user = json!({"role": "user", "agent": "orchestrator"});
        for (id, sid, ts, data) in [
            ("msg_1", "ses_root", 1_788_180_667_000i64, &assistant),
            ("msg_2", "ses_root", 1_788_180_600_000i64, &user),
            ("msg_3", "ses_child", 1_788_180_650_000i64, &assistant),
        ] {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?3, ?4)",
                params![id, sid, ts, data.to_string()],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn db_path_is_under_local_share() {
        assert_eq!(
            db_path(Path::new("/h")),
            Path::new("/h/.local/share/opencode/opencode.db")
        );
    }

    #[test]
    fn session_row_is_typed_with_model_json() {
        let conn = fixture();
        let s = session(&conn, "ses_root").unwrap().unwrap();
        assert_eq!(s.agent.as_deref(), Some("orchestrator"));
        assert_eq!(
            s.model_ref().as_deref(),
            Some("github-copilot/claude-opus-5")
        );
        assert_eq!(s.tokens.cache_read, 1_387_289);
        assert!((s.cost - 1.91).abs() < 1e-9);
        assert!(s.parent_id.is_none());

        let child = session(&conn, "ses_child").unwrap().unwrap();
        assert_eq!(child.parent_id.as_deref(), Some("ses_root"));
        assert_eq!(child.model_variant.as_deref(), Some("default"));

        let bare = session(&conn, "ses_grand").unwrap().unwrap();
        assert!(bare.model_ref().is_none());
    }

    #[test]
    fn session_accepts_unique_prefix_only() {
        let conn = fixture();
        assert_eq!(session(&conn, "ses_ro").unwrap().unwrap().id, "ses_root");
        assert!(session(&conn, "ses_").unwrap().is_none());
        assert!(session(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn directory_queries_skip_archived_sessions() {
        let conn = fixture();
        let ids = session_ids_for_directory(&conn, "/w/proj").unwrap();
        assert_eq!(ids, ["ses_grand", "ses_child", "ses_root"]);
        assert!(
            session_ids_for_directory(&conn, "/w/other")
                .unwrap()
                .is_empty()
        );
        assert!(session_exists(&conn, "ses_root").unwrap());
        assert!(!session_exists(&conn, "ses_old").unwrap());
    }

    #[test]
    fn root_sessions_can_filter_by_directory() {
        let conn = fixture();
        let all = root_sessions(&conn, None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "ses_root");
        let proj = root_sessions(&conn, Some("proj")).unwrap();
        assert_eq!(proj.len(), 1);
    }

    #[test]
    fn session_tree_walks_parent_links() {
        let conn = fixture();
        let tree = session_tree(&conn, "ses_root").unwrap();
        let shape: Vec<(usize, &str)> = tree.iter().map(|(d, s)| (*d, s.id.as_str())).collect();
        assert_eq!(shape, [(0, "ses_root"), (1, "ses_child"), (2, "ses_grand")]);
        assert_eq!(tree[1].1.agent.as_deref(), Some("scribe"));
    }

    #[test]
    fn assistant_messages_parse_tokens_and_skip_user_rows() {
        let conn = fixture();
        let all = assistant_messages(&conn, None, None).unwrap();
        assert_eq!(all.len(), 2);
        let root_only = assistant_messages(&conn, Some("ses_root"), None).unwrap();
        assert_eq!(root_only.len(), 1);
        let m = &root_only[0];
        assert_eq!(m.model_id.as_deref(), Some("claude-opus-5"));
        assert_eq!(m.agent.as_deref(), Some("orchestrator"));
        assert_eq!(m.tokens.cache_read, 73_575);
        assert_eq!(m.tokens.cache_write, 360);
        assert_eq!(m.time_created, 1_788_180_667_414);
        assert_eq!(m.cwd.as_deref(), Some("/w/proj"));
        let recent = assistant_messages(&conn, None, Some(1_788_180_660_000)).unwrap();
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn open_read_only_refuses_a_missing_file() {
        assert!(open_read_only(Path::new("/definitely/missing/opencode.db")).is_err());
    }
}
