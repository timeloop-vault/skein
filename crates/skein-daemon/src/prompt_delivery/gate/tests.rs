use super::*;

fn phase(over: impl FnOnce(&mut PhaseFacts)) -> PhaseFacts {
    let mut p = PhaseFacts {
        phase: ActivityPhase::Waiting,
        question_open: false,
        delegation_deferred_at: None,
        authoritative: true,
        adapter_heard: true,
        adapter_silent: false,
        launch_signal_at: None,
        injected: true,
    };
    over(&mut p);
    p
}

fn facts(over: impl FnOnce(&mut GateFacts)) -> GateFacts {
    let mut f = GateFacts {
        kind: HarnessKind::Claude,
        pty_live: true,
        runs_its_tool: true,
        phase: Some(phase(|_| {})),
        last_output_at: None,
        bracketed_paste: false,
        now_ms: 1_000_000,
    };
    over(&mut f);
    f
}

fn with_phase(over: impl FnOnce(&mut PhaseFacts)) -> GateFacts {
    facts(|f| f.phase = Some(phase(over)))
}

fn code(f: &GateFacts, body: &str) -> RefusalCode {
    can_send(f, body).expect_err("should refuse").code
}

fn reason(f: &GateFacts, body: &str) -> String {
    can_send(f, body).expect_err("should refuse").reason
}

#[test]
fn allows_a_single_line_prompt_to_a_waiting_verified_injected_harness() {
    assert_eq!(can_send(&facts(|_| {}), "nudge"), Ok(()));
}

#[test]
fn refuses_a_kind_with_no_terminal() {
    let f = facts(|f| f.kind = HarnessKind::Files);
    assert_eq!(code(&f, "x"), RefusalCode::NoTerminal);
    assert!(reason(&f, "x").contains("no terminal"));
}

#[test]
fn refuses_when_no_live_pty_or_no_phase_record() {
    let want = "this harness's terminal isn't ready yet";
    let f = facts(|f| f.pty_live = false);
    assert_eq!(code(&f, "x"), RefusalCode::NotReady);
    assert_eq!(reason(&f, "x"), want);
    let f = facts(|f| f.phase = None);
    assert_eq!(code(&f, "x"), RefusalCode::NotReady);
    assert_eq!(reason(&f, "x"), want);
}

#[test]
fn a_post_exit_shell_gets_no_prompt() {
    let f = facts(|f| f.runs_its_tool = false);
    assert_eq!(code(&f, "x"), RefusalCode::RunsShell);
    assert_eq!(reason(&f, "x"), "the pane is running a shell, not its tool");
}

#[test]
fn the_shell_check_comes_after_readiness_and_before_phase() {
    let f = facts(|f| {
        f.runs_its_tool = false;
        f.pty_live = false;
    });
    assert_eq!(code(&f, "x"), RefusalCode::NotReady);
    let mut f = with_phase(|p| p.phase = ActivityPhase::Permission);
    f.runs_its_tool = false;
    assert_eq!(code(&f, "x"), RefusalCode::RunsShell);
}

#[test]
fn permission_has_its_own_reason() {
    let f = with_phase(|p| p.phase = ActivityPhase::Permission);
    assert_eq!(code(&f, "x"), RefusalCode::Permission);
    assert!(reason(&f, "x").contains("permission dialog"));
}

#[test]
fn permission_wins_over_an_armed_deferral() {
    let f = with_phase(|p| {
        p.phase = ActivityPhase::Permission;
        p.delegation_deferred_at = Some(5);
    });
    assert_eq!(code(&f, "x"), RefusalCode::Permission);
}

#[test]
fn an_open_question_refuses_a_waiting_harness() {
    let f = with_phase(|p| p.question_open = true);
    assert_eq!(code(&f, "x"), RefusalCode::QuestionOpen);
    assert_eq!(
        reason(&f, "x"),
        "the harness is waiting on an answer to its own question"
    );
    assert_eq!(
        can_send(&with_phase(|p| p.question_open = false), "x"),
        Ok(())
    );
}

