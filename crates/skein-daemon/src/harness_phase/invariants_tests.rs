//! Port of `app/src/supervisor/invariants.test.ts` and
//! `invariants.background.test.ts` (#423): the six pure invariants. The TS
//! `it.each(rows)` is one test looping over the same rows in the same order;
//! a failing row names its code and case.

#![allow(clippy::unnecessary_wraps)]

use super::constants::DELEGATION_SETTLE_MS;
use super::invariants::{
    INVARIANTS, MailState, SupervisorCode, SupervisorSnapshot, TranscriptSample, Verdict,
};
use super::{ActivityPhase, AdapterEvent, TurnSignal, TurnSignalKind};

pub const NOW: i64 = 1_000_000;

pub fn snap() -> SupervisorSnapshot {
    SupervisorSnapshot {
        harness_id: "h1".to_owned(),
        phase: ActivityPhase::Running,
        phase_since: NOW - 100_000,
        last_output_at: Some(NOW - 100_000),
        authoritative: true,
        adapter_silent: false,
        live_attach: true,
        last_adapter_event: None,
        authority_lost_at: None,
        last_turn_signal: None,
        subagents_working: 0,
        background_working: 0,
        delegation_deferred_at: None,
        delegation_emptied_at: None,
        permission_at: None,
        last_submit_at: None,
        permission_agent_id: None,
        mail: None,
        transcript: None,
    }
}

fn verdict(code: SupervisorCode, s: &SupervisorSnapshot) -> Verdict {
    let inv = INVARIANTS
        .iter()
        .find(|i| i.code == code)
        .unwrap_or_else(|| panic!("no invariant {code:?}"));
    (inv.check)(s, NOW)
}

fn fires(code: SupervisorCode, s: &SupervisorSnapshot) -> bool {
    matches!(verdict(code, s), Verdict::Violated { .. })
}

fn sample(at: i64, size: i64) -> TranscriptSample {
    sample_m(at, size, 1.0)
}

fn sample_m(at: i64, size: i64, mtime_ms: f64) -> TranscriptSample {
    TranscriptSample { at, size, mtime_ms }
}

fn end(at: i64) -> Option<TurnSignal> {
    Some(TurnSignal {
        kind: TurnSignalKind::End,
        at,
    })
}

fn work(at: i64) -> Option<TurnSignal> {
    Some(TurnSignal {
        kind: TurnSignalKind::Work,
        at,
    })
}

fn event(at: i64, restores_authority: bool) -> Option<AdapterEvent> {
    Some(AdapterEvent {
        at,
        restores_authority,
    })
}

fn mail(unread: u32, last_refusal: Option<&str>, last_nudge_at: Option<i64>) -> Option<MailState> {
    Some(MailState {
        unread,
        last_refusal: last_refusal.map(str::to_owned),
        last_nudge_at,
    })
}

/// One row of the TS table: `snap(over)` then `fires(code, ..)`.
fn row(
    code: SupervisorCode,
    name: &str,
    expected: bool,
    over: impl FnOnce(&mut SupervisorSnapshot),
) {
    let mut s = snap();
    over(&mut s);
    assert_eq!(fires(code, &s), expected, "{}: {name}", code.as_str());
}

