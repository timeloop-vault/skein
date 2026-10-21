//! Hermes' on-disk session store: the `SQLite` database `state.db`
//! under `HERMES_HOME` (issue #624).
//!
//! Tables we read (schema as of Hermes v0.21.7):
//!
//! - `sessions` — one row per session. `source` is `tui` for the TUI;
//!   `parent_session_id` is non-null on a compression child;
//!   `started_at` is epoch SECONDS (REAL) of the FIRST PROMPT, not of
//!   launch; `cwd` is `os.getcwd()` in the platform's own form;
//!   `archived` is 0/1.
//!
//! The database is WAL and shared with running Hermes processes, so it
//! is opened strictly read-only on a short-lived connection with a
//! small busy timeout: a locked db fails fast and callers degrade. A
//! missing file means "Hermes never ran here" — no sessions, not an
//! error.
//!
//! # Claiming a session for a harness
//!
//! The session id is `YYYYMMDD_HHMMSS_<6 hex>`, stamped in LOCAL time
//! at process LAUNCH, but the row only appears at the first prompt and
//! `started_at` is that prompt's time. Two Hermes harnesses in one
//! worktree can therefore prompt in the opposite order to their spawn
//! order, and prompt order cannot tell them apart; the id's launch
//! stamp follows spawn order, so [`assign_sessions`] pairs on that.
//!
//! Limits: a Hermes launched from a plain terminal in the same cwd
//! after the harness spawned, and prompted before the harness, can be
//! claimed by the harness (nothing in the row tells them apart). The
//! stamp is compared using the UTC offset at spawn time
//! (`local_minus_utc_minutes`), so a launch across a DST transition is
//! off by an hour and does not match; the harness then stays unbound
//! rather than claiming wrongly.

pub mod hooks;
pub mod messages;
pub mod version;
pub mod work;

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use thiserror::Error;

use crate::time::days_from_civil;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// How long a reader waits on a locked database before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Hermes launches after Skein spawns it; the id stamp truncates to
/// the second, so a launch stamp may read up to this much BEFORE the
/// spawn time and still be that spawn's.
const STAMP_TRUNCATION_MS: i64 = 1_000;

/// Looser bound for clock skew between Skein's clock and the stamp's.
pub const CLOCK_SKEW_MS: i64 = 5_000;

/// Pure `HERMES_HOME` resolution: a non-empty `HERMES_HOME` wins, else
/// `%LOCALAPPDATA%\hermes` on Windows, else `<home>/.hermes`.
///
/// `-p <profile>` (#627) changes the home: see [`profile_home`].
pub fn hermes_home_from(
    env_hermes_home: Option<&OsStr>,
    local_app_data: Option<&OsStr>,
    home: &Path,
    windows: bool,
) -> PathBuf {
    if let Some(h) = env_hermes_home.filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    if windows && let Some(l) = local_app_data.filter(|l| !l.is_empty()) {
        return PathBuf::from(l).join("hermes");
    }
    home.join(".hermes")
}

/// Names Hermes refuses for a profile (`_RESERVED_NAMES` in
/// `hermes_cli/profiles.py`); `default` is handled separately because
/// it selects the root.
const RESERVED_PROFILE_NAMES: [&str; 5] = ["hermes", "test", "tmp", "root", "sudo"];

/// Hermes' `normalize_profile_name`: trimmed, lower-cased.
fn normalize_profile(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether `-p <name>` would be accepted by Hermes (after its own
/// strip + lower-case). Mirrors `PROFILE_ID_RE`
/// (`^[a-z0-9][a-z0-9_-]{0,63}$`, `hermes_constants.py`) and
/// `validate_profile_name` (`hermes_cli/profiles.py`): `default` is
/// valid (it means the root); the other reserved names are not.
pub fn valid_profile_name(name: &str) -> bool {
    let n = normalize_profile(name);
    if n == "default" {
        return true;
    }
    let mut bytes = n.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    first_ok
        && n.len() <= 64
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && !RESERVED_PROFILE_NAMES.contains(&n.as_str())
}

/// The `HERMES_HOME` that `-p <profile>` selects under `root`: `None`
/// or `default` is the root itself, anything else
/// `<root>/profiles/<name>`. The name is not validated here; check
/// [`valid_profile_name`] first.
pub fn profile_home(root: &Path, profile: Option<&str>) -> PathBuf {
    match profile.map(normalize_profile) {
        None => root.to_path_buf(),
        Some(n) if n == "default" => root.to_path_buf(),
        Some(n) => root.join("profiles").join(n),
    }
}

/// Hermes' home on this machine. `None` when no home directory can be
/// found (callers treat that as "no store").
pub fn hermes_home() -> Option<PathBuf> {
    let env = std::env::var_os("HERMES_HOME");
    let local = std::env::var_os("LOCALAPPDATA");
    let home = crate::home_dir().unwrap_or_default();
    let resolved = hermes_home_from(env.as_deref(), local.as_deref(), &home, cfg!(windows));
    // Without any anchor the fallback would be a bare relative `.hermes`.
    if resolved.is_relative() {
        return None;
    }
    Some(resolved)
}

/// `<home>/state.db`.
pub fn state_db_path(home: &Path) -> PathBuf {
    home.join("state.db")
}

/// A session that may belong to one of our harnesses.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCandidate {
    pub id: String,
    /// Epoch seconds of the first prompt.
    pub started_at: f64,
}

/// A Hermes harness still waiting to learn its session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHarness {
    pub harness_id: String,
    pub spawned_at_ms: i64,
}

