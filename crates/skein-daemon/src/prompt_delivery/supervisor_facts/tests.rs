//! #733 part B: the supervisor's `mail_held` invariant, fed through the real
//! pieces. The delivery core (on the shared fake env) decides what is held and
//! publishes the nudge time and refusal; the facts are built by
//! `mail_state_for`, as the daemon's closure does; the phase machine's
//! `supervise` judges them on a test clock.

use std::cell::Cell;

use skein_proto::harness_kind::HarnessKind;

use super::mail_state_for;
use crate::harness_phase::{
    ActivityPhase, HarnessFacts, PhaseMachine, SupervisorFacts, TransitionSource, Violation,
};
use crate::prompt_delivery::core::{DeliveryCore, DeliveryEvent};
use crate::prompt_delivery::env::{DraftState, MailFacts, UnreadMail};
use crate::prompt_delivery::test_support::{Fake, T0, fake};

const ID: &str = "h";

fn unread(count: u32) -> UnreadMail {
    UnreadMail {
        count,
        from: Vec::new(),
    }
}

/// A machine with `ID` waiting since `T0`, and a fake env with mail.
fn rig(count: u32) -> (PhaseMachine, DeliveryCore, Fake) {
    let mut m = PhaseMachine::new();
    m.spawned(ID, T0 - 10_000);
    m.set_waiting_from_adapter(ID, TransitionSource::L2c1ClaudeEndTurn, T0);
    let mut e = fake(HarnessKind::Claude);
    e.unread = Some(unread(count));
    (m, DeliveryCore::new(), e)
}

/// What the daemon's closure does per harness, with the unread reader
/// counted so a test can see whether it was called.
fn facts_of(m: &PhaseMachine, e: &Fake, reads: &Cell<u32>) -> SupervisorFacts {
    let mut facts = SupervisorFacts::default();
    for (id, phase) in m.phase_snapshot() {
        let mail = mail_state_for(
            &id,
            phase,
            || e.shared.mail_facts(&id),
            || {
                reads.set(reads.get() + 1);
                e.unread.as_ref().map(|u| u.count)
            },
        );
        facts.harnesses.insert(
            id,
            HarnessFacts {
                mail,
                ..HarnessFacts::default()
            },
        );
    }
    facts
}

fn supervise_at(m: &mut PhaseMachine, e: &Fake, at: i64) -> Vec<Violation> {
    m.supervise(&facts_of(m, e, &Cell::new(0)), at)
}

fn mail_violations(log: &[Violation]) -> usize {
    log.iter()
        .filter(|v| v.message.contains("code=mail_held violation"))
        .count()
}

#[test]
fn held_mail_is_reported_and_stops_once_delivered() {
    let (mut m, mut core, mut e) = rig(1);
    e.draft = DraftState::Held {
        reason: "typing".into(),
    };
    core.handle(
        &mut e,
        DeliveryEvent::MailChanged {
            harness_id: ID.into(),
        },
        T0,
    );
    assert!(e.writes.is_empty(), "the draft guard holds the nudge");
    let published = e.shared.mail_facts(ID);
    assert_eq!(published.last_nudge_at, None);
    assert!(published.last_refusal.is_some());

    // Inside the grace nothing is reported; past it, once.
    assert_eq!(mail_violations(&supervise_at(&mut m, &e, T0 + 1_000)), 0);
    assert_eq!(mail_violations(&supervise_at(&mut m, &e, T0 + 30_000)), 0);
    assert_eq!(mail_violations(&supervise_at(&mut m, &e, T0 + 90_000)), 1);

    // The draft is released and the worker nudges; the paste is stamped.
    e.draft = DraftState::Clean;
    core.handle(
        &mut e,
        DeliveryEvent::ClientFactsChanged {
            harness_id: ID.into(),
        },
        T0 + 95_000,
    );
    core.on_timer(&mut e, T0 + 96_000);
    assert!(!e.writes.is_empty(), "the nudge was written");
    assert_eq!(e.shared.mail_facts(ID).last_nudge_at, Some(T0 + 95_000));
    assert_eq!(e.shared.mail_facts(ID).last_refusal, None);
    let after = supervise_at(&mut m, &e, T0 + 100_000);
    assert_eq!(mail_violations(&after), 0);
    assert!(
        after
            .iter()
            .all(|v| !v.message.contains("code=mail_held violation")),
        "no fresh violation once the nudge is at or after phase_since"
    );
}

#[test]
fn a_waiting_harness_with_no_unread_mail_is_not_held() {
    let (mut m, _core, e) = rig(0);
    let reads = Cell::new(0);
    let facts = facts_of(&m, &e, &reads);
    assert_eq!(facts.harnesses[ID].mail.as_ref().map(|s| s.unread), Some(0));
    assert_eq!(reads.get(), 1);
    assert_eq!(mail_violations(&m.supervise(&facts, T0 + 1_000)), 0);
    assert_eq!(mail_violations(&m.supervise(&facts, T0 + 90_000)), 0);
}

#[test]
fn a_harness_that_is_not_waiting_reads_no_unread_count() {
    let (mut m, _core, e) = rig(3);
    m.set_running_from_adapter(ID, TransitionSource::L2c1ClaudeAssistant, false, T0 + 600);
    assert_ne!(m.phase_snapshot()[ID], ActivityPhase::Waiting);
    let reads = Cell::new(0);
    let facts = facts_of(&m, &e, &reads);
    assert_eq!(facts.harnesses[ID].mail, None);
    assert_eq!(reads.get(), 0, "unread is read only for a waiting harness");
}

#[test]
fn a_failed_unread_lookup_reads_as_no_facts() {
    let (m, _core, mut e) = rig(1);
    e.unread = None;
    let reads = Cell::new(0);
    assert_eq!(facts_of(&m, &e, &reads).harnesses[ID].mail, None);
    assert_eq!(reads.get(), 1);
}

#[test]
fn forgetting_a_harness_removes_its_published_entry() {
    let (_m, mut core, mut e) = rig(1);
    e.draft = DraftState::Held {
        reason: "typing".into(),
    };
    core.handle(
        &mut e,
        DeliveryEvent::MailChanged {
            harness_id: ID.into(),
        },
        T0,
    );
    assert_eq!(e.shared.mail_entries(), 1);
    core.handle(
        &mut e,
        DeliveryEvent::Forgotten {
            harness_id: ID.into(),
        },
        T0 + 1_000,
    );
    assert_eq!(e.shared.mail_entries(), 0);
    assert_eq!(e.shared.mail_facts(ID), MailFacts::default());
}
