//! `DeliveryHandle::check` / the core's `Check` event: the gate's answer
//! without sending anything.

#![allow(clippy::assert_is_empty)]

use std::sync::Arc;

use super::core::{DeliveryCore, DeliveryEvent, Reply};
use super::gate::{GateMode, RefusalCode};
use super::service::DeliveryService;
use super::shared::Shared;
use super::test_support::{Fake, T0, fake};
use crate::harness_phase::ActivityPhase;
use skein_proto::harness_kind::HarnessKind;

type Prep = fn(&mut Fake);

fn check(mode: GateMode, body: Option<&str>) -> DeliveryEvent {
    DeliveryEvent::Check {
        harness_id: "h".into(),
        body: body.map(str::to_owned),
        mode,
    }
}

fn manual(body: &str) -> DeliveryEvent {
    DeliveryEvent::Manual {
        harness_id: "h".into(),
        body: body.into(),
    }
}

#[test]
fn a_send_check_on_a_sendable_harness_is_ok_and_touches_nothing() {
    let mut e = fake(HarnessKind::Claude);
    let mut c = DeliveryCore::new();
    let r = c.handle(&mut e, check(GateMode::Send, Some("hi")), T0);
    assert_eq!(r, Reply::Check(Ok(())));
    assert!(e.writes.is_empty());
    assert_eq!(e.submitted, 0);
    assert_eq!(c.next_deadline(), None);
    // Nothing is in flight, so a real send still goes through.
    assert_eq!(c.handle(&mut e, manual("hi"), T0), Reply::Manual(Ok(())));
}

#[test]
fn a_none_body_is_an_empty_single_line_body() {
    let mut e = fake(HarnessKind::Claude);
    let mut c = DeliveryCore::new();
    assert_eq!(
        c.handle(&mut e, check(GateMode::Send, None), T0),
        Reply::Check(Ok(()))
    );
}

#[test]
fn send_refusals_match_send_manual_for_the_same_state() {
    let states: [(&str, Prep); 2] = [
        ("permission", |e| {
            e.set_phase(|p| p.phase = ActivityPhase::Permission);
        }),
        ("not at stopping point", |e| {
            e.set_phase(|p| p.phase = ActivityPhase::Running);
        }),
    ];
    for (name, prep) in states {
        let mut e = fake(HarnessKind::Claude);
        prep(&mut e);
        let mut c = DeliveryCore::new();
        let Reply::Check(checked) = c.handle(&mut e, check(GateMode::Send, Some("x")), T0) else {
            panic!("wrong reply");
        };
        let Reply::Manual(sent) = c.handle(&mut e, manual("x"), T0) else {
            panic!("wrong reply");
        };
        assert!(checked.is_err(), "{name}");
        assert_eq!(checked, sent, "{name}");
    }
}

#[test]
fn a_send_check_reports_busy_while_a_prompt_is_in_flight() {
    let mut e = fake(HarnessKind::Claude);
    let mut c = DeliveryCore::new();
    assert_eq!(c.handle(&mut e, manual("hi"), T0), Reply::Manual(Ok(())));
    let writes = e.writes.len();
    let Reply::Check(Err(r)) = c.handle(&mut e, check(GateMode::Send, Some("x")), T0 + 1) else {
        panic!("expected a refusal");
    };
    assert_eq!(r.code, RefusalCode::Busy);
    assert_eq!(e.writes.len(), writes);
}

#[test]
fn insert_is_ok_on_a_running_harness_where_send_is_refused() {
    let mut e = fake(HarnessKind::Claude);
    e.set_phase(|p| p.phase = ActivityPhase::Running);
    let mut c = DeliveryCore::new();
    assert_eq!(
        c.handle(&mut e, check(GateMode::Insert, None), T0),
        Reply::Check(Ok(()))
    );
    let Reply::Check(Err(r)) = c.handle(&mut e, check(GateMode::Send, Some("x")), T0) else {
        panic!("expected a refusal");
    };
    assert_eq!(r.code, RefusalCode::NotAtStoppingPoint);
    assert!(e.writes.is_empty());
}

#[test]
fn the_handle_answers_a_check_through_the_worker() {
    let mut e = fake(HarnessKind::Claude);
    e.set_phase(|p| p.phase = ActivityPhase::Running);
    let h = DeliveryService::start(Box::new(e), Arc::new(Shared::default()), Arc::new(|| T0));
    assert_eq!(h.check("h", None, GateMode::Insert), Ok(()));
    assert_eq!(
        h.check("h", Some("x"), GateMode::Send).unwrap_err().code,
        RefusalCode::NotAtStoppingPoint
    );
}