#[test]
fn any_other_non_waiting_phase_refuses_naming_it() {
    for (ph, name) in [
        (ActivityPhase::Running, "running"),
        (ActivityPhase::Idle, "idle"),
        (ActivityPhase::Spawning, "spawning"),
        (ActivityPhase::Exited, "exited"),
    ] {
        let f = with_phase(|p| p.phase = ph);
        assert_eq!(code(&f, "x"), RefusalCode::NotAtStoppingPoint);
        assert_eq!(
            reason(&f, "x"),
            format!("the harness isn't at a safe stopping point (currently {name})")
        );
    }
}

#[test]
fn a_running_harness_with_a_delegation_deferral_is_at_a_stopping_point() {
    let f = with_phase(|p| {
        p.phase = ActivityPhase::Running;
        p.delegation_deferred_at = Some(5);
    });
    assert_eq!(can_send(&f, "x"), Ok(()));
    let f = with_phase(|p| p.phase = ActivityPhase::Running);
    assert_eq!(code(&f, "x"), RefusalCode::NotAtStoppingPoint);
}

#[test]
fn at_safe_stopping_point_table() {
    use ActivityPhase::{Exited, Idle, Permission, Running, Spawning, Waiting};
    for (ph, deferred, want) in [
        (Waiting, None, true),
        (Waiting, Some(1), true),
        (Running, Some(1), true),
        (Running, None, false),
        (Permission, Some(1), false),
        (Idle, Some(1), false),
        (Spawning, None, false),
        (Exited, None, false),
    ] {
        let p = phase(|p| {
            p.phase = ph;
            p.delegation_deferred_at = deferred;
        });
        assert_eq!(at_safe_stopping_point(&p), want, "{ph:?} {deferred:?}");
    }
}

#[test]
fn refuses_without_a_source_proven_to_be_watching() {
    for f in [
        with_phase(|p| p.authoritative = false),
        with_phase(|p| p.adapter_heard = false),
        with_phase(|p| p.adapter_silent = true),
    ] {
        assert_eq!(code(&f, "x"), RefusalCode::NotWatched);
        assert_eq!(
            reason(&f, "x"),
            "no confirmed adapter is watching this harness"
        );
    }
}

#[test]
fn refuses_a_harness_without_config_injection() {
    let f = with_phase(|p| p.injected = false);
    assert_eq!(code(&f, "x"), RefusalCode::NotInjected);
    assert!(reason(&f, "x").contains("review tools"));
}

fn hermes(over: impl FnOnce(&mut PhaseFacts)) -> GateFacts {
    let mut f = with_phase(|p| {
        p.adapter_heard = true;
        p.launch_signal_at = Some(1_000_000 - 10_000);
        over(p);
    });
    f.kind = HarnessKind::Hermes;
    f.bracketed_paste = true;
    f
}

#[test]
fn hermes_allows_single_and_multi_line_with_hooks_and_waiting() {
    let f = hermes(|_| {});
    assert_eq!(can_send(&f, "x"), Ok(()));
    assert_eq!(can_send(&f, "a\nb"), Ok(()));
}

#[test]
fn hermes_screen_read_only_stays_refused() {
    let f = hermes(|p| {
        p.authoritative = false;
        p.adapter_heard = false;
        p.launch_signal_at = None;
    });
    assert!(reason(&f, "x").contains("no confirmed adapter"));
}

#[test]
fn hermes_refuses_permission_silent_adapter_and_missing_injection() {
    assert_eq!(
        code(&hermes(|p| p.phase = ActivityPhase::Permission), "x"),
        RefusalCode::Permission
    );
    assert_eq!(
        code(&hermes(|p| p.adapter_silent = true), "x"),
        RefusalCode::NotWatched
    );
    assert_eq!(
        code(&hermes(|p| p.injected = false), "x"),
        RefusalCode::NotInjected
    );
}

