//! #733 acceptance: delivery with no client attached. The real phase
//! service, store, database, agent-API mail path and delivery worker; only
//! the PTY is a recorder. Nothing here is a client: no event consumer, no
//! client inputs, no frontend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use skein_proto::rooms::{Harness, LaunchMode, Room};
use tempfile::TempDir;

use super::daemon_env::{DaemonEnv, PtyPort};
use super::mail_nudge::mail_nudge_text;
use super::paste::paste_bytes;
use super::service::{DeliveryHandle, DeliveryService};
use super::shared::Shared;
use crate::agent_api::AgentApiState;
use crate::db::{Database, HarnessMessageRow};
use crate::event_sink::{EventSink, SharedEventSink};
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::{PhaseService, now_ms};
use crate::room_store::RoomStore;

const ROOM: &str = "r2";
const HARNESS: &str = "h2";

#[derive(Default)]
struct TestSink(Mutex<Vec<(String, String)>>);

impl EventSink for TestSink {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        self.0.lock().push((event.to_owned(), payload));
        Ok(())
    }
}

/// A PTY that records writes.
struct RecPty {
    live: AtomicBool,
    writes: Mutex<Vec<Vec<u8>>>,
}

impl PtyPort for RecPty {
    fn live_pty(&self, _: &str) -> Option<String> {
        self.live.load(Ordering::SeqCst).then(|| "pty-1".to_owned())
    }
    fn write(&self, _: &str, bytes: &[u8]) -> bool {
        self.writes.lock().push(bytes.to_vec());
        true
    }
}

fn harness(id: &str, launch: Option<LaunchMode>) -> Harness {
    Harness {
        id: id.into(),
        kind: "claude".into(),
        name: id.into(),
        status: "running".into(),
        model: String::new(),
        tokens: "0".into(),
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
        launch,
    }
}

fn room(id: &str, harnesses: Vec<Harness>) -> Room {
    Room {
        id: id.into(),
        name: format!("room {id}"),
        task: String::new(),
        status: "idle".into(),
        badge: 0,
        active_harness_id: harnesses.first().map(|h| h.id.clone()).unwrap_or_default(),
        harnesses,
        cwd: None,
        branch: None,
        repo: None,
        archived: None,
        repo_root: None,
        attention: None,
        created_by: None,
        closed_by: None,
        todos: None,
        retired: None,
        repo_identity: None,
    }
}

struct Rig {
    phase: Arc<PhaseService>,
    db: Arc<Database>,
    api: AgentApiState,
    handle: DeliveryHandle,
    pty: Arc<RecPty>,
    env: DaemonEnv,
    sink: Arc<TestSink>,
    _dir: TempDir,
}

impl Rig {
    fn new(launch: Option<LaunchMode>) -> Self {
        let (dir, db) = crate::db::test_support::fresh_db();
        let db = Arc::new(db);
        let rooms = vec![
            room("r1", vec![harness("h1", None)]),
            room(ROOM, vec![harness(HARNESS, launch)]),
        ];
        db.save_all(&rooms).unwrap();
        let sink = Arc::new(TestSink::default());
        let events: SharedEventSink = sink.clone();
        let store = RoomStore::new(Arc::clone(&db), events.clone());
        store.fill(rooms).unwrap();
        let phase = PhaseService::with_clock(Arc::new(now_ms), Arc::clone(&db), events.clone());
        let pty = Arc::new(RecPty {
            live: AtomicBool::new(true),
            writes: Mutex::new(Vec::new()),
        });
        let shared = Arc::new(Shared::default());
        let env = DaemonEnv {
            store: Arc::clone(&store),
            phase: Arc::clone(&phase),
            db: Arc::clone(&db),
            events: events.clone(),
            pty: pty.clone(),
            shared: Arc::clone(&shared),
        };
        let test_env = DaemonEnv {
            store: Arc::clone(&env.store),
            phase: Arc::clone(&env.phase),
            db: Arc::clone(&env.db),
            events: env.events.clone(),
            pty: env.pty.clone(),
            shared: Arc::clone(&shared),
        };
        let handle = DeliveryService::start(Box::new(env), Arc::clone(&shared), phase.clock());
        phase.set_listener(handle.phase_listener());
        let api = AgentApiState::for_test(Arc::clone(&db));
        let mail = handle.clone();
        api.set_mail_hook(Arc::new(move |_, harness_id| {
            mail.notify(super::DeliveryEvent::MailChanged {
                harness_id: harness_id.to_owned(),
            });
        }));
        Self {
            phase,
            db,
            api,
            handle,
            pty,
            env: test_env,
            sink,
            _dir: dir,
        }
    }

    /// What a real Claude adapter does: attach, then a turn that ends.
    fn running(&self) {
        self.phase.spawned(HARNESS, ROOM, true);
        self.phase.claude_attached(HARNESS);
        self.phase
            .claude_event(HARNESS, &ClaudeEvent::AssistantTurn);
    }

    fn end_turn(&self) {
        self.phase
            .claude_event(HARNESS, &ClaudeEvent::AwaitingPrompt);
    }

    fn waiting(&self) {
        self.running();
        self.end_turn();
    }

    /// Store a row and tell the daemon, the way the HTTP and MCP routes do.
    fn mail(&self, n: u32) {
        self.db
            .insert_harness_message(&HarnessMessageRow {
                id: format!("m{n}"),
                room_id: ROOM.into(),
                harness_id: HARNESS.into(),
                from_room_id: "r1".into(),
                from_harness_id: Some("h1".into()),
                body: format!("hello {n}"),
                created_ms: now_ms() + i64::from(n),
                read_ms: None,
            })
            .unwrap();
        self.api.notify_mail_changed(ROOM, HARNESS);
    }

