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
//! | `close_harness_tests.rs` | `close_harness` (#411): guards and the daemon-side removal (#734) |
//! | `close_room_tests.rs` | `close_room` (#411): guards, sign-off requirements and the daemon-side archive (#734) |
//! | `create_room_ceiling_tests.rs` | `create_room` (#330): the open-room ceiling and its per-repository grouping |
//! | `create_room_spawn_tests.rs` | `create_room` (#734): with a real daemon and no client, the new harness is started |
//! | `create_room_stored_tests.rs` | `create_room` (#330, #734): the stored room, worktree, refusals, prompt and the created event |
//! | `create_room_tests.rs` | `create_room` (#330): argument validation, kill switch and rate cap, plus the shared fixtures |
//! | `element_tests.rs` | element threads through the agent verbs (#435) |
//! | `find_rooms_tests.rs` | `find_rooms_for_path` (#354): matching, `safe_to_remove` and the MCP call |
//! | `frontend_request_tests.rs` | the frontend request round-trip (#328): the pending-request map |
//! | `hook_route_tests.rs` | the hook routes over real HTTP: permission (#86) and session-start (#273) |
//! | `http_route_tests.rs` | the endpoint over real HTTP: handshake, refused ways in, the `/api` routes |
//! | `info_tests.rs` | `skein_info` (#535): MCP and `/api/info` agree with `build_info` |
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
use super::daemon_handles::DaemonHandles;
use super::mcp;
use super::state::AgentApiState;
use super::verbs::MailContext;
use crate::db::{Database, Harness, ReviewCommentRow, ReviewThreadRow, Room};
use crate::room_store::RoomStore;

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
        design_device: None,
        pending_notifications: None,
        created_by: None,
        shell_claim: None,
        remote: None,
        env_profile: None,
        hermes_profile: None,
        opencode_version: None,
        launch: None,
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
        todos: None,
        revision: 0,
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

struct NoEvents;

impl crate::event_sink::EventSink for NoEvents {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

/// A loaded [`RoomStore`] over `db` holding one open room per id, which are
/// also saved to the db (so `authenticate` finds them). Its debounced write
/// is set far out so it cannot fire mid-assertion.
pub(super) fn store_with(db: &Arc<Database>, ids: &[&str]) -> Arc<RoomStore> {
    let rooms: Vec<Room> = ids.iter().map(|id| room(id, vec![])).collect();
    store_of_rooms(db, rooms)
}

/// [`store_with`] for rooms the test built itself.
pub(super) fn store_of_rooms(db: &Arc<Database>, rooms: Vec<Room>) -> Arc<RoomStore> {
    save(db, &rooms);
    let store = RoomStore::with_delays(
        Arc::clone(db),
        Arc::new(NoEvents),
        std::time::Duration::from_secs(3600),
        std::time::Duration::from_secs(3600),
    );
    store.fill(rooms).unwrap();
    store
}

/// [`agent_api_state`] with a filled room store attached through the daemon
/// handles (no runner, no client), plus that store.
pub(super) fn agent_api_state_with_rooms(
    f: &Fixture,
    ids: &[&str],
) -> (AgentApiState, Arc<RoomStore>) {
    let state = agent_api_state(f);
    let store = store_with(&f.db, ids);
    attach(&state, &store);
    (state, store)
}

/// [`agent_api_state_with_rooms`] over rooms the test built, saved to the db
/// and the store alike, with the token-revoking observer registered.
pub(super) fn agent_api_state_with_room_records(
    f: &Fixture,
    rooms: Vec<Room>,
) -> (AgentApiState, Arc<RoomStore>) {
    let state = agent_api_state(f);
    let store = store_of_rooms(&f.db, rooms);
    super::token_lifecycle::revoke_tokens_of_closed_rooms(&store, &f.db);
    attach(&state, &store);
    (state, store)
}

fn attach(state: &AgentApiState, store: &Arc<RoomStore>) {
    assert!(
        state
            .set_daemon_handles(DaemonHandles {
                rooms: Arc::clone(store),
                runner: None,
                harness_config: None,
                verb_lock: tokio::sync::Mutex::new(()),
            })
            .is_ok()
    );
}
