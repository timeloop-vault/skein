//! What #213 actually promises.
//!
//! Two properties carry the design and are tested hardest:
//!
//! * **A token reaches exactly one room.** Not "usually" and not "as
//!   long as the ids don't collide" — every verb re-checks, so each one
//!   is exercised against a thread that belongs to somebody else.
//! * **The agent cannot resolve.** It is not enough for `resolve` to be
//!   missing from the tool list; a model that finds a tool absent goes
//!   looking for another way in. So it is refused by name, and the
//!   thread is checked to still be open afterwards.
//!
//! The rest is lifecycle: what a rotated token does, what an archived
//! room does, and whether two harnesses in one room can both answer.
//!
//! # Where the tests are
//!
//! This file holds the shared fixtures; the tests live in sibling files.
//!
//! | file | covers |
//! | :-- | :-- |
//! | `close_harness_tests.rs` | `close_harness` (#411): guards and happy paths |
//! | `close_room_tests.rs` | `close_room` (#411): guards, sign-off requirements and the frontend archive |
//! | `create_room_ceiling_tests.rs` | `create_room` (#330): the open-room ceiling and its per-repository grouping |
//! | `create_room_frontend_tests.rs` | `create_room` (#330): the prompt guard, frontend round trips and happy paths |
//! | `create_room_tests.rs` | `create_room` (#330): argument validation, kill switch and rate cap, plus the shared fixtures |
//! | `element_tests.rs` | element threads through the agent verbs (#435) |
//! | `find_rooms_tests.rs` | `find_rooms_for_path` (#354): matching, `safe_to_remove` and the MCP call |
//! | `frontend_request_tests.rs` | the frontend request round-trip (#328): the pending-request map |
//! | `hook_route_tests.rs` | the hook routes over real HTTP: permission (#86) and session-start (#273) |
//! | `http_route_tests.rs` | the endpoint over real HTTP: handshake, refused ways in, the `/api` routes |
//! | `mail_history_tests.rs` | `message_history` (#364): direction, filters, paging and scope |
//! | `mail_notice_tests.rs` | unread summary (#329) and the `message_in` / `message_out` rows |
//! | `mail_send_tests.rs` | `send_message` and `read_messages` (#327), plus the shared mail fixtures |
//! | `mcp_protocol_tests.rs` | the MCP envelope: initialize, notifications, refused tools, unknown methods |
//! | `open_harness_tests.rs` | `open_harness` (#411): guards and happy paths |
//! | `refusal_tests.rs` | the refusal-by-name set (approve, resolve, destroy, `archive_room`) and the tool list |
//! | `review_tests.rs` | `review_status`, replies, `mark_addressed`, listing; hosts `git_repo_with_commit` |
//! | `room_listing_tests.rs` | `list_rooms`, `get_room` and `list_harnesses` (#356) |
//! | `token_tests.rs` | tokens and scoping: a room token reaches exactly one room |

use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::auth::{self, Caller};
use super::mcp;
use super::state::AgentApiState;
use super::verbs::MailContext;
use crate::db::{Database, Harness, ReviewCommentRow, ReviewThreadRow, Room};

// ── fixtures ──────────────────────────────────────────────────────

pub(super) struct Fixture {
    _dir: TempDir,
    pub(super) db: Arc<Database>,
}

pub(super) fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("skein.db")).unwrap();
    Fixture {
        _dir: dir,
        db: Arc::new(db),
    }
}

pub(super) fn harness(id: &str, kind: &str, name: &str) -> Harness {
    Harness {
        id: id.to_owned(),
        kind: kind.to_owned(),
        name: name.to_owned(),
        status: "running".to_owned(),
        model: String::new(),
        tokens: "0".to_owned(),
        live: None,
        cmd: None,
        cwd: None,
        session_id: None,
        agent: None,
        design_entry: None,
        pending_notifications: None,
        created_by: None,
        shell_claim: None,
    }
}

pub(super) fn room(id: &str, harnesses: Vec<Harness>) -> Room {
    Room {
        id: id.to_owned(),
        name: format!("room {id}"),
        task: String::new(),
        status: "running".to_owned(),
        badge: 0,
        active_harness_id: harnesses.first().map(|h| h.id.clone()).unwrap_or_default(),
        harnesses,
        // No worktree: every test here is about scoping, attribution and
        // protocol, none of which needs git. The verbs that do need one
        // say so rather than pretending.
        cwd: None,
        branch: None,
        repo: None,
        archived: None,
        repo_root: None,
        attention: None,
        created_by: None,
        closed_by: None,
        retired: None,
        repo_identity: None,
    }
}

/// Persist rooms the way the frontend's autosave does.
pub(super) fn save(db: &Database, rooms: &[Room]) {
    db.load_all().unwrap();
    db.save_all(rooms).unwrap();
}

/// A line thread with one human comment on it.
pub(super) fn seed_thread(db: &Database, room_id: &str, thread_id: &str, body: &str) {
    db.insert_review_thread(&ReviewThreadRow {
        id: thread_id.to_owned(),
        room_id: room_id.to_owned(),
        scope: "line".to_owned(),
        file_path: Some("src/a.rs".to_owned()),
        commit_sha: None,
        side: Some("new".to_owned()),
        line_start: Some(10),
        line_end: Some(11),
        anchor_hash: None,
        anchor_lines: Some(json!(["let x = 1;", "let y = 2;"]).to_string()),
        resolved_ms: None,
        created_ms: 1,
        updated_ms: 1,
    })
    .unwrap();
    db.insert_review_comment(&ReviewCommentRow {
        id: format!("{thread_id}-c1"),
        thread_id: thread_id.to_owned(),
        room_id: room_id.to_owned(),
        author_kind: "user".to_owned(),
        author_id: None,
        body: body.to_owned(),
        created_ms: 1,
        updated_ms: 1,
    })
    .unwrap();
}

pub(super) fn caller_for(db: &Database, room_id: &str, harness_id: Option<&str>) -> Caller {
    let token = db.ensure_room_token(room_id, 1).unwrap();
    auth::authenticate(db, Some(&token), harness_id).unwrap()
}

pub(super) async fn handled(state: &AgentApiState, caller: &Caller, body: &Value) -> Value {
    match mcp::handle(state, caller, &body.to_string(), &MailContext::permissive()).await {
        mcp::Outcome::Json(v) => *v,
        other => panic!("expected a JSON-RPC response, got {other:?}"),
    }
}
pub(super) fn agent_api_state(f: &Fixture) -> AgentApiState {
    AgentApiState::for_test(Arc::clone(&f.db))
}