    fn writes(&self) -> Vec<Vec<u8>> {
        self.pty.writes.lock().clone()
    }

    /// The worker has handled everything sent before this call.
    fn barrier(&self) {
        assert!(self.handle.send_manual("no-such-harness", "x").is_err());
    }

    fn wait_for_writes(&self, n: usize) {
        let end = Instant::now() + Duration::from_secs(5);
        while self.writes().len() < n && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(self.writes().len(), n, "writes: {:?}", self.writes());
    }

    fn assert_delivered_once(&self) {
        self.wait_for_writes(2);
        let w = self.writes();
        let text = mail_nudge_text(1, &["room r1".to_owned()]);
        assert_eq!(w[0], paste_bytes(&text, false));
        assert_eq!(w[1], b"\r");
    }
}

#[test]
fn mail_for_a_waiting_harness_is_delivered_once_with_no_client() {
    let r = Rig::new(None);
    r.waiting();
    r.mail(1);
    r.assert_delivered_once();
    // An unrelated change to the same mailbox writes nothing more.
    r.api.notify_mail_changed(ROOM, HARNESS);
    r.barrier();
    std::thread::sleep(Duration::from_millis(300));
    r.barrier();
    assert_eq!(r.writes().len(), 2, "{:?}", r.writes());
    // No client ever consumed an event or sent an input; the daemon recorded
    // its own submit and only the phase stream went out.
    assert!(r.phase.get(HARNESS).unwrap().last_submit_at.is_some());
    assert_eq!(r.handle.held_snapshot().len(), 0);
}

#[test]
fn a_delivered_nudge_announces_its_paste_then_its_submit() {
    let r = Rig::new(None);
    r.waiting();
    r.mail(1);
    r.assert_delivered_once();
    let seam: Vec<_> = r
        .sink
        .0
        .lock()
        .iter()
        .filter(|(e, _)| e == "skein://delivery-seam")
        .map(|(_, p)| p.clone())
        .collect();
    assert_eq!(
        seam,
        vec![
            r#"{"harnessId":"h2","event":"paste"}"#.to_owned(),
            r#"{"harnessId":"h2","event":"submit"}"#.to_owned()
        ]
    );
}

#[test]
fn mail_waits_for_a_running_harness_then_goes_once_when_it_turns_waiting() {
    let r = Rig::new(None);
    r.running();
    r.mail(1);
    r.barrier();
    std::thread::sleep(Duration::from_millis(300));
    r.barrier();
    assert_eq!(r.writes().len(), 0);
    r.end_turn();
    r.assert_delivered_once();
    r.barrier();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.writes().len(), 2);
}

#[test]
fn a_post_exit_shell_gets_no_mail() {
    let r = Rig::new(Some(LaunchMode::Shell {
        argv: vec!["sh".into()],
    }));
    r.waiting();
    r.mail(1);
    r.barrier();
    std::thread::sleep(Duration::from_millis(300));
    r.barrier();
    assert_eq!(r.writes().len(), 0, "{:?}", r.writes());
}

#[test]
fn a_permission_dialog_gets_no_mail() {
    let r = Rig::new(None);
    r.running();
    r.phase
        .claude_permission_hook(HARNESS, Some("Bash"), None, None);
    assert_eq!(
        r.phase.get(HARNESS).unwrap().phase,
        crate::harness_phase::ActivityPhase::Permission
    );
    r.mail(1);
    r.barrier();
    std::thread::sleep(Duration::from_millis(300));
    r.barrier();
    assert_eq!(r.writes().len(), 0, "{:?}", r.writes());
}

#[test]
fn gate_facts_read_the_stored_record_the_output_tap_and_the_pty() {
    use super::env::DeliveryEnv;
    let r = Rig::new(None);
    r.waiting();
    let f = r.env.gate_facts(HARNESS, 5);
    assert!(f.pty_live && f.runs_its_tool);
    assert_eq!((f.last_output_at, f.bracketed_paste), (None, false));
    r.handle.note_output(HARNESS, "\x1b[?2004h", 1234);
    let f = r.env.gate_facts(HARNESS, 5);
    assert_eq!((f.last_output_at, f.bracketed_paste), (Some(1234), true));
    // A new process drops the old one's output state.
    r.phase.spawned(HARNESS, ROOM, true);
    r.barrier();
    let f = r.env.gate_facts(HARNESS, 5);
    assert_eq!((f.last_output_at, f.bracketed_paste), (None, false));
    r.pty.live.store(false, Ordering::SeqCst);
    assert!(!r.env.gate_facts(HARNESS, 5).pty_live);
    assert!(!r.env.gate_facts("unknown", 5).pty_live);
}

#[test]
fn held_mail_is_published_and_snapshotted() {
    use super::env::DraftState;
    let r = Rig::new(None);
    r.waiting();
    r.handle.report_client_facts(
        HARNESS,
        DraftState::Held {
            reason: "typing".into(),
        },
        3,
        1,
    );
    r.mail(1);
    let end = Instant::now() + Duration::from_secs(5);
    while r.handle.held_snapshot().is_empty() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(r.handle.held_snapshot(), vec![HARNESS.to_owned()]);
    assert_eq!(r.writes().len(), 0);
    let held: Vec<_> = r
        .sink
        .0
        .lock()
        .iter()
        .filter(|(e, _)| e == "skein://mail-held")
        .cloned()
        .collect();
    assert_eq!(
        held,
        vec![(
            "skein://mail-held".to_owned(),
            r#"{"harnessId":"h2","held":true}"#.to_owned()
        )]
    );
}