#[test]
fn hermes_needs_paste_mode_even_for_one_line() {
    let mut f = hermes(|_| {});
    f.bracketed_paste = false;
    assert_eq!(code(&f, "x"), RefusalCode::NeedsPasteMode);
    assert!(reason(&f, "x").contains("paste mode"));
}

#[test]
fn a_kind_that_does_not_need_paste_mode_is_not_held_to_it() {
    assert_eq!(can_send(&facts(|f| f.bracketed_paste = false), "x"), Ok(()));
}

#[test]
fn a_launch_signal_alone_is_proof_of_life() {
    let f = with_phase(|p| {
        p.adapter_heard = false;
        p.launch_signal_at = Some(1_000_000 - 10_000);
    });
    assert_eq!(can_send(&f, "x"), Ok(()));
}

#[test]
fn a_launch_signalled_harness_still_needs_injection_and_a_live_watchdog() {
    let f = with_phase(|p| {
        p.adapter_heard = false;
        p.launch_signal_at = Some(1_000_000);
        p.injected = false;
    });
    assert_eq!(code(&f, "x"), RefusalCode::NotInjected);
    let f = with_phase(|p| {
        p.adapter_heard = false;
        p.launch_signal_at = Some(1_000_000);
        p.adapter_silent = true;
    });
    assert_eq!(code(&f, "x"), RefusalCode::NotWatched);
}

#[test]
fn multi_line_needs_paste_mode() {
    let f = facts(|_| {});
    assert_eq!(code(&f, "line one\nline two"), RefusalCode::NeedsPasteMode);
    assert!(reason(&f, "line one\nline two").contains("multi-line"));
    let f = facts(|f| f.bracketed_paste = true);
    assert_eq!(can_send(&f, "line one\nline two"), Ok(()));
}

mod settling {
    use super::*;

    const SIGNAL: i64 = 1_000_000;

    fn launching(last_output_at: Option<i64>, now_ms: i64, heard: bool) -> GateFacts {
        let mut f = with_phase(|p| {
            p.adapter_heard = heard;
            p.launch_signal_at = Some(SIGNAL);
        });
        f.last_output_at = last_output_at;
        f.now_ms = now_ms;
        f
    }

    #[test]
    fn refuses_the_first_paste_while_output_is_recent() {
        let f = launching(Some(SIGNAL + 200), SIGNAL + 200, false);
        assert_eq!(code(&f, "x"), RefusalCode::Settling);
        assert!(reason(&f, "x").contains("#404"));
    }

    #[test]
    fn allows_once_past_the_quiet_period() {
        let f = launching(Some(SIGNAL + 200), SIGNAL + 200 + LAUNCH_QUIET_MS, false);
        assert_eq!(can_send(&f, "x"), Ok(()));
        let f = launching(
            Some(SIGNAL + 200),
            SIGNAL + 200 + LAUNCH_QUIET_MS - 1,
            false,
        );
        assert_eq!(code(&f, "x"), RefusalCode::Settling);
    }

    #[test]
    fn a_stale_output_stamp_does_not_pull_quiet_backwards() {
        let f = launching(Some(SIGNAL - 5_000), SIGNAL + 100, false);
        assert_eq!(code(&f, "x"), RefusalCode::Settling);
    }

    #[test]
    fn no_output_at_all_measures_from_the_signal() {
        assert_eq!(
            can_send(&launching(None, SIGNAL + LAUNCH_QUIET_MS, false), "x"),
            Ok(())
        );
        assert_eq!(
            code(&launching(None, SIGNAL + 1, false), "x"),
            RefusalCode::Settling
        );
    }

    #[test]
    fn the_cap_outranks_recent_output() {
        let now = SIGNAL + LAUNCH_READY_CAP_MS;
        assert_eq!(can_send(&launching(Some(now - 1), now, false), "x"), Ok(()));
    }

    #[test]
    fn is_unaffected_once_the_adapter_has_heard() {
        let f = launching(Some(SIGNAL + 200), SIGNAL + 200, true);
        assert_eq!(can_send(&f, "x"), Ok(()));
    }
}
