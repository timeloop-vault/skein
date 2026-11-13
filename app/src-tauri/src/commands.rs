//! The Tauri commands the frontend invokes, grouped by what they act on.
//! (Commands for larger subsystems live beside them: `git`, `fs`,
//! `review`, `review_surface::commands`, `agent_api::commands`, …)
//!
//! Registration stays one list in `lib.rs`: `generate_handler!` is one
//! proc-macro invocation producing one invoke handler, and
//! `Builder::invoke_handler` replaces rather than merges, so per-module
//! handler lists can't be composed — commands are grouped by module path
//! in the single list instead.
//!
//! # Where things are
//!
//! - `app` — `ping` and `frontend_log` (#362).
//! - `pty` — write / resize / opencode scan.
//! - `harness_runner` — ensure-running / stop / detach over the daemon's
//!   `HarnessRunner` (#731).
//! - `harness_events` — attach/detach for Claude's JSONL tail (#50 L2c-1)
//!   and opencode's SSE stream (L2c-2).
//! - `harness_log` — the `harness_events` and `harness_actions` tables:
//!   record and recent-rows queries.
//! - `harness_phase` — client-only inputs to the daemon's shadow phase
//!   machine (#732).
//! - `fs` — the Files overlay's list / read / write commands (#185, #409).
//! - `git` — the git commands: repo inspection, worktrees, status, diff and
//!   the two watchers.
//! - `hermes_work` — attach/detach for the Hermes work poller (#628).
//! - `rooms` — `db_load_rooms` / `rooms_apply` / `rooms_flush`.
//! - `todos` — `db_load_global_todos` / `db_save_global_todos` (#335).
//! - `spawn_env` — `SpawnEnvState`, the Settings environment commands,
//!   agent discovery, and the default shell / home / cwd lookups.

pub(crate) mod app;
pub(crate) mod fs;
pub(crate) mod git;
pub(crate) mod harness_argv;
pub(crate) mod harness_events;
pub(crate) mod harness_log;
pub(crate) mod harness_phase;
pub(crate) mod harness_runner;
pub(crate) mod hermes_work;
pub(crate) mod pty;
pub(crate) mod rooms;
pub(crate) mod spawn_env;
pub(crate) mod todos;
