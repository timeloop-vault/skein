use crate::harness_events_opencode::OpencodeEvent as E;
use crate::harness_phase::ActivityPhase as P;
use crate::harness_phase::TransitionSource as S;
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::translate::opencode::OpencodeTranslator;
use crate::harness_phase::translate::pending_prompts::{PendingPhase, PendingPrompts};

const ID: &str = "h1";

fn rig() -> (Rig, OpencodeTranslator) {
    let mut rig = Rig::new();
    rig.spawned(ID);
    let mut t = OpencodeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    (rig, t)
}

fn phase(rig: &Rig) -> P {
    rig.m.get(ID).expect("harness known").phase
}

fn send(rig: &mut Rig, t: &mut OpencodeTranslator, e: &E) {
    t.apply(&mut rig.m, ID, e, rig.now);
}

fn req(id: &str) -> (String, String) {
    (id.to_owned(), "s1".to_owned())
}

#[test]
fn busy_then_idle() {
    let (mut rig, mut t) = rig();
    send(&mut rig, &mut t, &E::SessionBusy);
    assert_eq!(phase(&rig), P::Running);
    send(&mut rig, &mut t, &E::SessionIdle);
    assert_eq!(phase(&rig), P::Waiting);
}

#[test]
fn permission_asked_then_replied() {
    let (mut rig, mut t) = rig();
    send(&mut rig, &mut t, &E::SessionBusy);
    let (request_id, session_id) = req("p1");
    send(
        &mut rig,
        &mut t,
        &E::PermissionAsked {
            request_id: request_id.clone(),
            session_id: session_id.clone(),
        },
    );
    assert_eq!(phase(&rig), P::Permission);
    // "still working" must not clear the dialog.
    send(&mut rig, &mut t, &E::MessageDelta);
    assert_eq!(phase(&rig), P::Permission);
    send(
        &mut rig,
        &mut t,
        &E::PermissionReplied {
            request_id,
            session_id,
        },
    );
    assert_eq!(phase(&rig), P::Running);
}

#[test]
fn question_is_outranked_by_permission_and_idle_forgets_both() {
    let (mut rig, mut t) = rig();
    send(&mut rig, &mut t, &E::SessionBusy);
    let (q, s) = req("q1");
    send(
        &mut rig,
        &mut t,
        &E::QuestionAsked {
            request_id: q.clone(),
            session_id: s.clone(),
        },
    );
    assert_eq!(phase(&rig), P::Waiting);
    assert!(rig.m.get(ID).expect("known").question_open);
    let (p, s2) = req("p1");
    send(
        &mut rig,
        &mut t,
        &E::PermissionAsked {
            request_id: p,
            session_id: s2,
        },
    );
    send(
        &mut rig,
        &mut t,
        &E::QuestionResolved {
            request_id: q,
            session_id: s,
        },
    );
    assert_eq!(phase(&rig), P::Permission);
    send(&mut rig, &mut t, &E::SessionIdle);
    assert_eq!(phase(&rig), P::Waiting);
    assert!(!rig.m.get(ID).expect("known").question_open);
}

// ---- harnessEventsOpencode.question.test.ts (#672) ----------------------

fn question_rig() -> (Rig, OpencodeTranslator) {
    let mut rig = Rig::new();
    rig.spawned(ID);
    rig.m.set_injected(ID, true);
    let mut t = OpencodeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    (rig, t)
}

fn question_open(rig: &Rig) -> bool {
    rig.m.get(ID).expect("known").question_open
}

fn asked() -> E {
    E::QuestionAsked {
        request_id: "q1".to_owned(),
        session_id: "s".to_owned(),
    }
}

// NOT PORTED: the `canSendPrompt` gate assertions (`gate(id)` toEqual ok /
// REFUSED) in the first two question cases: the gate is webview code
// (`harnessInput.ts`) over HARNESS_KINDS and the registered paste seam; the
// Rust side owns only the `question_open` mirror it reads.
#[test]
fn question_asked_sets_question_open_and_the_gate_refuses() {
    let (mut rig, mut t) = question_rig();
    assert!(!question_open(&rig));
    send(&mut rig, &mut t, &E::SessionIdle);
    send(&mut rig, &mut t, &asked());
    assert_eq!(phase(&rig), P::Waiting);
    assert!(question_open(&rig));
}