/// Open strictly read-only with a short busy timeout. `None` when the
/// file does not exist.
pub fn open(db: &Path) -> Result<Option<Connection>> {
    if !db.is_file() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(Some(conn))
}

/// Whether a live (non-archived) session with this id exists. A
/// missing database is `Ok(false)`.
pub fn session_exists(db: &Path, id: &str) -> Result<bool> {
    let Some(conn) = open(db)? else {
        return Ok(false);
    };
    let mut stmt = conn.prepare("SELECT 1 FROM sessions WHERE id = ?1 AND archived = 0")?;
    Ok(stmt.exists(params![id])?)
}

/// TUI root sessions started at or after `since_epoch_secs` whose cwd
/// is `cwd`. A missing database is `Ok(vec![])`.
pub fn tui_sessions_in_cwd(
    db: &Path,
    cwd: &str,
    since_epoch_secs: f64,
) -> Result<Vec<SessionCandidate>> {
    let Some(conn) = open(db)? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT id, started_at, cwd FROM sessions \
         WHERE source = 'tui' AND archived = 0 AND parent_session_id IS NULL \
         AND started_at >= ?1 AND cwd IS NOT NULL ORDER BY started_at, id",
    )?;
    let rows = stmt.query_map(params![since_epoch_secs], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, f64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let windows = cfg!(windows);
    let mut out = Vec::new();
    for row in rows {
        let (id, started_at, row_cwd) = row?;
        if cwd_matches(&row_cwd, cwd, windows) {
            out.push(SessionCandidate { id, started_at });
        }
    }
    Ok(out)
}

/// Whether two cwd strings name the same folder. On Windows `/` and
/// `\` are alike and case is ignored; one trailing separator is
/// stripped on both sides.
pub fn cwd_matches(a: &str, b: &str, windows: bool) -> bool {
    fn norm(s: &str, windows: bool) -> String {
        let s = if windows {
            s.replace('/', "\\").to_lowercase()
        } else {
            s.to_string()
        };
        let sep = if windows { '\\' } else { '/' };
        match s.strip_suffix(sep) {
            Some(rest) if !rest.is_empty() => rest.to_string(),
            _ => s,
        }
    }
    norm(a, windows) == norm(b, windows)
}

/// The launch time encoded in a session id, as UTC epoch ms.
/// `local_minus_utc_minutes` is the zone offset in force at that time
/// (e.g. +120 for UTC+2). `None` for a malformed id.
#[allow(clippy::many_single_char_names)]
pub fn launch_epoch_ms(id: &str, local_minus_utc_minutes: i32) -> Option<i64> {
    let b = id.as_bytes();
    if b.len() != 22 || b[8] != b'_' || b[15] != b'_' {
        return None;
    }
    if !b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit)
        || !b[16..].iter().all(u8::is_ascii_hexdigit)
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| id.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(4..6)?, n(6..8)?);
    let (h, mi, s) = (n(9..11)?, n(11..13)?, n(13..15)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let local_ms = days_from_civil(y, mo, d) * 86_400_000 + (h * 3600 + mi * 60 + s) * 1000;
    Some(local_ms - i64::from(local_minus_utc_minutes) * 60_000)
}

/// The claim rule: pair candidate sessions with pending harnesses.
///
/// Claimed and unparseable candidates are dropped; the rest go in
/// launch order (tie: id). Each takes, among harnesses still
/// unassigned, the one spawned LATEST at or before its launch (plus the
/// stamp's one-second truncation); failing that, the closest within
/// [`CLOCK_SKEW_MS`]. A harness gets its earliest session only, and a
/// session goes to one harness.
pub fn assign_sessions<S: std::hash::BuildHasher>(
    candidates: &[SessionCandidate],
    pending: &[PendingHarness],
    claimed: &HashSet<String, S>,
    local_minus_utc_minutes: i32,
) -> HashMap<String, String> {
    let mut launches: Vec<(i64, &str)> = candidates
        .iter()
        .filter(|c| !claimed.contains(&c.id))
        .filter_map(|c| {
            launch_epoch_ms(&c.id, local_minus_utc_minutes).map(|ms| (ms, c.id.as_str()))
        })
        .collect();
    launches.sort_unstable();

    let mut free: Vec<&PendingHarness> = pending.iter().collect();
    let mut out = HashMap::new();
    for (launch, id) in launches {
        let tight = free
            .iter()
            .enumerate()
            .filter(|(_, p)| p.spawned_at_ms <= launch + STAMP_TRUNCATION_MS)
            .max_by_key(|(_, p)| p.spawned_at_ms)
            .map(|(i, _)| i);
        let pick = tight.or_else(|| {
            free.iter()
                .enumerate()
                .filter(|(_, p)| p.spawned_at_ms <= launch + CLOCK_SKEW_MS)
                .min_by_key(|(_, p)| (p.spawned_at_ms - launch).abs())
                .map(|(i, _)| i)
        });
        if let Some(i) = pick {
            let p = free.remove(i);
            out.insert(p.harness_id.clone(), id.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests;