#[test]
fn invariants_table() {
    use SupervisorCode::{
        AdapterWithoutAuthority as Awa, DeferralWithoutWork as Dww, EndedTurnNotWaiting as Etnw,
        MailHeld as Mh, PermissionOrphaned as Po, TailSilentFileGrowing as Tsfg,
    };

    // adapter_without_authority
    row(Awa, "fires", true, |s| {
        s.authoritative = false;
        s.last_adapter_event = event(NOW - 1000, true);
        s.authority_lost_at = Some(NOW - 5000);
    });
    row(Awa, "fires with no authorityLostAt", true, |s| {
        s.authoritative = false;
        s.last_adapter_event = event(NOW - 1000, true);
    });
    row(Awa, "authoritative", false, |s| {
        s.last_adapter_event = event(NOW - 1000, true);
    });
    row(Awa, "exited", false, |s| {
        s.phase = ActivityPhase::Exited;
        s.authoritative = false;
        s.last_adapter_event = event(NOW - 1000, true);
    });
    row(Awa, "adapterSilent", false, |s| {
        s.authoritative = false;
        s.adapter_silent = true;
        s.last_adapter_event = event(NOW - 1000, true);
    });
    row(Awa, "session_end event", false, |s| {
        s.authoritative = false;
        s.last_adapter_event = event(NOW - 1000, false);
    });
    row(Awa, "event predates authorityLostAt", false, |s| {
        s.authoritative = false;
        s.last_adapter_event = event(NOW - 5000, true);
        s.authority_lost_at = Some(NOW - 1000);
    });
    row(Awa, "no live attach", false, |s| {
        s.authoritative = false;
        s.live_attach = false;
        s.last_adapter_event = event(NOW - 1000, true);
    });
    row(Awa, "no adapter event", false, |s| s.authoritative = false);

    // ended_turn_not_waiting
    row(Etnw, "submit after the end", false, |s| {
        s.last_submit_at = Some(NOW - 15_000);
        s.last_turn_signal = end(NOW - 20_000);
    });
    row(Etnw, "submit before the end", true, |s| {
        s.last_submit_at = Some(NOW - 25_000);
        s.last_turn_signal = end(NOW - 20_000);
    });
    row(Etnw, "running", true, |s| {
        s.last_turn_signal = end(NOW - 20_000);
    });
    row(Etnw, "idle, no output ever", true, |s| {
        s.phase = ActivityPhase::Idle;
        s.last_output_at = None;
        s.last_turn_signal = end(NOW - 20_000);
    });
    row(Etnw, "no turn signal", false, |_| {});
    row(Etnw, "work signal", false, |s| {
        s.last_turn_signal = work(NOW - 20_000);
    });
    row(Etnw, "subagents working", false, |s| {
        s.subagents_working = 1;
        s.last_turn_signal = end(NOW - 20_000);
    });
    row(Etnw, "deferred", false, |s| {
        s.delegation_deferred_at = Some(NOW - 1000);
        s.last_turn_signal = end(NOW - 20_000);
    });
    row(Etnw, "recent output", false, |s| {
        s.last_output_at = Some(NOW - 2000);
        s.last_turn_signal = end(NOW - 20_000);
    });
    for phase in [
        ActivityPhase::Permission,
        ActivityPhase::Exited,
        ActivityPhase::Waiting,
        ActivityPhase::Spawning,
    ] {
        row(Etnw, &format!("phase {}", phase.as_str()), false, |s| {
            s.phase = phase;
            s.last_turn_signal = end(NOW - 20_000);
        });
    }

    // mail_held
    row(Mh, "fires", true, |s| {
        s.phase = ActivityPhase::Waiting;
        s.mail = mail(2, Some("busy"), None);
    });
    row(Mh, "nudge before waiting began", true, |s| {
        s.phase = ActivityPhase::Waiting;
        s.mail = mail(1, None, Some(NOW - 200_000));
    });
    row(Mh, "no mail", false, |s| s.phase = ActivityPhase::Waiting);
    row(Mh, "nothing unread", false, |s| {
        s.phase = ActivityPhase::Waiting;
        s.mail = mail(0, None, None);
    });
    row(Mh, "not waiting", false, |s| {
        s.phase = ActivityPhase::Running;
        s.mail = mail(1, None, None);
    });
    row(Mh, "nudged since waiting", false, |s| {
        s.phase = ActivityPhase::Waiting;
        s.mail = mail(1, None, Some(NOW - 50_000));
    });

    // tail_silent_file_growing
    row(Tsfg, "size grew", true, |s| {
        s.transcript = Some(vec![sample(NOW - 70_000, 100), sample(NOW, 200)]);
    });
    row(Tsfg, "mtime grew only", true, |s| {
        s.transcript = Some(vec![
            sample_m(NOW - 70_000, 100, 1.0),
            sample_m(NOW, 100, 2.0),
        ]);
    });
    row(Tsfg, "silent since last adapter event", true, |s| {
        s.last_adapter_event = event(NOW - 70_000, true);
        s.transcript = Some(vec![
            sample(NOW - 100_000, 10),
            sample(NOW - 70_000, 100),
            sample(NOW, 200),
        ]);
    });
    row(Tsfg, "growth predates adapter event", false, |s| {
        s.last_adapter_event = event(NOW - 30_000, true);
        s.transcript = Some(vec![
            sample(NOW - 100_000, 10),
            sample(NOW - 70_000, 100),
            sample(NOW, 100),
        ]);
    });
    row(Tsfg, "no samples", false, |_| {});
    row(Tsfg, "one sample", false, |s| {
        s.transcript = Some(vec![sample(NOW, 100)]);
    });
    row(Tsfg, "no live attach", false, |s| {
        s.live_attach = false;
        s.transcript = Some(vec![sample(NOW - 70_000, 100), sample(NOW, 200)]);
    });
    row(Tsfg, "unchanged file", false, |s| {
        s.transcript = Some(vec![sample(NOW - 70_000, 100), sample(NOW, 100)]);
    });
    row(Tsfg, "window too short", false, |s| {
        s.transcript = Some(vec![sample(NOW - 30_000, 100), sample(NOW, 200)]);
    });

    // deferral_without_work
    row(Dww, "fires", true, |s| {
        s.delegation_deferred_at = Some(NOW - 20_000);
    });
    row(Dww, "emptied past settle", true, |s| {
        s.delegation_deferred_at = Some(NOW - 200_000);
        s.delegation_emptied_at = Some(NOW - DELEGATION_SETTLE_MS);
    });
    row(Dww, "not deferred", false, |_| {});
    row(Dww, "subagents working", false, |s| {
        s.delegation_deferred_at = Some(NOW - 20_000);
        s.subagents_working = 2;
    });
    row(Dww, "permission", false, |s| {
        s.delegation_deferred_at = Some(NOW - 20_000);
        s.phase = ActivityPhase::Permission;
    });
    row(Dww, "exited", false, |s| {
        s.delegation_deferred_at = Some(NOW - 20_000);
        s.phase = ActivityPhase::Exited;
    });
    row(Dww, "emptied recently", false, |s| {
        s.delegation_deferred_at = Some(NOW - 20_000);
        s.delegation_emptied_at = Some(NOW - 1000);
    });

    // permission_orphaned
    row(Po, "turn moved on", true, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 30_000);
        s.last_turn_signal = work(NOW - 20_000);
    });
    row(Po, "ceiling", true, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 15 * 60_000);
    });
    row(Po, "ceiling with subagent dialog", true, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 16 * 60_000);
        s.permission_agent_id = Some("a1".to_owned());
    });
    row(Po, "not in permission", false, |s| {
        s.permission_at = Some(NOW - 16 * 60_000);
    });
    row(Po, "no permissionAt", false, |s| {
        s.phase = ActivityPhase::Permission;
    });
    row(Po, "subagent dialog, turn moved on", false, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 30_000);
        s.permission_agent_id = Some("a1".to_owned());
        s.last_turn_signal = work(NOW - 20_000);
    });
    row(Po, "turn moved on but subagents working", false, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 30_000);
        s.subagents_working = 1;
        s.last_turn_signal = work(NOW - 20_000);
    });
    row(Po, "subagents working still hit the ceiling", true, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 16 * 60_000);
        s.subagents_working = 1;
    });
    row(Po, "turn signal older than dialog", false, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 30_000);
        s.last_turn_signal = work(NOW - 40_000);
    });
    row(Po, "turn signal too fresh", false, |s| {
        s.phase = ActivityPhase::Permission;
        s.permission_at = Some(NOW - 30_000);
        s.last_turn_signal = work(NOW - 5000);
    });
}

