//! The delivery driver's small parts: the phase-listener translation, the
//! output tap, the worker's lifecycle.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::core::DeliveryEvent;
use super::env::DraftState;
use super::service::{DeliveryService, event_for_notice};
use super::shared::Shared;
use super::test_support::{Fake, fake};
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::{ActivityPhase, PhaseNotice, PhaseService};
use skein_proto::harness_kind::HarnessKind;

fn id(s: &str) -> String {
    s.to_owned()
}

#[test]
fn notices_translate_one_to_one() {
    let h = id("h");
    assert_eq!(
        event_for_notice(PhaseNotice::Spawned {
            harness_id: h.clone()
        }),
        DeliveryEvent::Spawned {
            harness_id: h.clone()
        }
    );
    assert_eq!(
        event_for_notice(PhaseNotice::Transition {
            harness_id: h.clone(),
            from: ActivityPhase::Running,
            to: ActivityPhase::Waiting,
        }),
        DeliveryEvent::Transition {
            harness_id: h.clone(),
            from: ActivityPhase::Running,
            to: ActivityPhase::Waiting,
        }
    );
    assert_eq!(
        event_for_notice(PhaseNotice::DeferralArmed {
            harness_id: h.clone()
        }),
        DeliveryEvent::DeferralArmed {
            harness_id: h.clone()
        }
    );
    assert_eq!(
        event_for_notice(PhaseNotice::Forgotten {
            harness_id: h.clone()
        }),
        DeliveryEvent::Forgotten { harness_id: h }
    );
}

#[test]
fn the_phase_service_reports_spawn_transitions_deferral_and_forget() {
    let svc = PhaseService::test_new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    svc.set_listener(Arc::new(move |n| sink.lock().push(n)));
    svc.spawned("h", "room-1", true);
    svc.claude_attached("h");
    svc.claude_event("h", &ClaudeEvent::AssistantTurn);
    svc.claude_event(
        "h",
        &ClaudeEvent::SubagentStart {
            agent_id: "a1".into(),
            agent_type: None,
            description: None,
            initial: false,
        },
    );
    svc.claude_event("h", &ClaudeEvent::AwaitingPrompt);
    svc.forget("h");
    let seen = seen.lock().clone();
    assert_eq!(
        seen.first(),
        Some(&PhaseNotice::Spawned {
            harness_id: id("h")
        })
    );
    assert!(seen.contains(&PhaseNotice::Transition {
        harness_id: id("h"),
        from: ActivityPhase::Spawning,
        to: ActivityPhase::Running,
    }));
    assert!(seen.contains(&PhaseNotice::DeferralArmed {
        harness_id: id("h")
    }));
    assert_eq!(
        seen.last(),
        Some(&PhaseNotice::Forgotten {
            harness_id: id("h")
        })
    );
    // The deferral kept the phase `running`: no waiting transition.
    assert!(!seen.iter().any(|n| matches!(
        n,
        PhaseNotice::Transition {
            to: ActivityPhase::Waiting,
            ..
        }
    )));
}

#[test]
fn an_unknown_harness_makes_no_notice() {
    let svc = PhaseService::test_new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    svc.set_listener(Arc::new(move |n| sink.lock().push(n)));
    svc.forget("never-spawned");
    svc.exited("never-spawned", Some(0));
    assert!(seen.lock().is_empty());
}

/// An env that counts events' effects: `unread` is the only thing the
/// mail layer reads on a `MailChanged`.
struct Counting {
    inner: Fake,
    unread_calls: Arc<AtomicU32>,
}

