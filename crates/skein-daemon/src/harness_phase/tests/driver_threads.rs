//! Registry changes reaching the stream without a tick, the supervisor pass
//! (#761), and the threads ending with the service.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use serde_json::Value;
use skein_harness::claude::background::{BackgroundKind, OutcomeStatus};

use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::SupervisorFacts;
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::stream::HARNESS_PHASE_EVENT;
use crate::harness_phase::test_support::RecordingSink;
use crate::harness_phase::types::{ActivityPhase, TurnSignal, TurnSignalKind};

const T0: i64 = 1_000_000;

struct Rig {
    svc: Arc<PhaseService>,
    sink: Arc<RecordingSink>,
    now: Arc<AtomicI64>,
}

impl Rig {
    fn new() -> Self {
        let now = Arc::new(AtomicI64::new(T0));
        let c = Arc::clone(&now);
        let (svc, sink) =
            PhaseService::test_with_clock_and_sink(Arc::new(move || c.load(Ordering::SeqCst)));
        Self { svc, sink, now }
    }

    /// Every update emitted for `h` so far, as JSON.
    fn updates(&self) -> Vec<Value> {
        self.svc.flush_outputs();
        self.sink
            .events()
            .into_iter()
            .inspect(|(e, _)| assert_eq!(e, HARNESS_PHASE_EVENT))
            .map(|(_, p)| serde_json::from_str::<Value>(&p).unwrap())
            .filter(|v| v["harnessId"] == "h")
            .collect()
    }

    /// The update the last call emitted, which must exist: `(subagents,
    /// background tasks)`.
    fn after(&self, before: usize) -> (usize, usize) {
        let all = self.updates();
        assert_eq!(all.len(), before + 1, "exactly one update from the call");
        let last = all.last().unwrap();
        (
            last["subagents"].as_array().unwrap().len(),
            last["backgroundTasks"].as_array().unwrap().len(),
        )
    }
}

/// A registry change made by an event is published in the same call, with no
/// tick and no driver.
#[test]
fn a_registry_change_emits_an_update_without_a_tick() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.claude_attached("h");
    let mut n = r.updates().len();
    let mut step = |event: ClaudeEvent| {
        r.svc.claude_event("h", &event);
        let got = r.after(n);
        n += 1;
        got
    };
    assert_eq!(
        step(ClaudeEvent::SubagentStart {
            agent_id: "a1".into(),
            agent_type: None,
            description: None,
            initial: false,
        }),
        (1, 0)
    );
    assert_eq!(
        step(ClaudeEvent::SubagentEnd {
            agent_id: "a1".into(),
            agent_type: None,
            description: None,
        }),
        (0, 0)
    );
    assert_eq!(
        step(ClaudeEvent::BackgroundStart {
            task_id: "t1".into(),
            tool_use_id: "u1".into(),
            task_kind: BackgroundKind::Bash,
            description: None,
            command: None,
            timeout_ms: None,
            persistent: false,
            auto_backgrounded: false,
            agent_id: None,
            initial: false,
        }),
        (0, 1)
    );
    assert_eq!(
        step(ClaudeEvent::BackgroundEnd {
            task_id: "t1".into(),
            task_kind: BackgroundKind::Bash,
            agent_id: None,
            status: OutcomeStatus::Completed,
            exit_code: None,
        }),
        (0, 0)
    );
}

/// The supervisor pass runs at its interval, and a repair it makes is
/// published. A fake clock puts the harness past the grace.
#[test]
fn the_supervisor_pass_runs_on_its_interval_and_publishes_its_repair() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.record_output("h", "x");
    r.svc.with(|s, _| {
        s.machine.store.get_mut("h").unwrap().last_turn_signal = Some(TurnSignal {
            kind: TurnSignalKind::End,
            at: T0 + 1,
        });
    });
    r.now.store(T0 + 20_000, Ordering::SeqCst);
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let started = Instant::now();
    let _supervisor = r
        .svc
        .start_supervisor_every(
            Duration::from_millis(20),
            Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                SupervisorFacts::default()
            }),
        )
        .expect("thread");
    // Two facts calls: the first pass has finished, so first sight was at
    // T0 + 20 s.
    while calls.load(Ordering::SeqCst) < 2 {
        assert!(started.elapsed() < Duration::from_secs(5), "no pass ran");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(r.svc.get("h").unwrap().phase, ActivityPhase::Running);
    r.now.store(T0 + 30_000, Ordering::SeqCst);
    while r.svc.get("h").unwrap().phase != ActivityPhase::Waiting {
        assert!(started.elapsed() < Duration::from_secs(5), "no repair");
        std::thread::sleep(Duration::from_millis(5));
    }
    let published = r.updates().iter().any(|u| {
        u["transitions"].as_array().is_some_and(|ts| {
            ts.iter()
                .any(|t| t["source"] == "supervisor-recovered" && t["to"] == "waiting")
        })
    });
    assert!(published, "the repair's transition was not emitted");
    // The interval is the injected one, not a faster loop: ~20 ms apart.
    let elapsed = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    let n = u32::try_from(calls.load(Ordering::SeqCst)).unwrap_or(u32::MAX);
    assert!(n <= elapsed / 10 + 2, "{n} passes in {elapsed} ms");
}

/// Both threads end when the last `Arc` goes, at once: the supervisor's own
/// interval is the production 5 s, so only the close can end it in time.
#[test]
fn both_threads_end_when_the_service_is_dropped() {
    let svc = PhaseService::test_new();
    svc.spawned("h", "room-1", false);
    let driver = svc.start_deadline_driver().expect("thread");
    let supervisor = svc
        .start_supervisor(Arc::new(SupervisorFacts::default))
        .expect("thread");
    // Drop only once the driver sleeps, so the close has to wake it.
    let end = Instant::now() + Duration::from_secs(5);
    while !svc.driver_is_asleep() {
        assert!(Instant::now() < end, "driver asleep");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(svc);
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        driver.join().ok();
        supervisor.join().ok();
        tx.send(()).ok();
    });
    rx.recv_timeout(Duration::from_secs(2))
        .expect("the threads ended with the service");
}