#[test]
fn evidence_names_the_facts() {
    let mut s = snap();
    s.last_turn_signal = end(NOW - 20_000);
    let Verdict::Violated { evidence } = verdict(SupervisorCode::EndedTurnNotWaiting, &s) else {
        panic!("expected a violation");
    };
    assert!(evidence.contains("phase=running"), "{evidence}");
    assert!(evidence.contains("lastTurn=end@20000ms"), "{evidence}");
}

#[test]
fn permission_evidence_says_which_arm() {
    let mut s = snap();
    s.phase = ActivityPhase::Permission;
    s.permission_at = Some(NOW - 16 * 60_000);
    let Verdict::Violated { evidence } = verdict(SupervisorCode::PermissionOrphaned, &s) else {
        panic!("expected a violation");
    };
    assert!(evidence.contains("arm=ceiling"), "{evidence}");
}

#[test]
fn declares_each_code_once() {
    let codes: Vec<_> = INVARIANTS.iter().map(|i| i.code).collect();
    let unique: std::collections::HashSet<_> = codes.iter().collect();
    assert_eq!(unique.len(), codes.len());
}

// invariants.background.test.ts: "invariants with background tasks working"

#[test]
fn background_ended_turn_not_waiting_stays_quiet() {
    let mut s = snap();
    s.background_working = 1;
    s.last_turn_signal = end(NOW - 20_000);
    assert!(!fires(SupervisorCode::EndedTurnNotWaiting, &s));
}

#[test]
fn background_deferral_without_work_stays_quiet() {
    let mut s = snap();
    s.delegation_deferred_at = Some(NOW - 20_000);
    s.background_working = 1;
    assert!(!fires(SupervisorCode::DeferralWithoutWork, &s));
}
