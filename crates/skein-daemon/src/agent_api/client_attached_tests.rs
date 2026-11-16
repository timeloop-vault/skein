//! "A client is attached" means a client can answer right now, not that the
//! seam object exists (#734): `ClientRequests::is_attached`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::client::ClientRequests;
use super::design_tests::{design_room, folder};
use super::state::{AgentApiState, AgentRequest};
use super::tests::{
    agent_api_state, agent_api_state_with_room_records, caller_for, fixture, harness, room, save,
};
use super::verbs::{self, DesignTargetArgs, VerbError};

/// Counts the requests it is handed; fails each one so no answer is awaited.
struct Fake {
    attached: AtomicBool,
    sent: AtomicUsize,
}

impl Fake {
    fn new(attached: bool) -> Arc<Self> {
        Arc::new(Self {
            attached: AtomicBool::new(attached),
            sent: AtomicUsize::new(0),
        })
    }

    fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }
}

impl ClientRequests for Fake {
    fn is_attached(&self) -> bool {
        self.attached.load(Ordering::SeqCst)
    }

    fn send(&self, _request: &AgentRequest) -> Result<(), String> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        Err("gone".to_owned())
    }
}

fn with_client(mut state: AgentApiState, client: &Arc<Fake>) -> AgentApiState {
    state.client = Some(client.clone());
    state
}

fn no_target() -> DesignTargetArgs {
    DesignTargetArgs { harness: None }
}

#[test]
fn has_client_needs_an_attached_client() {
    let f = fixture();
    let fake = Fake::new(false);
    let state = with_client(agent_api_state(&f), &fake);
    assert!(!state.has_client());
    fake.attached.store(true, Ordering::SeqCst);
    assert!(state.has_client());
}

#[tokio::test]
async fn a_design_verb_refuses_no_client_without_sending_when_not_attached() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let fake = Fake::new(false);
    let state = with_client(agent_api_state(&f), &fake);
    let err = verbs::get_design_state(&state, &caller, &no_target())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Unavailable(m) | VerbError::Refused(m) if m.contains("no_client")),
        "{err:?}"
    );
    assert_eq!(fake.sent(), 0);
}

#[tokio::test]
async fn a_design_verb_sends_when_attached() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let fake = Fake::new(true);
    let state = with_client(agent_api_state(&f), &fake);
    let err = verbs::get_design_state(&state, &caller, &no_target())
        .await
        .unwrap_err();
    assert!(
        !matches!(&err, VerbError::Unavailable(m) | VerbError::Refused(m) if m.contains("no_client")),
        "{err:?}"
    );
    assert_eq!(fake.sent(), 1);
}

#[tokio::test]
async fn close_harness_of_a_files_harness_proceeds_without_asking_when_not_attached() {
    let f = fixture();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("hf", "files", "files"),
            ],
        )],
    );
    let fake = Fake::new(false);
    let state = with_client(state, &fake);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let args = verbs::CloseHarnessArgs {
        harness: "hf".to_owned(),
    };
    verbs::close_harness(&state, &caller, &args, true)
        .await
        .unwrap();
    assert_eq!(fake.sent(), 0);
    let r1 = store.rooms().into_iter().find(|r| r.id == "r1").unwrap();
    assert_eq!(r1.harnesses.len(), 1);
}
