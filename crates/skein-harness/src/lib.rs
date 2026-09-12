// Every fallible function here wraps one I/O or sqlite error type;
// documenting the variants per function is noise at this size.
#![allow(clippy::missing_errors_doc)]

//! `skein-harness` — how to find and read the session data that AI
//! coding harnesses keep on disk.
//!
//! Claude Code writes one JSONL transcript per session under
//! `~/.claude/projects`; opencode keeps a `SQLite` database under
//! `~/.local/share/opencode`. This crate knows the paths, the row and
//! table shapes, and the token/cost fields — and nothing about
//! Skein's UI or Tauri. The app and any standalone tool (usage
//! reports, cost post-mortems) read the same code, so a fix to the
//! parser lands in both. Issue #209.
//!
//! Everything is synchronous, read-only and local. The harnesses own
//! their stores; we never write to them.

pub mod agents;
pub mod claude;
pub mod opencode;
pub mod time;

use std::path::PathBuf;

/// The user's home directory — `USERPROFILE` on Windows, `HOME`
/// elsewhere. `None` when neither is set (some CI and sandboxes);
/// callers treat that as "the store does not exist".
pub fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key).map(PathBuf::from)
}