#[test]
fn question_resolved_clears_it_then_session_idle_reaches_a_sendable_waiting() {
    let (mut rig, mut t) = question_rig();
    send(&mut rig, &mut t, &asked());
    send(
        &mut rig,
        &mut t,
        &E::QuestionResolved {
            request_id: "q1".to_owned(),
            session_id: "s".to_owned(),
        },
    );
    assert!(!question_open(&rig));
    assert_eq!(phase(&rig), P::Running);
    send(&mut rig, &mut t, &E::SessionIdle);
    assert_eq!(phase(&rig), P::Waiting);
}

#[test]
fn does_not_notify_subscribers_when_unchanged() {
    let (mut rig, mut t) = question_rig();
    send(&mut rig, &mut t, &asked());
    // TS subscribes here; `drain_changed` is the Rust form of "emits since".
    rig.m.drain_changed();
    rig.m.set_question_open(ID, true);
    assert_eq!(rig.m.drain_changed(), Vec::<String>::new());
}

fn clears_question(event: &E) {
    let (mut rig, mut t) = question_rig();
    send(&mut rig, &mut t, &asked());
    assert!(question_open(&rig));
    send(&mut rig, &mut t, event);
    assert!(!question_open(&rig));
}

#[test]
fn session_idle_clears_it() {
    clears_question(&E::SessionIdle);
}

#[test]
fn session_end_clears_it() {
    clears_question(&E::SessionEnd);
}

#[test]
fn connected_clears_it() {
    clears_question(&E::Connected);
}

#[test]
fn session_baseline_idle_clears_it() {
    clears_question(&E::SessionBaselineIdle);
}

// ---- harnessEventsOpencode.baseline.test.ts (#175) ----------------------

// NOT PORTED: harnessEventsOpencode.v2attach.test.ts ("passes
// sidecarGeneration for v2", "omits the key for v1"): both assert only the
// mocked `invoke("opencode_events_attach", args)` payload, which is webview
// plumbing with no counterpart in the translator.

fn seen(rig: &mut Rig) -> Vec<(P, P, S)> {
    rig.m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == ID)
        .map(|t| (t.from, t.to, t.source))
        .collect()
}

#[test]
fn session_baseline_idle_moves_running_to_waiting_under_the_baseline_source() {
    let (mut rig, mut t) = rig();
    rig.m
        .set_running_from_adapter(ID, S::L2c2OpencodeBusy, false, rig.now);
    rig.m.drain_transitions();
    send(&mut rig, &mut t, &E::SessionBaselineIdle);
    assert_eq!(phase(&rig), P::Waiting);
    let got: Vec<_> = seen(&mut rig)
        .into_iter()
        .map(|(_, to, s)| (to, s))
        .collect();
    assert_eq!(got, vec![(P::Waiting, S::L2c2OpencodeBaseline)]);
}

// NOT PORTED: the final `isNotifiable(classifyTransition(...))` assertion of
// the next case: notification classification is webview code
// (`harnessNotifyLogic.ts`); the quiet baseline source it keys on is
// asserted via the transition below.
#[test]
fn session_baseline_idle_moves_permission_to_waiting_quietly() {
    let (mut rig, mut t) = rig();
    rig.m
        .set_permission_from_adapter(ID, S::L2c2OpencodePermission, None, None, None, rig.now);
    rig.m.drain_transitions();
    send(&mut rig, &mut t, &E::Connected);
    send(&mut rig, &mut t, &E::SessionBaselineIdle);
    assert_eq!(phase(&rig), P::Waiting);
    assert_eq!(
        seen(&mut rig),
        vec![(P::Permission, P::Waiting, S::L2c2OpencodeBaseline)]
    );
}

#[test]
fn session_baseline_idle_does_not_leave_exited() {
    let (mut rig, mut t) = rig();
    rig.m.exited(ID, Some(0), rig.now);
    send(&mut rig, &mut t, &E::SessionBaselineIdle);
    assert_eq!(phase(&rig), P::Exited);
}

