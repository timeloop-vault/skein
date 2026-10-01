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
//! - `pty` — spawn / write / resize / kill, and `pick_free_port`.
//! - `harness_events` — attach/detach for Claude's JSONL tail (#50 L2c-1)
//!   and opencode's SSE stream (L2c-2).
//! - `harness_log` — the `harness_events` and `harness_actions` tables:
//!   record and recent-rows queries.
//! - `rooms` — `db_load_rooms` / `db_save_rooms`.
//! - `spawn_env` — `SpawnEnvState`, the Settings environment commands,
//!   agent discovery, and the default shell / home / cwd lookups.

pub(crate) mod app;
pub(crate) mod harness_events;
pub(crate) mod harness_log;
pub(crate) mod pty;
pub(crate) mod rooms;
pub(crate) mod spawn_env;