impl super::env::DeliveryEnv for Counting {
    fn gate_facts(&self, i: &str, n: i64) -> super::facts::GateFacts {
        self.inner.gate_facts(i, n)
    }
    fn spawn_token(&self, i: &str) -> Option<String> {
        self.inner.spawn_token(i)
    }
    fn user_input_seq(&self, i: &str) -> u64 {
        self.inner.user_input_seq(i)
    }
    fn draft(&self, i: &str) -> DraftState {
        self.inner.draft(i)
    }
    fn write_pty(&mut self, i: &str, b: &[u8]) -> bool {
        self.inner.write_pty(i, b)
    }
    fn note_prompt_submitted(&mut self, i: &str) {
        self.inner.note_prompt_submitted(i);
    }
    fn recover_unheard_silence(&mut self, i: &str) {
        self.inner.recover_unheard_silence(i);
    }
    fn unread(&self, i: &str) -> Option<super::env::UnreadMail> {
        self.unread_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.unread(i)
    }
    fn publish_held(&mut self, i: &str, h: bool) {
        self.inner.publish_held(i, h);
    }
    fn publish_mail_facts(&mut self, i: &str, f: super::env::MailFacts) {
        self.inner.publish_mail_facts(i, f);
    }
}

fn counting_service() -> (super::service::DeliveryHandle, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let env = Counting {
        inner: fake(HarnessKind::Claude),
        unread_calls: Arc::clone(&calls),
    };
    let handle = DeliveryService::start(
        Box::new(env),
        Arc::new(Shared::default()),
        Arc::new(|| 1_000_000),
    );
    (handle, calls)
}

#[test]
fn note_output_feeds_paste_mode_and_last_output() {
    let (h, _) = counting_service();
    let s = h.shared();
    assert_eq!(s.output_facts("h"), (None, false));
    h.note_output("h", "hello \x1b[?20", 10);
    assert_eq!(s.output_facts("h"), (Some(10), false));
    // A sequence split across chunks still turns the mode on.
    h.note_output("h", "04h", 20);
    assert_eq!(s.output_facts("h"), (Some(20), true));
    h.note_output("h", "\x1b[?2004l", 30);
    assert_eq!(s.output_facts("h"), (Some(30), false));
    // Other harnesses are independent; spawn and forget reset.
    assert_eq!(s.output_facts("other"), (None, false));
    h.note_output("h", "\x1b[?2004h", 40);
    h.notify(DeliveryEvent::Spawned {
        harness_id: id("h"),
    });
    assert_eq!(s.output_facts("h"), (None, false));
}

#[test]
fn client_reports_are_stored_and_dropped_on_spawn() {
    let (h, _) = counting_service();
    let s = h.shared();
    assert_eq!(s.client_facts("h"), (DraftState::Clean, 0));
    let held = DraftState::Held {
        reason: "typing".into(),
    };
    h.report_client_facts("h", held.clone(), 7, 1);
    assert_eq!(s.client_facts("h"), (held, 7));
    h.notify(DeliveryEvent::Forgotten {
        harness_id: id("h"),
    });
    assert_eq!(s.client_facts("h"), (DraftState::Clean, 0));
}

#[test]
fn events_reach_the_worker_and_a_blocking_call_is_ordered_after_them() {
    let (h, calls) = counting_service();
    h.notify(DeliveryEvent::MailChanged {
        harness_id: id("h"),
    });
    h.notify(DeliveryEvent::MailChanged {
        harness_id: id("h"),
    });
    // The round trip returns only after both events were handled; the fake
    // has no unread mail, so it is refused.
    assert!(h.deliver_now("h").is_err());
    assert!(calls.load(Ordering::SeqCst) >= 2);
}

#[test]
fn the_worker_stops_when_the_last_handle_drops() {
    let (h, calls) = counting_service();
    let second = h.clone();
    drop(h);
    // Still alive through the clone: it answers.
    assert!(second.deliver_now("h").is_err());
    assert!(Arc::strong_count(&calls) > 1);
    drop(second);
    // The thread ends and drops the env, which held the other count.
    let end = Instant::now() + Duration::from_secs(5);
    while Arc::strong_count(&calls) > 1 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(Arc::strong_count(&calls), 1);
}

fn held(reason: &str) -> DraftState {
    DraftState::Held {
        reason: reason.into(),
    }
}