#[test]
fn a_real_session_idle_still_reports_its_own_source() {
    let (mut rig, mut t) = rig();
    rig.m
        .set_running_from_adapter(ID, S::L2c2OpencodeBusy, false, rig.now);
    rig.m.drain_transitions();
    send(&mut rig, &mut t, &E::SessionIdle);
    let got: Vec<_> = seen(&mut rig).into_iter().map(|(_, _, s)| s).collect();
    assert_eq!(got, vec![S::L2c2OpencodeIdle]);
}

// ---- pendingPrompts.test.ts (#86, #672) ---------------------------------

#[test]
fn a_permission_always_yields_permission() {
    let mut p = PendingPrompts::default();
    assert_eq!(p.permission_asked("req_1"), PendingPhase::Permission);
}

#[test]
fn draining_the_only_permission_with_nothing_else_pending_yields_running() {
    let mut p = PendingPrompts::default();
    p.permission_asked("req_1");
    assert_eq!(p.permission_replied("req_1"), PendingPhase::Running);
}

#[test]
fn draining_one_of_several_pending_permissions_stays_on_permission() {
    let mut p = PendingPrompts::default();
    p.permission_asked("req_1");
    p.permission_asked("req_2");
    assert_eq!(p.permission_replied("req_1"), PendingPhase::Permission);
    assert_eq!(p.permission_replied("req_2"), PendingPhase::Running);
}

#[test]
fn a_question_with_nothing_else_pending_yields_waiting() {
    let mut p = PendingPrompts::default();
    assert_eq!(p.question_asked("q_1"), PendingPhase::Waiting);
}

#[test]
fn a_question_is_outranked_by_a_pending_permission() {
    let mut p = PendingPrompts::default();
    p.permission_asked("req_1");
    assert_eq!(p.question_asked("q_1"), PendingPhase::Permission);
    // Resolving the question alone does not clear the still-pending permission.
    assert_eq!(p.question_resolved("q_1"), PendingPhase::Permission);
    // Only draining the permission itself reaches running.
    assert_eq!(p.permission_replied("req_1"), PendingPhase::Running);
}

#[test]
fn resolving_a_question_with_no_permission_pending_yields_running() {
    let mut p = PendingPrompts::default();
    p.question_asked("q_1");
    assert_eq!(p.question_resolved("q_1"), PendingPhase::Running);
}

#[test]
fn a_permission_that_outlasts_a_question_still_reads_permission_after_the_question_resolves() {
    let mut p = PendingPrompts::default();
    p.question_asked("q_1");
    p.permission_asked("req_1");
    assert_eq!(p.question_resolved("q_1"), PendingPhase::Permission);
}

#[test]
fn forget_drops_every_pending_id_for_that_harness() {
    let mut p = PendingPrompts::default();
    p.permission_asked("req_1");
    p.question_asked("q_1");
    p.forget();
    // Replying/resolving ids that no longer exist is a no-op, and with
    // nothing tracked the harness reads as drained (running).
    assert_eq!(p.permission_replied("req_1"), PendingPhase::Running);
    assert_eq!(p.question_resolved("q_1"), PendingPhase::Running);
}

#[test]
fn tracks_harnesses_independently() {
    // The TS keys one module map by harness id; here each harness owns its
    // own instance, so independence is two instances.
    let mut a = PendingPrompts::default();
    let mut b = PendingPrompts::default();
    a.permission_asked("req_1");
    assert_eq!(b.question_asked("q_1"), PendingPhase::Waiting);
    assert_eq!(a.permission_replied("req_1"), PendingPhase::Running);
}

#[test]
fn has_question_is_true_only_while_a_question_is_outstanding() {
    let mut p = PendingPrompts::default();
    assert!(!p.has_question());
    p.permission_asked("req_1");
    assert!(!p.has_question());
    p.question_asked("q_1");
    p.question_asked("q_2");
    assert!(p.has_question());
    p.question_resolved("q_1");
    assert!(p.has_question());
    p.question_resolved("q_2");
    assert!(!p.has_question());
    p.question_asked("q_3");
    p.forget();
    assert!(!p.has_question());
}