#[test]
fn a_stale_client_report_is_dropped() {
    let (h, _) = counting_service();
    let s = h.shared();
    h.report_client_facts("h", held("typing"), 5, 10);
    // Arrives late: older number, must not replace the newer facts.
    h.report_client_facts("h", DraftState::Clean, 4, 9);
    assert_eq!(s.client_facts("h"), (held("typing"), 5));
    h.report_client_facts("h", DraftState::Clean, 6, 11);
    assert_eq!(s.client_facts("h"), (DraftState::Clean, 6));
    // A respawn forgets the number, so a fresh process starts over.
    h.notify(DeliveryEvent::Spawned {
        harness_id: id("h"),
    });
    h.report_client_facts("h", held("again"), 1, 1);
    assert_eq!(s.client_facts("h"), (held("again"), 1));
}

#[test]
fn only_a_first_report_or_held_to_clean_asks_for_a_check() {
    let s = Shared::default();
    assert!(s.set_client("h", held("x"), 1, 1), "first report");
    assert!(!s.set_client("h", held("x"), 2, 2), "seq only");
    assert!(s.set_client("h", DraftState::Clean, 2, 3), "held -> clean");
    assert!(!s.set_client("h", DraftState::Clean, 3, 4), "seq only");
    assert!(!s.set_client("h", held("x"), 3, 5), "clean -> held");
    assert!(!s.set_client("h", DraftState::Clean, 3, 5), "stale number");
}

#[test]
fn a_call_that_expired_before_the_worker_reached_it_is_skipped() {
    use super::env::{DeliveryEnv, UnreadMail};
    use super::gate::RefusalCode;
    use std::sync::mpsc::{Receiver, channel};
    struct Parked {
        inner: Fake,
        gate: Mutex<Receiver<()>>,
        writes: Arc<AtomicU32>,
    }
    impl DeliveryEnv for Parked {
        fn gate_facts(&self, i: &str, n: i64) -> super::facts::GateFacts {
            self.inner.gate_facts(i, n)
        }
        fn spawn_token(&self, i: &str) -> Option<String> {
            self.inner.spawn_token(i)
        }
        fn user_input_seq(&self, i: &str) -> u64 {
            self.inner.user_input_seq(i)
        }
        fn draft(&self, i: &str) -> DraftState {
            self.inner.draft(i)
        }
        fn write_pty(&mut self, i: &str, b: &[u8]) -> bool {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner.write_pty(i, b)
        }
        fn note_prompt_submitted(&mut self, i: &str) {
            self.inner.note_prompt_submitted(i);
        }
        fn recover_unheard_silence(&mut self, i: &str) {
            self.inner.recover_unheard_silence(i);
        }
        fn unread(&self, i: &str) -> Option<UnreadMail> {
            // The first event parks the worker until the test lets go.
            let _ = self.gate.lock().recv_timeout(Duration::from_secs(10));
            self.inner.unread(i)
        }
        fn publish_held(&mut self, i: &str, h: bool) {
            self.inner.publish_held(i, h);
        }
        fn publish_mail_facts(&mut self, i: &str, f: crate::prompt_delivery::env::MailFacts) {
            self.inner.publish_mail_facts(i, f);
        }
    }
    let (gate_tx, gate_rx) = channel::<()>();
    let writes = Arc::new(AtomicU32::new(0));
    let mut inner = fake(HarnessKind::Claude);
    inner.unread = Some(UnreadMail {
        count: 1,
        from: Vec::new(),
    });
    // A held draft: the automatic check holds, a release would paste.
    inner.draft = held("typing");
    let h = DeliveryService::start(
        Box::new(Parked {
            inner,
            gate: Mutex::new(gate_rx),
            writes: Arc::clone(&writes),
        }),
        Arc::new(Shared::default()),
        Arc::new(|| 1_000_000),
    );
    h.notify(DeliveryEvent::MailChanged {
        harness_id: id("h"),
    });
    // The worker is parked inside the event; this call times out waiting.
    let r = h.deliver_now_within("h", Duration::from_millis(50));
    assert_eq!(r.unwrap_err().code, RefusalCode::Busy);
    gate_tx.send(()).unwrap();
    drop(gate_tx);
    // The worker answers a later, live call (so it is past the expired one,
    // which was queued first). Only this one pastes.
    assert!(h.deliver_now("h").is_ok());
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}
